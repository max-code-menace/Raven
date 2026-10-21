//! Importer des ouvrages dans la bibliothèque (inventaire de Vertuoza, 17/10/2026) : la bibliothèque d'Ubique compte
//! 209 ouvrages ; Vertuoza les exporte en Excel (menu « ⋯ » › « Exporter ouvrages simples »).
//!
//! On envoie un fichier Excel (.xlsx) ou CSV (point-virgule, virgule ou tabulation). La ligne d'en-tête est
//! retrouvée par le titre des colonnes, pas par leur position : Description (obligatoire), Type, Catégorie,
//! Fournisseur, Unité, Prix d'achat, Marge, Prix de vente. Sans prix de vente, il est calculé depuis le prix d'achat
//! et la marge (une marge vide vaut 0 %, comme chez Ubique). Un ouvrage déjà présent (même description et même
//! unité) n'est pas ajouté une seconde fois : on peut relancer l'import sans créer de doublons.

use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;
use sqlx::{Connection as _, PgConnection};

use crate::base::Base;
use crate::bibliotheque::{self, Saisie};
use crate::import_excel::{self, Cellule, cle};
use crate::{AppState, Erreur};

const TAILLE_MAX: usize = 10 * 1024 * 1024;
const LIGNES_MAX: usize = 5_000;

/// Les colonnes trouvées dans la ligne d'en-tête.
#[derive(Default, Debug)]
struct Colonnes {
    description: usize,
    genre: Option<usize>,
    categorie: Option<usize>,
    fournisseur: Option<usize>,
    unite: Option<usize>,
    achat: Option<usize>,
    marge: Option<usize>,
    vente: Option<usize>,
}

fn trouver_entete(grille: &[Vec<Cellule>]) -> Option<(usize, Colonnes)> {
    for (i, ligne) in grille.iter().enumerate().take(30) {
        let mut c = Colonnes::default();
        let mut description = None;
        for (j, case) in ligne.iter().enumerate() {
            let titre = cle(&case.texte());
            if titre.is_empty() {
                continue;
            }
            let premiere = |o: &mut Option<usize>| {
                if o.is_none() {
                    *o = Some(j);
                }
            };
            if titre.contains("description") || titre.contains("designation") || titre == "libelle" || titre == "nom" {
                premiere(&mut description);
            } else if titre.contains("categorie") {
                premiere(&mut c.categorie);
            } else if titre.contains("fournisseur") {
                premiere(&mut c.fournisseur);
            } else if titre.starts_with("unite") || titre == "u" {
                premiere(&mut c.unite);
            } else if titre.contains("achat") {
                premiere(&mut c.achat);
            } else if titre.contains("marge") {
                premiere(&mut c.marge);
            } else if titre.contains("vente") || titre.starts_with("prix") {
                premiere(&mut c.vente);
            } else if titre == "type" || titre.starts_with("type ") {
                premiere(&mut c.genre);
            }
        }
        if let Some(d) = description
            && (c.achat.is_some() || c.vente.is_some())
        {
            c.description = d;
            return Some((i, c));
        }
    }
    None
}

/// Le type de ligne d'après le texte du fichier (« Main d'œuvre », « Fourniture »…) ; « Ouvrage » par défaut.
fn genre(texte: &str) -> &'static str {
    let t = cle(texte);
    if t.contains("fourniture") {
        "fourniture"
    } else if t.contains("main") || t.contains("prestation") {
        "main_oeuvre"
    } else if t.contains("sous") && t.contains("trait") {
        "sous_traitant"
    } else if t.contains("equipement") {
        "equipement"
    } else if t.contains("libre") {
        "poste_libre"
    } else {
        "ouvrage"
    }
}

/// Un nombre du fichier, écrit pour la saisie (« 197,5 »). Vide si la case est vide.
fn nombre(case: Option<&Cellule>) -> String {
    match case {
        Some(Cellule::Texte(t)) => t.replace(['\u{a0}', ' ', '€', '%'], ""),
        Some(c @ Cellule::Nombre(_)) => c.nombre_e5().map(crate::calcul::ecrire_decimal_e5).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Ce que le fichier contient : les ouvrages lus, et les lignes écartées (avec leur numéro dans le fichier).
#[derive(Debug, Default)]
pub(crate) struct Lecture {
    pub ouvrages: Vec<bibliotheque::Valeurs>,
    pub ecartees: Vec<String>,
}

pub(crate) fn lire_grille(grille: &[Vec<Cellule>]) -> Result<Lecture, String> {
    let (entete, c) = trouver_entete(grille).ok_or_else(|| {
        "La ligne d'en-tête n'a pas été trouvée : il faut au moins une colonne « Description » et une colonne de prix (« Prix d'achat » ou « Prix de vente »).".to_string()
    })?;
    let mut lecture = Lecture::default();
    for (i, ligne) in grille.iter().enumerate().skip(entete + 1).take(LIGNES_MAX) {
        let case = |o: Option<usize>| o.and_then(|j| ligne.get(j));
        let texte = |o: Option<usize>| case(o).map(Cellule::texte).unwrap_or_default();
        let description = texte(Some(c.description));
        if description.is_empty() {
            continue;
        }
        let mut marge = nombre(case(c.marge));
        let vente = nombre(case(c.vente));
        if vente.is_empty() && marge.is_empty() {
            marge = "0".to_string();
        }
        let saisie = Saisie {
            description,
            genre: genre(&texte(c.genre)).to_string(),
            categorie: texte(c.categorie),
            fournisseur: texte(c.fournisseur),
            unite: texte(c.unite).replace("M2", "m²").replace("M3", "m³"),
            prix_achat: nombre(case(c.achat)),
            marge,
            prix_vente: vente,
            remarque: String::new(),
        };
        match bibliotheque::verifier(&saisie) {
            Ok(v) => lecture.ouvrages.push(v),
            Err(erreurs) => lecture.ecartees.push(format!("Ligne {} : {}", i + 1, erreurs.join(" "))),
        }
    }
    if lecture.ouvrages.is_empty() && lecture.ecartees.is_empty() {
        return Err("Le fichier ne contient aucun ouvrage sous la ligne d'en-tête.".to_string());
    }
    Ok(lecture)
}

/// Découpe un fichier CSV (séparateur deviné sur la première ligne : point-virgule, tabulation ou virgule ;
/// guillemets doublés acceptés).
pub(crate) fn grille_csv(texte: &str) -> Vec<Vec<Cellule>> {
    let texte = texte.trim_start_matches('\u{feff}');
    let premiere = texte.lines().next().unwrap_or("");
    let separateur = [';', '\t', ','].into_iter().max_by_key(|s| premiere.matches(*s).count()).unwrap_or(';');
    let mut grille = Vec::new();
    let mut ligne = Vec::new();
    let mut champ = String::new();
    let mut guillemets = false;
    let mut caracteres = texte.chars().peekable();
    let fin_champ = |champ: &mut String, ligne: &mut Vec<Cellule>| {
        let t = std::mem::take(champ);
        ligne.push(if t.trim().is_empty() { Cellule::Vide } else { Cellule::Texte(t) });
    };
    while let Some(c) = caracteres.next() {
        match c {
            '"' if guillemets && caracteres.peek() == Some(&'"') => {
                champ.push('"');
                caracteres.next();
            }
            '"' => guillemets = !guillemets,
            c if c == separateur && !guillemets => fin_champ(&mut champ, &mut ligne),
            '\r' if !guillemets => {}
            '\n' if !guillemets => {
                fin_champ(&mut champ, &mut ligne);
                grille.push(std::mem::take(&mut ligne));
            }
            c => champ.push(c),
        }
    }
    if !champ.is_empty() || !ligne.is_empty() {
        fin_champ(&mut champ, &mut ligne);
        grille.push(ligne);
    }
    grille
}

/// Lit un fichier .xlsx (la première feuille qui contient l'en-tête) ou CSV.
pub(crate) fn lire_fichier(nom: &str, octets: &[u8]) -> Result<Lecture, String> {
    if octets.starts_with(b"PK") {
        let mut derniere = "Le classeur ne contient aucune feuille.".to_string();
        for grille in import_excel::feuilles_xlsx(octets)? {
            match lire_grille(&grille) {
                Ok(l) => return Ok(l),
                Err(e) => derniere = e,
            }
        }
        return Err(derniere);
    }
    let texte = match std::str::from_utf8(octets) {
        Ok(t) => t.to_string(),
        // Un CSV enregistré par Excel en Windows-1252 : les octets ASCII et Latin-1 suffisent ici.
        Err(_) => octets.iter().map(|b| char::from(*b)).collect(),
    };
    if nom.to_lowercase().ends_with(".xls") {
        return Err("Le format .xls (ancien Excel) n'est pas lu : enregistrez le fichier en .xlsx ou en CSV.".to_string());
    }
    lire_grille(&grille_csv(&texte))
}

/// Ajoute les ouvrages qui ne sont pas déjà dans la bibliothèque. Renvoie (ajoutés, déjà présents).
async fn ajouter(db: &mut PgConnection, ouvrages: &[bibliotheque::Valeurs]) -> Result<(usize, usize), sqlx::Error> {
    let mut tx = db.begin().await?;
    let (mut ajoutes, mut deja) = (0, 0);
    for v in ouvrages {
        let present: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM ouvrages WHERE lower(description) = lower($1) AND unite = $2 AND supprime_le IS NULL)")
            .bind(&v.description)
            .bind(&v.unite)
            .fetch_one(&mut *tx)
            .await?;
        if present {
            deja += 1;
        } else {
            bibliotheque::inserer(&mut *tx, v).await?;
            ajoutes += 1;
        }
    }
    tx.commit().await?;
    Ok((ajoutes, deja))
}

// ─── Écran ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "bibliotheque_import.html")]
struct Page {
    erreur: String,
    resultat: Option<(usize, usize)>,
    ecartees: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ajoutes: String,
    deja: String,
}

async fn formulaire(Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let resultat = match (r.ajoutes.parse(), r.deja.parse()) {
        (Ok(a), Ok(d)) => Some((a, d)),
        _ => None,
    };
    Ok(Html(Page { erreur: String::new(), resultat, ecartees: Vec::new() }.render()?))
}

fn erreur(message: &str, ecartees: Vec<String>) -> Result<Response, Erreur> {
    Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(Page { erreur: message.to_string(), resultat: None, ecartees }.render()?)).into_response())
}

async fn recevoir(mut db: Base, mut multipart: Multipart) -> Result<Response, Erreur> {
    let mut recu: Option<(String, Vec<u8>)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(champ)) if champ.name() == Some("fichier") => {
                let nom = champ.file_name().unwrap_or("").to_string();
                match champ.bytes().await {
                    Ok(octets) => recu = Some((nom, octets.to_vec())),
                    Err(_) => return erreur("Le fichier est trop gros (10 Mo au maximum) ou illisible.", Vec::new()),
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => return erreur("Le fichier est trop gros (10 Mo au maximum) ou illisible.", Vec::new()),
        }
    }
    let Some((nom, octets)) = recu.filter(|(_, o)| !o.is_empty()) else {
        return erreur("Choisissez un fichier avant de cliquer sur « Importer ».", Vec::new());
    };
    let lecture = match tokio::task::spawn_blocking(move || lire_fichier(&nom, &octets)).await {
        Ok(Ok(l)) => l,
        Ok(Err(message)) => return erreur(&message, Vec::new()),
        Err(_) => return erreur("Le fichier n'a pas pu être lu.", Vec::new()),
    };
    if lecture.ouvrages.is_empty() {
        return erreur("Aucun ouvrage n'a pu être lu : voir les lignes écartées.", lecture.ecartees);
    }
    let (ajoutes, deja) = ajouter(&mut *db, &lecture.ouvrages).await?;
    if !lecture.ecartees.is_empty() {
        // Le résultat et les lignes écartées sur la même page (rien n'est perdu de vue).
        let page = Page { erreur: String::new(), resultat: Some((ajoutes, deja)), ecartees: lecture.ecartees };
        return Ok(Html(page.render()?).into_response());
    }
    Ok(Redirect::to(&format!("/bibliotheque/import?ajoutes={ajoutes}&deja={deja}")).into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/bibliotheque/import", get(formulaire).post(recevoir).layer(DefaultBodyLimit::max(TAILLE_MAX + 64 * 1024)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_csv_de_vertuoza() {
        let csv = "\u{feff}Description;Type;Catégorie;Fournisseur;Unité;Prix d’achat;Marge\n\
                   Modification alimentation boiler;Ouvrage;;;fft;197,50000 €;0 %\n\
                   \"Prise double bipolaire + terre; sécurité\";;;;pce;107,8;\n\
                   ;;;;;;\n\
                   Main d'œuvre électricien;Main d'œuvre;Électricité;;H;50;20\n\
                   Sans prix;;;;pce;;\n";
        let l = lire_grille(&grille_csv(csv)).unwrap();
        assert_eq!(l.ouvrages.len(), 4);
        let d: Vec<(&str, &str, Option<i64>, Option<i64>)> =
            l.ouvrages.iter().map(|v| (v.description.as_str(), v.genre, v.prix_achat_e5, v.prix_vente_e5)).collect();
        assert_eq!(d[0], ("Modification alimentation boiler", "ouvrage", Some(19_750_000), Some(19_750_000)));
        assert_eq!(d[1], ("Prise double bipolaire + terre; sécurité", "ouvrage", Some(10_780_000), Some(10_780_000)));
        // 50 € + 20 % de marge = 60 €.
        assert_eq!(d[2], ("Main d'œuvre électricien", "main_oeuvre", Some(5_000_000), Some(6_000_000)));
        assert_eq!(l.ouvrages[2].categorie, "Électricité");
        // Une ligne sans aucun prix est gardée, prix « à compléter ».
        assert_eq!((l.ouvrages[3].description.as_str(), l.ouvrages[3].prix_vente_e5), ("Sans prix", None));
        assert!(l.ecartees.is_empty(), "{:?}", l.ecartees);
    }

    #[test]
    fn un_fichier_avec_prix_de_vente() {
        let csv = "Désignation,Unité,Prix de vente\nCarrelage,M2,\"83,5\"\n";
        let l = lire_grille(&grille_csv(csv)).unwrap();
        assert_eq!((l.ouvrages[0].unite.as_str(), l.ouvrages[0].prix_achat_e5, l.ouvrages[0].prix_vente_e5), ("m²", None, Some(8_350_000)));
    }

    #[test]
    fn un_fichier_sans_en_tete() {
        assert!(lire_grille(&grille_csv("a;b;c\n1;2;3\n")).is_err());
        assert!(lire_fichier("vieux.xls", b"abc").is_err());
    }
}
