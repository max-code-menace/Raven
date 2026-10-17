//! Importer des entreprises ou des contacts (transition depuis Vertuoza, 17/10/2026) : Vertuoza exporte ses listes
//! Entreprises et Contacts en Excel (menu « ⋯ » › Exporter).
//!
//! On envoie un fichier Excel (.xlsx) ou CSV. La ligne d'en-tête est retrouvée par le titre des colonnes : Nom
//! (obligatoire), Prénom, Civilité, Entreprise, Profils, Téléphone, Téléphone secondaire, E-mail, TVA, IBAN, BIC,
//! et l'adresse, soit en colonnes (Rue, Numéro, Code postal, Ville, Pays), soit en une seule colonne comme dans la
//! liste de Vertuoza (« Rue du Beau Site 12, 1390 Grez-Doiceau Belgique »). Une fiche déjà présente (même nom et
//! même e-mail, ou même numéro de TVA pour une entreprise) n'est pas ajoutée une seconde fois : on peut relancer.

use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;
use sqlx::PgPool;

use crate::bibliotheque_import::grille_csv;
use crate::commun::{PAYS, email_valide};
use crate::import_excel::{self, Cellule, cle};
use crate::{AppState, Erreur};

const TAILLE_MAX: usize = 10 * 1024 * 1024;
const LIGNES_MAX: usize = 10_000;
const LONGUEUR_MAX: usize = 300;

/// (valeur en base, libellé) des profils, comme dans `fiches`.
const PROFILS: [(&str, &str); 10] = [
    ("prospect", "Prospect"),
    ("client", "Client"),
    ("fournisseur_materiaux", "Fournisseur - Matériaux"),
    ("fournisseur_sous_traitant", "Fournisseur - Sous-traitant"),
    ("co_traitant", "Co-traitant"),
    ("architecte", "Architecte"),
    ("autre", "Autre profil"),
    ("fournisseur_autres", "Fournisseur - Autres"),
    ("ex_client", "Ex-client"),
    ("proprietaire", "Propriétaire"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Genre {
    Entreprise,
    Contact,
}

/// Une fiche lue dans le fichier.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Fiche {
    pub nom: String,
    pub prenom: Option<String>,
    pub civilite: Option<&'static str>,
    pub entreprise: Option<String>,
    pub profils: Vec<&'static str>,
    pub telephone: Option<String>,
    pub telephone_secondaire: Option<String>,
    pub email: Option<String>,
    pub email_bis: Option<String>,
    pub tva: Option<String>,
    pub rue: String,
    pub numero: String,
    pub code_postal: String,
    pub ville: String,
    pub pays: &'static str,
    pub iban: Option<String>,
    pub bic: Option<String>,
}

#[derive(Default, Debug)]
struct Colonnes {
    nom: usize,
    prenom: Option<usize>,
    civilite: Option<usize>,
    entreprise: Option<usize>,
    profils: Option<usize>,
    telephones: Vec<usize>,
    emails: Vec<usize>,
    tva: Option<usize>,
    adresse: Option<usize>,
    rue: Option<usize>,
    numero: Option<usize>,
    code_postal: Option<usize>,
    ville: Option<usize>,
    pays: Option<usize>,
    iban: Option<usize>,
    bic: Option<usize>,
}

fn trouver_entete(grille: &[Vec<Cellule>], genre: Genre) -> Option<(usize, Colonnes)> {
    for (i, ligne) in grille.iter().enumerate().take(30) {
        let mut c = Colonnes::default();
        let mut nom = None;
        for (j, case) in ligne.iter().enumerate() {
            let t = cle(&case.texte());
            let premiere = |o: &mut Option<usize>| {
                if o.is_none() {
                    *o = Some(j);
                }
            };
            match t.as_str() {
                "" => {}
                "nom" | "nom de l'entreprise" | "nom de l’entreprise" | "raison sociale" | "societe" => premiere(&mut nom),
                "prenom" => premiere(&mut c.prenom),
                "civilite" => premiere(&mut c.civilite),
                "entreprise" if genre == Genre::Entreprise => premiere(&mut nom),
                "entreprise" => premiere(&mut c.entreprise),
                "rue" => premiere(&mut c.rue),
                "numero" | "n°" | "no" if c.rue.is_some() => premiere(&mut c.numero),
                "code postal" | "cp" => premiere(&mut c.code_postal),
                "ville" | "localite" | "commune" => premiere(&mut c.ville),
                "pays" => premiere(&mut c.pays),
                "iban" => premiere(&mut c.iban),
                "bic" | "bic/swift" => premiere(&mut c.bic),
                t if t.starts_with("profil") => premiere(&mut c.profils),
                t if t.starts_with("adresse") && !t.contains("factur") && !t.contains("mail") => premiere(&mut c.adresse),
                t if t.contains("mail") => c.emails.push(j),
                t if t.starts_with("tel") || t.starts_with("gsm") || t.starts_with("portable") => c.telephones.push(j),
                t if t.contains("tva") => premiere(&mut c.tva),
                _ => {}
            }
        }
        if let Some(n) = nom {
            c.nom = n;
            return Some((i, c));
        }
    }
    None
}

/// « +32 471 78 04 10 » / 471780410 (nombre) → texte propre.
fn texte_court(case: Option<&Cellule>) -> Option<String> {
    let t = case.map(Cellule::texte).unwrap_or_default();
    let t = t.trim();
    (!t.is_empty()).then(|| t.chars().take(LONGUEUR_MAX).collect())
}

fn pays(texte: &str) -> Option<&'static str> {
    let t = cle(texte);
    PAYS.iter().find(|(code, nom)| cle(nom) == t || code.to_lowercase() == t).map(|(code, _)| *code)
}

/// Découpe l'adresse en une colonne de la liste de Vertuoza : « Rue du Beau Site 12, 1390 Grez-Doiceau 🇧🇪 Belgique ».
/// Renvoie (rue, numéro, code postal, ville, pays).
pub fn decouper_adresse(texte: &str) -> (String, String, String, String, &'static str) {
    // Sans les drapeaux (lettres régionales) ni les espaces doublés.
    let propre: String = texte.chars().filter(|c| !('\u{1F1E6}'..='\u{1F1FF}').contains(c)).collect();
    let propre = propre.split_whitespace().collect::<Vec<_>>().join(" ");
    // Seulement le pays (fiche sans adresse dans Vertuoza : « 🇧🇪 Belgique »).
    if let Some(code) = pays(&propre) {
        return (String::new(), String::new(), String::new(), String::new(), code);
    }
    let (avant, apres) = match propre.rsplit_once(", ") {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None if propre.chars().next().is_some_and(|c| c.is_ascii_digit()) => (String::new(), propre.clone()),
        None => (propre.clone(), String::new()),
    };
    // Le pays à la fin de la seconde partie.
    let mut ville_et_pays = apres.trim().to_string();
    let mut code_pays = "BE";
    for (code, nom) in PAYS {
        if let Some(reste) = ville_et_pays.strip_suffix(nom) {
            ville_et_pays = reste.trim().to_string();
            code_pays = code;
            break;
        }
    }
    let (code_postal, ville) = match ville_et_pays.split_once(' ') {
        Some((cp, v)) if cp.chars().all(|c| c.is_ascii_alphanumeric()) && cp.chars().any(|c| c.is_ascii_digit()) => (cp.to_string(), v.trim().to_string()),
        _ if ville_et_pays.chars().all(|c| c.is_ascii_digit()) => (ville_et_pays.clone(), String::new()),
        _ => (String::new(), ville_et_pays.clone()),
    };
    // Le numéro à la fin de la rue (« 12 », « 15C », « 28 bte 3 » reste dans la rue).
    let avant = avant.trim().to_string();
    let (rue, numero) = match avant.rsplit_once(' ') {
        Some((r, n)) if n.chars().next().is_some_and(|c| c.is_ascii_digit()) && n.len() <= 8 => (r.trim().to_string(), n.to_string()),
        _ => (avant, String::new()),
    };
    (rue, numero, code_postal, ville, code_pays)
}

fn profils(texte: &str) -> Vec<&'static str> {
    texte
        .split([',', ';', '/', '|', '\n'])
        .filter_map(|p| {
            let p = cle(p.trim());
            PROFILS.iter().find(|(v, l)| cle(l) == p || *v == p).map(|(v, _)| *v)
        })
        .fold(Vec::new(), |mut acc, p| {
            if !acc.contains(&p) {
                acc.push(p);
            }
            acc
        })
}

fn civilite(texte: &str) -> Option<&'static str> {
    match cle(texte).as_str() {
        "m" | "monsieur" | "mr" => Some("M."),
        "mme" | "madame" => Some("Mme."),
        "" => None,
        _ => Some("Inconnu"),
    }
}

/// Ce que le fichier contient : les fiches, et les lignes écartées ou corrigées (avec leur numéro dans le fichier).
#[derive(Debug, Default)]
pub struct Lecture {
    pub fiches: Vec<Fiche>,
    pub alertes: Vec<String>,
}

pub fn lire_grille(grille: &[Vec<Cellule>], genre: Genre) -> Result<Lecture, String> {
    let (entete, c) = trouver_entete(grille, genre)
        .ok_or_else(|| "La ligne d'en-tête n'a pas été trouvée : il faut au moins une colonne « Nom ».".to_string())?;
    let mut lecture = Lecture::default();
    for (i, ligne) in grille.iter().enumerate().skip(entete + 1).take(LIGNES_MAX) {
        let case = |o: Option<usize>| o.and_then(|j| ligne.get(j));
        let Some(nom) = texte_court(case(Some(c.nom))) else { continue };
        let mut f = Fiche { nom, pays: "BE", ..Default::default() };
        if genre == Genre::Contact {
            f.prenom = texte_court(case(c.prenom));
            f.civilite = texte_court(case(c.civilite)).and_then(|t| civilite(&t));
            f.entreprise = texte_court(case(c.entreprise));
            let mut tels = c.telephones.iter().filter_map(|j| texte_court(ligne.get(*j)));
            f.telephone = tels.next();
            f.telephone_secondaire = tels.next();
        } else {
            f.telephone = c.telephones.iter().find_map(|j| texte_court(ligne.get(*j)));
        }
        f.profils = texte_court(case(c.profils)).map(|t| profils(&t)).unwrap_or_default();
        let mut emails = Vec::new();
        for j in &c.emails {
            if let Some(e) = texte_court(ligne.get(*j)) {
                if email_valide(&e) {
                    emails.push(e);
                } else {
                    lecture.alertes.push(format!("Ligne {} ({}) : l'e-mail « {e} » n'est pas valable, il n'est pas repris.", i + 1, f.nom));
                }
            }
        }
        let mut emails = emails.into_iter();
        f.email = emails.next();
        f.email_bis = emails.next();
        f.tva = texte_court(case(c.tva));
        f.iban = texte_court(case(c.iban));
        f.bic = texte_court(case(c.bic));
        if c.rue.is_some() || c.code_postal.is_some() || c.ville.is_some() {
            f.rue = texte_court(case(c.rue)).unwrap_or_default();
            f.numero = texte_court(case(c.numero)).unwrap_or_default();
            f.code_postal = texte_court(case(c.code_postal)).unwrap_or_default();
            f.ville = texte_court(case(c.ville)).unwrap_or_default();
            f.pays = texte_court(case(c.pays)).and_then(|p| pays(&p)).unwrap_or("BE");
        } else if let Some(adresse) = texte_court(case(c.adresse)) {
            (f.rue, f.numero, f.code_postal, f.ville, f.pays) = decouper_adresse(&adresse);
        }
        lecture.fiches.push(f);
    }
    if lecture.fiches.is_empty() {
        return Err("Le fichier ne contient aucune fiche sous la ligne d'en-tête.".to_string());
    }
    Ok(lecture)
}

pub fn lire_fichier(nom: &str, octets: &[u8], genre: Genre) -> Result<Lecture, String> {
    if octets.starts_with(b"PK") {
        let mut derniere = "Le classeur ne contient aucune feuille.".to_string();
        for grille in import_excel::feuilles_xlsx(octets)? {
            match lire_grille(&grille, genre) {
                Ok(l) => return Ok(l),
                Err(e) => derniere = e,
            }
        }
        return Err(derniere);
    }
    if nom.to_lowercase().ends_with(".xls") {
        return Err("Le format .xls (ancien Excel) n'est pas lu : enregistre le fichier en .xlsx ou en CSV.".to_string());
    }
    let texte = match std::str::from_utf8(octets) {
        Ok(t) => t.to_string(),
        Err(_) => octets.iter().map(|b| char::from(*b)).collect(),
    };
    lire_grille(&grille_csv(&texte), genre)
}

/// Ajoute les fiches absentes. Renvoie (ajoutées, déjà présentes).
async fn ajouter(db: &PgPool, fiches: &[Fiche], genre: Genre) -> Result<(usize, usize), sqlx::Error> {
    let mut tx = db.begin().await?;
    let (mut ajoutees, mut deja) = (0, 0);
    for f in fiches {
        let present: bool = match genre {
            Genre::Entreprise => sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM entreprises WHERE (lower(nom) = lower($1) AND lower(COALESCE(email, '')) = lower(COALESCE($2, '')))
                                                       OR ($3::text IS NOT NULL AND replace(lower(numero_tva), ' ', '') = replace(lower($3), ' ', '')))",
            )
            .bind(&f.nom)
            .bind(&f.email)
            .bind(&f.tva)
            .fetch_one(&mut *tx)
            .await?,
            Genre::Contact => sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM contacts WHERE lower(nom) = lower($1) AND lower(COALESCE(prenom, '')) = lower(COALESCE($2, ''))
                                                     AND (lower(COALESCE(email, '')) = lower(COALESCE($3, '')) OR $3::text IS NULL OR email IS NULL))",
            )
            .bind(&f.nom)
            .bind(&f.prenom)
            .bind(&f.email)
            .fetch_one(&mut *tx)
            .await?,
        };
        if present {
            deja += 1;
            continue;
        }
        let profils: Vec<String> = f.profils.iter().map(|p| (*p).to_string()).collect();
        match genre {
            Genre::Entreprise => {
                sqlx::query(
                    "INSERT INTO entreprises (nom, profils, numero_tva, rue, numero, code_postal, ville, pays, email, email_bis, telephone, iban, bic)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
                )
                .bind(&f.nom)
                .bind(&profils)
                .bind(&f.tva)
                .bind(&f.rue)
                .bind(&f.numero)
                .bind(&f.code_postal)
                .bind(&f.ville)
                .bind(f.pays)
                .bind(&f.email)
                .bind(&f.email_bis)
                .bind(&f.telephone)
                .bind(&f.iban)
                .bind(&f.bic)
                .execute(&mut *tx)
                .await?;
            }
            Genre::Contact => {
                let entreprise: Option<i64> = match &f.entreprise {
                    Some(e) => sqlx::query_scalar("SELECT id FROM entreprises WHERE lower(nom) = lower($1) ORDER BY id LIMIT 1").bind(e).fetch_optional(&mut *tx).await?,
                    None => None,
                };
                sqlx::query(
                    "INSERT INTO contacts (civilite, nom, prenom, entreprise_id, profils, numero_tva, rue, numero, code_postal, ville, pays,
                                           email, email_bis, telephone, telephone_secondaire, iban, bic)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
                )
                .bind(f.civilite)
                .bind(&f.nom)
                .bind(&f.prenom)
                .bind(entreprise)
                .bind(&profils)
                .bind(&f.tva)
                .bind(&f.rue)
                .bind(&f.numero)
                .bind(&f.code_postal)
                .bind(&f.ville)
                .bind(f.pays)
                .bind(&f.email)
                .bind(&f.email_bis)
                .bind(&f.telephone)
                .bind(&f.telephone_secondaire)
                .bind(&f.iban)
                .bind(&f.bic)
                .execute(&mut *tx)
                .await?;
            }
        }
        ajoutees += 1;
    }
    tx.commit().await?;
    Ok((ajoutees, deja))
}

// ─── Écran ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "contacts_import.html")]
struct Page {
    contact: bool,
    erreur: String,
    resultat: Option<(usize, usize)>,
    alertes: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ajoutees: String,
    deja: String,
}

fn page(genre: Genre, erreur: String, resultat: Option<(usize, usize)>, alertes: Vec<String>) -> Result<Html<String>, Erreur> {
    Ok(Html(Page { contact: genre == Genre::Contact, erreur, resultat, alertes }.render()?))
}

async fn formulaire(genre: Genre, r: Retour) -> Result<Html<String>, Erreur> {
    let resultat = match (r.ajoutees.parse(), r.deja.parse()) {
        (Ok(a), Ok(d)) => Some((a, d)),
        _ => None,
    };
    page(genre, String::new(), resultat, Vec::new())
}

fn refus(genre: Genre, message: &str) -> Result<Response, Erreur> {
    Ok((StatusCode::UNPROCESSABLE_ENTITY, page(genre, message.to_string(), None, Vec::new())?).into_response())
}

async fn recevoir(db: &PgPool, genre: Genre, mut multipart: Multipart) -> Result<Response, Erreur> {
    let mut recu: Option<(String, Vec<u8>)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(champ)) if champ.name() == Some("fichier") => {
                let nom = champ.file_name().unwrap_or("").to_string();
                match champ.bytes().await {
                    Ok(o) => recu = Some((nom, o.to_vec())),
                    Err(_) => return refus(genre, "Le fichier est trop gros (10 Mo au maximum) ou illisible."),
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => return refus(genre, "Le fichier est trop gros (10 Mo au maximum) ou illisible."),
        }
    }
    let Some((nom, octets)) = recu.filter(|(_, o)| !o.is_empty()) else {
        return refus(genre, "Choisis un fichier avant de cliquer sur « Importer ».");
    };
    let lecture = match tokio::task::spawn_blocking(move || lire_fichier(&nom, &octets, genre)).await {
        Ok(Ok(l)) => l,
        Ok(Err(m)) => return refus(genre, &m),
        Err(_) => return refus(genre, "Le fichier n'a pas pu être lu."),
    };
    let (ajoutees, deja) = ajouter(db, &lecture.fiches, genre).await?;
    let base = if genre == Genre::Contact { "/contacts" } else { "/entreprises" };
    if !lecture.alertes.is_empty() {
        return Ok(page(genre, String::new(), Some((ajoutees, deja)), lecture.alertes)?.into_response());
    }
    Ok(Redirect::to(&format!("{base}/import?ajoutees={ajoutees}&deja={deja}")).into_response())
}

pub fn routes() -> Router<AppState> {
    let limite = DefaultBodyLimit::max(TAILLE_MAX + 64 * 1024);
    Router::new()
        .route(
            "/entreprises/import",
            get(|Query(r): Query<Retour>| formulaire(Genre::Entreprise, r))
                .post(|State(s): State<AppState>, m: Multipart| async move { recevoir(&s.db, Genre::Entreprise, m).await })
                .layer(limite.clone()),
        )
        .route(
            "/contacts/import",
            get(|Query(r): Query<Retour>| formulaire(Genre::Contact, r))
                .post(|State(s): State<AppState>, m: Multipart| async move { recevoir(&s.db, Genre::Contact, m).await })
                .layer(limite),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_adresse_de_vertuoza_en_une_colonne() {
        assert_eq!(
            decouper_adresse("Rue du Beau Site 12, 1390 Grez-Doiceau 🇧🇪 Belgique"),
            ("Rue du Beau Site".into(), "12".into(), "1390".into(), "Grez-Doiceau".into(), "BE")
        );
        assert_eq!(decouper_adresse("Rue de l'Aisne 15C, 6997 fanzel 🇧🇪 Belgique").1, "15C");
        assert_eq!(decouper_adresse("1170 Watermael boitsfort").2, "1170");
        assert_eq!(decouper_adresse("🇧🇪 Belgique"), (String::new(), String::new(), String::new(), String::new(), "BE"));
        assert_eq!(decouper_adresse("Rue de Rivoli 5, 75001 Paris France").4, "FR");
    }

    #[test]
    fn une_liste_de_contacts_de_vertuoza() {
        let csv = "Nom;Entreprise;Profils;Téléphone;Téléphone secondaire;E-mail;Adresse\n\
                   Thierry Migeal;;Client;0477446832;;mgl.maison@exemple.be;Rue du beau site 12, 1390 Grez-doiceau 🇧🇪 Belgique\n\
                   Pauline Delpire;Delpire SRL;Architecte;;;pauline@exemple.be;🇧🇪 Belgique\n\
                   Sans mail;;Client, Prospect;;;pas-un-mail;\n";
        let l = lire_grille(&grille_csv(csv), Genre::Contact).unwrap();
        assert_eq!(l.fiches.len(), 3);
        let t = &l.fiches[0];
        assert_eq!((t.nom.as_str(), t.telephone.as_deref(), t.email.as_deref(), t.profils.clone()), ("Thierry Migeal", Some("0477446832"), Some("mgl.maison@exemple.be"), vec!["client"]));
        assert_eq!((t.rue.as_str(), t.numero.as_str(), t.code_postal.as_str(), t.ville.as_str()), ("Rue du beau site", "12", "1390", "Grez-doiceau"));
        assert_eq!((l.fiches[1].entreprise.as_deref(), l.fiches[1].profils.clone()), (Some("Delpire SRL"), vec!["architecte"]));
        assert_eq!((l.fiches[2].email.clone(), l.fiches[2].profils.clone()), (None, vec!["client", "prospect"]));
        assert_eq!(l.alertes.len(), 1, "l'e-mail faux est signalé");
    }

    #[test]
    fn des_entreprises_en_colonnes() {
        let csv = "Nom de l'entreprise,N° TVA,Rue,Numéro,Code postal,Ville,Pays,E-mail\nCEP asbl,BE0123456789,Rue X,5,1170,Bruxelles,Belgique,info@cep.be\n";
        let l = lire_grille(&grille_csv(csv), Genre::Entreprise).unwrap();
        let e = &l.fiches[0];
        assert_eq!((e.nom.as_str(), e.tva.as_deref(), e.numero.as_str(), e.ville.as_str(), e.pays), ("CEP asbl", Some("BE0123456789"), "5", "Bruxelles", "BE"));
        assert!(lire_grille(&grille_csv("a;b\n1;2\n"), Genre::Contact).is_err());
    }
}
