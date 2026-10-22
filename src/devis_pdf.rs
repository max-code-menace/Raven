//! Le devis en PDF (étape 5c-1) : les données du devis, leur mise en page avec Typst
//! (`assets/devis.typ`), et les conditions générales ajoutées à la suite.
//! Les prix d'achat et la marge n'entrent jamais dans ces données : le PDF est fait pour le client.

use std::collections::HashMap;

use axum::{
    Router,
    extract::Path,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgConnection};

use crate::base::Base;
use crate::commun::{PAYS, libelle};
use crate::devis_lignes::{Encodage, charger_encodage, charger_entete};
use crate::fiches::{Genre, Options};
use crate::{AppState, Erreur, pdf};

const MODELE: &str = include_str!("../assets/devis.typ");

// ─── Les données du devis ───────────────────────────────────────────────────

/// Un nom et une adresse, prêts à afficher.
pub struct Bloc {
    pub nom: String,
    pub adresse: Vec<String>,
    pub tva: String,
}

pub struct Societe {
    pub bloc: Bloc,
    pub email: String,
    pub telephone: String,
    pub iban: String,
    /// Nom de fichier (avec la bonne extension) et contenu du logo.
    pub logo: Option<(String, Vec<u8>)>,
}

pub struct Donnees {
    pub id: i64,
    /// Une ligne au taux « Franchise de TVA » : la mention légale est imprimée.
    pub franchise: bool,
    /// La société est en franchise de TVA.
    pub regime_franchise: bool,
    /// Des lignes chiffrées dont le taux ne correspond pas au régime de TVA de la société.
    pub hors_regime: bool,
    /// L'organisation parle de « projet » (sinon « chantier »).
    pub projet: bool,
    pub numero: String,
    /// « Devis » ou « Avenant ».
    pub genre: &'static str,
    /// Pour un avenant : « Chantier n° 1 · devis 2026-57 ». Vide pour un devis.
    pub corrige: String,
    pub brouillon: bool,
    pub date: String,
    pub reference: String,
    pub responsable: String,
    pub societe: Option<Societe>,
    pub client: Option<Bloc>,
    /// Présent seulement si le devis est facturé à quelqu'un d'autre que le client.
    pub facturation: Option<Bloc>,
    pub chantier: Vec<String>,
    pub conditions: String,
    pub remarque_1: String,
    pub remarque_2: String,
    pub enc: Encodage,
}

impl Donnees {
    /// Ce qu'il reste à compléter avant d'envoyer le devis au client.
    pub fn a_completer(&self) -> Vec<String> {
        let mut manques = Vec::new();
        if self.reference.is_empty() {
            manques.push("La référence du devis".to_string());
        }
        if self.client.is_none() {
            manques.push("Le client".to_string());
        }
        if self.conditions.is_empty() {
            manques.push("Les conditions de paiement".to_string());
        }
        if !self.enc.lignes.iter().any(|l| l.est_chiffre) {
            manques.push("Au moins une ligne chiffrée (onglet Encodage)".to_string());
        }
        if self.hors_regime {
            manques.push(if self.regime_franchise {
                "Des lignes au taux « Franchise de TVA » (votre société est en franchise de TVA)".to_string()
            } else {
                "Des lignes sans le taux « Franchise de TVA », réservé aux sociétés en franchise (Réglages › Société)".to_string()
            });
        }
        match &self.societe {
            None => manques.push("Les coordonnées de votre société (Réglages › Société)".to_string()),
            Some(s) => {
                if s.bloc.tva.is_empty() {
                    manques.push("Le numéro de TVA de votre société (Réglages › Société)".to_string());
                }
                if s.iban.is_empty() {
                    manques.push("Un compte bancaire par défaut (Réglages › Société)".to_string());
                }
            }
        }
        manques
    }
}

/// « Rue 12 », « 1000 Bruxelles », puis le pays s'il n'est pas la Belgique.
pub fn lignes_adresse(rue: &str, numero: &str, code_postal: &str, ville: &str, pays: &str) -> Vec<String> {
    let rue = format!("{rue} {numero}").trim().to_string();
    let ville = format!("{code_postal} {ville}").trim().to_string();
    let mut lignes: Vec<String> = [rue, ville].into_iter().filter(|s| !s.is_empty()).collect();
    if !lignes.is_empty() && pays != "BE" && !pays.is_empty() {
        lignes.push(libelle(&PAYS, pays));
    }
    lignes
}

type Adresse = (String, String, String, String, String, String, String);

fn depuis_adresse((nom, rue, numero, code_postal, ville, pays, tva): Adresse) -> Bloc {
    Bloc { nom, adresse: lignes_adresse(&rue, &numero, &code_postal, &ville, &pays), tva }
}

/// L'entreprise ou le contact choisi ; `facturation` demande son adresse de facturation
/// (qui est l'adresse normale quand « facturation identique » est coché sur sa fiche).
pub(crate) async fn charger_bloc(db: &mut PgConnection, entreprise: Option<i64>, contact: Option<i64>, facturation: bool) -> Result<Option<Bloc>, sqlx::Error> {
    let champs = |nom: &str| {
        let colonne = |c: &str| {
            if facturation {
                format!("CASE WHEN facturation_identique THEN {c} ELSE COALESCE(facturation_{c}, '') END")
            } else {
                c.to_string()
            }
        };
        format!(
            "SELECT {nom}, {}, {}, {}, {}, {}, COALESCE(numero_tva, '')",
            colonne("rue"),
            colonne("numero"),
            colonne("code_postal"),
            colonne("ville"),
            colonne("pays")
        )
    };
    if let Some(id) = entreprise {
        let sql = format!("{} FROM entreprises WHERE id = $1", champs("nom"));
        let ligne: Option<Adresse> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(&mut *db).await?;
        return Ok(ligne.map(depuis_adresse));
    }
    if let Some(id) = contact {
        let sql = format!("{} FROM contacts WHERE id = $1", champs("trim(concat_ws(' ', prenom, nom))"));
        let ligne: Option<Adresse> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(&mut *db).await?;
        return Ok(ligne.map(depuis_adresse));
    }
    Ok(None)
}

pub(crate) async fn charger_societe(db: &mut PgConnection) -> Result<Option<Societe>, sqlx::Error> {
    let ligne: Option<(String, String, String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT nom, rue, numero, code_postal, ville, pays, COALESCE(numero_tva, ''), email, telephone
         FROM societe WHERE organisation_id = app_organisation()",
    )
    .fetch_optional(&mut *db)
    .await?;
    let Some((nom, rue, numero, code_postal, ville, pays, tva, email, telephone)) = ligne else {
        return Ok(None);
    };
    let iban: Option<String> =
        sqlx::query_scalar("SELECT iban FROM societe_comptes_bancaires WHERE organisation_id = app_organisation() AND par_defaut").fetch_optional(&mut *db).await?;
    let logo: Option<(String, Vec<u8>)> = sqlx::query_as("SELECT type_mime, contenu FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = 'logo'")
        .fetch_optional(&mut *db)
        .await?;
    // L'extension dit à Typst quel format d'image lire.
    let logo = logo.and_then(|(mime, contenu)| match mime.as_str() {
        "image/png" => Some(("logo.png".to_string(), contenu)),
        "image/jpeg" => Some(("logo.jpg".to_string(), contenu)),
        _ => None,
    });
    Ok(Some(Societe {
        bloc: Bloc { nom, adresse: lignes_adresse(&rue, &numero, &code_postal, &ville, &pays), tva },
        email,
        telephone,
        iban: iban.unwrap_or_default(),
        logo,
    }))
}

#[derive(FromRow)]
struct Tete {
    date_devis: NaiveDate,
    reference: String,
    responsable: String,
    client_entreprise_id: Option<i64>,
    client_contact_id: Option<i64>,
    facturation_identique: bool,
    facturation_entreprise_id: Option<i64>,
    facturation_contact_id: Option<i64>,
    chantier_identique: bool,
    chantier_rue: String,
    chantier_numero: String,
    chantier_code_postal: String,
    chantier_ville: String,
    chantier_pays: String,
    conditions_paiement_id: Option<i64>,
    remarque_1: String,
    remarque_2: String,
}

pub async fn charger_donnees(db: &mut PgConnection, id: i64) -> Result<Donnees, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    let tete: Tete = sqlx::query_as(
        "SELECT date_devis, reference, responsable, client_entreprise_id, client_contact_id, facturation_identique,
                facturation_entreprise_id, facturation_contact_id, chantier_identique, chantier_rue, chantier_numero,
                chantier_code_postal, chantier_ville, chantier_pays, conditions_paiement_id, remarque_1, remarque_2
         FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await?;

    let client = charger_bloc(&mut *db, tete.client_entreprise_id, tete.client_contact_id, false).await?;
    let facturation = if tete.facturation_identique {
        None
    } else {
        charger_bloc(&mut *db, tete.facturation_entreprise_id, tete.facturation_contact_id, true).await?
    };
    let chantier = if tete.chantier_identique {
        client.as_ref().map(|c| c.adresse.clone()).unwrap_or_default()
    } else {
        lignes_adresse(&tete.chantier_rue, &tete.chantier_numero, &tete.chantier_code_postal, &tete.chantier_ville, &tete.chantier_pays)
    };
    let options = Options::charger(&mut *db, Genre::Entreprise).await?;
    let conditions = tete
        .conditions_paiement_id
        .and_then(|cid| options.conditions.iter().find(|(i, _)| *i == cid).map(|(_, t)| t.clone()))
        .unwrap_or_default();
    let enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    let corrige: Option<String> = sqlx::query_scalar(
        "SELECT 'Chantier n° ' || ch.numero || ' · devis ' || extract(year FROM b.date_devis)::int || '-' || b.numero
         FROM devis a JOIN chantiers ch ON ch.id = a.chantier_id JOIN devis b ON b.id = ch.devis_id
         WHERE a.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;

    let projet = crate::profil::projet_en_base(&mut *db).await?;
    let corrige = corrige.map(|c| if projet { c.replacen("Chantier", "Projet", 1) } else { c });
    let franchise: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM devis_lignes l JOIN taux_tva t ON t.id = l.taux_tva_id WHERE l.devis_id = $1 AND t.franchise)",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await?;
    let regime_franchise = crate::societe::en_franchise(&mut *db).await?;
    let hors_regime: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM devis_lignes l JOIN taux_tva t ON t.id = l.taux_tva_id WHERE l.devis_id = $1 AND t.franchise <> $2)",
    )
    .bind(id)
    .bind(regime_franchise)
    .fetch_one(&mut *db)
    .await?;
    Ok(Donnees {
        id,
        franchise,
        regime_franchise,
        hors_regime,
        projet,
        numero: entete.numero,
        genre: entete.genre,
        corrige: corrige.unwrap_or_default(),
        brouillon: entete.brouillon,
        date: tete.date_devis.format("%d/%m/%Y").to_string(),
        reference: tete.reference,
        responsable: tete.responsable,
        societe: charger_societe(&mut *db).await?,
        client,
        facturation,
        chantier,
        conditions,
        remarque_1: tete.remarque_1,
        remarque_2: tete.remarque_2,
        enc,
    })
}

// ─── Le texte Typst ─────────────────────────────────────────────────────────

/// Un texte entre guillemets, tel que Typst le lit : aucun caractère ne peut en sortir.
pub(crate) fn litteral(texte: &str) -> String {
    let mut sortie = String::with_capacity(texte.len() + 2);
    sortie.push('"');
    for c in texte.chars() {
        match c {
            '\\' => sortie.push_str("\\\\"),
            '"' => sortie.push_str("\\\""),
            '\n' => sortie.push_str("\\n"),
            '\r' => {}
            '\t' => sortie.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => sortie.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => sortie.push(c),
        }
    }
    sortie.push('"');
    sortie
}

/// `(clé: valeur, …)` ; les valeurs sont déjà du texte Typst.
pub(crate) fn dictionnaire(champs: &[(&str, String)]) -> String {
    if champs.is_empty() {
        return "(:)".to_string();
    }
    let milieu: Vec<String> = champs.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    format!("({})", milieu.join(", "))
}

/// `(a, b,)` : la virgule finale garde un tableau d'un seul élément.
pub(crate) fn tableau(elements: &[String]) -> String {
    if elements.is_empty() { "()".to_string() } else { format!("({},)", elements.join(", ")) }
}

pub(crate) fn option_litteral(texte: Option<&str>) -> String {
    texte.map(litteral).unwrap_or_else(|| "none".to_string())
}

pub(crate) fn bloc_typst(b: &Bloc) -> String {
    let adresse: Vec<String> = b.adresse.iter().map(|l| litteral(l)).collect();
    dictionnaire(&[("nom", litteral(&b.nom)), ("adresse", tableau(&adresse)), ("tva", litteral(&b.tva))])
}

/// Le document complet : le modèle, puis l'appel qui lui donne les données du devis.
/// Renvoie aussi les fichiers cités (le logo).
pub fn source_typst(d: &Donnees) -> (String, HashMap<String, Vec<u8>>) {
    let mut fichiers = HashMap::new();

    let societe = match &d.societe {
        None => "none".to_string(),
        Some(s) => {
            let adresse: Vec<String> = s.bloc.adresse.iter().map(|l| litteral(l)).collect();
            let logo = s.logo.as_ref().map(|(nom, octets)| {
                fichiers.insert(nom.clone(), octets.clone());
                nom.as_str()
            });
            dictionnaire(&[
                ("nom", litteral(&s.bloc.nom)),
                ("adresse", tableau(&adresse)),
                ("telephone", litteral(&s.telephone)),
                ("email", litteral(&s.email)),
                ("tva", litteral(&s.bloc.tva)),
                ("iban", litteral(&s.iban)),
                ("logo", option_litteral(logo)),
            ])
        }
    };

    // Les lignes, par « segments » : un saut de page du devis ouvre un nouveau segment.
    let mut segments: Vec<Vec<String>> = vec![Vec::new()];
    for l in &d.enc.lignes {
        if l.est_saut {
            segments.push(Vec::new());
            continue;
        }
        let ligne = if l.est_chiffre {
            dictionnaire(&[
                ("type", litteral("poste")),
                ("numero", litteral(&l.numero)),
                ("description", litteral(&l.description)),
                ("remarque", litteral(&l.remarque)),
                ("type_quantite", litteral(l.sigle_quantite)),
                ("quantite", litteral(&l.quantite)),
                ("unite", litteral(&l.unite)),
                ("prix_unitaire", litteral(&l.prix_affiche)),
                ("tva", litteral(&l.taux_texte)),
                ("total", litteral(&l.total)),
            ])
        } else {
            let genre = if l.est_titre { "titre" } else { l.genre };
            dictionnaire(&[
                ("type", litteral(genre)),
                ("numero", litteral(&l.numero)),
                ("description", litteral(&l.description)),
                ("remarque", litteral(&l.remarque)),
                ("sous_total", litteral(&l.sous_total)),
            ])
        };
        if let Some(dernier) = segments.last_mut() {
            dernier.push(ligne);
        }
    }
    let segments: Vec<String> = segments.iter().map(|s| tableau(s)).collect();

    // La légende des types de quantité employés dans le devis, dans l'ordre de l'Encodage.
    let legende: Vec<String> = crate::devis_lignes::TYPES_QUANTITE
        .iter()
        .filter(|(valeur, ..)| d.enc.lignes.iter().any(|l| l.est_chiffre && l.type_quantite == *valeur))
        .map(|(valeur, sigle, mot, _)| {
            let texte = match *valeur {
                "estimee" => format!("{}, facturée selon le métré réel", mot.to_lowercase()),
                _ => mot.to_lowercase(),
            };
            dictionnaire(&[("sigle", litteral(sigle)), ("texte", litteral(&texte))])
        })
        .collect();

    let t = &d.enc.totaux;
    let tva: Vec<String> = t
        .tva
        .iter()
        .map(|v| dictionnaire(&[("taux", litteral(&v.taux)), ("base", litteral(&v.base)), ("montant", litteral(&v.montant))]))
        .collect();
    let totaux = dictionnaire(&[
        ("a_ajustement", t.a_ajustement.to_string()),
        ("sous_total", litteral(&t.sous_total)),
        ("libelle_ajustement", litteral(&t.libelle_ajustement)),
        ("ajustement", litteral(&t.ajustement)),
        ("ht", litteral(&t.ht)),
        ("tva", tableau(&tva)),
        ("ttc", litteral(&t.ttc)),
    ]);

    let donnees = dictionnaire(&[
        ("numero", litteral(&d.numero)),
        ("genre", litteral(d.genre)),
        ("corrige", litteral(&d.corrige)),
        // Cette ligne porte l'adresse : « Chantier » (comme dans Vertuoza) ou, pour les autres métiers, « Adresse du projet ».
        ("mot_chantier", litteral(if d.projet { "Adresse du projet" } else { "Chantier" })),
        ("date", litteral(&d.date)),
        ("reference", litteral(&d.reference)),
        ("responsable", litteral(&d.responsable)),
        ("societe", societe),
        ("client", d.client.as_ref().map(bloc_typst).unwrap_or_else(|| "none".to_string())),
        ("facturation", d.facturation.as_ref().map(bloc_typst).unwrap_or_else(|| "none".to_string())),
        ("chantier", litteral(&d.chantier.join(", "))),
        ("conditions", litteral(&d.conditions)),
        ("franchise", d.franchise.to_string()),
        ("mention_franchise", litteral(crate::societe::MENTION_FRANCHISE)),
        ("remarque_1", litteral(&d.remarque_1)),
        ("remarque_2", litteral(&d.remarque_2)),
        ("segments", tableau(&segments)),
        ("legende", tableau(&legende)),
        ("totaux", totaux),
    ]);
    (format!("{MODELE}\n#devis({donnees})\n"), fichiers)
}

// ─── Le PDF ─────────────────────────────────────────────────────────────────

/// Le PDF du devis, conditions générales comprises quand elles sont déposées dans Société.
pub async fn pdf_du_devis(db: &mut PgConnection, id: i64) -> Result<(String, Vec<u8>), Erreur> {
    let donnees = charger_donnees(&mut *db, id).await?;
    let nom = format!("{}-{}.pdf", donnees.genre, donnees.numero);
    let (texte, fichiers) = source_typst(&donnees);
    let avait_un_logo = !fichiers.is_empty();

    // La mise en page est un travail de calcul : hors du fil qui répond aux requêtes.
    let essai = {
        let (texte, fichiers) = (texte.clone(), fichiers);
        tokio::task::spawn_blocking(move || pdf::generer(texte, fichiers)).await
    };
    let mut octets = match essai {
        Ok(Ok(octets)) => octets,
        // Un logo abîmé ne doit pas empêcher d'éditer le devis : on réessaie sans lui.
        Ok(Err(erreur)) if avait_un_logo => {
            eprintln!("PDF du devis {id} : {erreur} ; nouvel essai sans le logo");
            let (texte, _) = source_typst(&Donnees { societe: donnees.societe.map(|s| Societe { logo: None, ..s }), ..donnees });
            tokio::task::spawn_blocking(move || pdf::generer(texte, HashMap::new()))
                .await
                .map_err(|e| Erreur::pdf(&e.to_string()))?
                .map_err(|e| Erreur::pdf(&e))?
        }
        Ok(Err(erreur)) => return Err(Erreur::pdf(&erreur)),
        Err(e) => return Err(Erreur::pdf(&e.to_string())),
    };

    let conditions: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT contenu FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = 'conditions_generales'").fetch_optional(&mut *db).await?;
    if let Some(conditions) = conditions {
        match tokio::task::spawn_blocking({
            let base = octets.clone();
            move || pdf::fusionner(&base, &conditions)
        })
        .await
        {
            Ok(Ok(fusion)) => octets = fusion,
            // Des conditions générales illisibles ne bloquent pas le devis : il part sans elles.
            Ok(Err(erreur)) => eprintln!("PDF du devis {id} : conditions générales ignorées ({erreur})"),
            Err(e) => eprintln!("PDF du devis {id} : conditions générales ignorées ({e})"),
        }
    }
    Ok((nom, octets))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParametresPdf {
    telecharger: String,
}

async fn servir(mut db: Base, Path(id): Path<i64>, axum::extract::Query(o): axum::extract::Query<ParametresPdf>) -> Result<Response, Erreur> {
    let (nom, octets) = pdf_du_devis(&mut *db, id).await?;
    let telechargement = !o.telecharger.is_empty();
    if telechargement {
        // Seul le téléchargement est noté ; le simple affichage dans l'onglet Visualisation ne l'est pas.
        let mut tx = db.begin().await?;
        crate::journal::noter(&mut tx, id, crate::journal::Genre::Pdf, "PDF téléchargé").await?;
        tx.commit().await?;
    }
    let disposition = if telechargement { "attachment" } else { "inline" };
    let mut reponse = octets.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    // Le nom ne contient que des chiffres et un tiret : il est sûr dans un en-tête.
    entetes.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("{disposition}; filename=\"{nom}\"")).map_err(|_| Erreur::pdf("nom de fichier"))?,
    );
    entetes.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(reponse)
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/pdf", get(servir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_textes_sont_proteges_pour_typst() {
        assert_eq!(litteral("simple"), "\"simple\"");
        assert_eq!(litteral("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(litteral("ligne 1\nligne 2\r\n\ttab"), "\"ligne 1\\nligne 2\\n\\ttab\"");
        assert_eq!(litteral("\u{1}\u{7f}"), "\"\\u{1}\\u{7f}\"");
        // Du code Typst tapé par quelqu'un reste du texte.
        assert_eq!(litteral("#panic(\"x\")"), "\"#panic(\\\"x\\\")\"");
        assert_eq!(litteral("é € 日本"), "\"é € 日本\"");
    }

    #[test]
    fn dictionnaires_et_tableaux() {
        assert_eq!(dictionnaire(&[]), "(:)");
        assert_eq!(dictionnaire(&[("a", "1".into()), ("b", "\"x\"".into())]), "(a: 1, b: \"x\")");
        assert_eq!(tableau(&[]), "()");
        assert_eq!(tableau(&["1".into()]), "(1,)");
        assert_eq!(tableau(&["1".into(), "2".into()]), "(1, 2,)");
        assert_eq!(option_litteral(None), "none");
        assert_eq!(option_litteral(Some("logo.png")), "\"logo.png\"");
    }
}
