//! Entreprises et contacts : liste avec recherche, création, modification.
//! Les deux écrans fonctionnent pareil ; `Genre` dit duquel on parle.

use askama::Template;
use axum::{
    Router,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::{FromRow, PgConnection, Postgres, postgres::PgArguments, query::Query as SqlQuery};

use crate::base::Base;
use crate::{AppState, Erreur};
use crate::commun::{Choix, PAYS, choix_fixes, echapper_like, email_valide, libelle, vide_en_none};

const PAR_PAGE: i64 = 20;

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
const SOURCES: [(&str, &str); 7] = [
    ("apporteur_affaires", "Apporteur d'affaires"),
    ("client", "Client"),
    ("marketing", "Marketing"),
    ("prospection", "Prospection"),
    ("recommandation_client", "Recommandation client"),
    ("reseaux_sociaux", "Réseaux sociaux"),
    ("site_web", "Site web"),
];
const LANGUES: [(&str, &str); 4] = [
    ("fr", "Français"),
    ("en", "Anglais"),
    ("nl_be", "Néerlandais (Belgique)"),
    ("nl", "Néerlandais"),
];
const CIVILITES: [(&str, &str); 3] = [("M.", "M."), ("Mme.", "Mme."), ("Inconnu", "Inconnu")];

// ─── Entreprise ou contact ──────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Genre {
    Entreprise,
    Contact,
}

impl Genre {
    fn table(self) -> &'static str {
        match self {
            Genre::Entreprise => "entreprises",
            Genre::Contact => "contacts",
        }
    }
    fn base(self) -> &'static str {
        match self {
            Genre::Entreprise => "/entreprises",
            Genre::Contact => "/contacts",
        }
    }
    fn est_contact(self) -> bool {
        self == Genre::Contact
    }
}

// ─── Petits outils ──────────────────────────────────────────────────────────

/// « 1 150,50 » ou « 1150.5 » → 115050 centimes. `None` si le texte n'est pas un montant.
pub fn euros_vers_centimes(texte: &str) -> Option<i64> {
    let t: String = texte.chars().filter(|c| !c.is_whitespace()).collect();
    let t = t.replace(',', ".");
    let (entier, decimales) = t.split_once('.').unwrap_or((t.as_str(), ""));
    if entier.is_empty() && decimales.is_empty() {
        return None;
    }
    let chiffres = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if !chiffres(entier) || !chiffres(decimales) || decimales.len() > 2 {
        return None;
    }
    let e: i64 = if entier.is_empty() { 0 } else { entier.parse().ok()? };
    let d: i64 = match decimales.len() {
        0 => 0,
        1 => decimales.parse::<i64>().ok()? * 10,
        _ => decimales.parse().ok()?,
    };
    e.checked_mul(100)?.checked_add(d)
}

fn centimes_vers_texte(centimes: i64) -> String {
    format!("{},{:02}", centimes / 100, centimes % 100)
}

fn nombre(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}

// ─── Listes de choix pour les menus déroulants ─────────────────────────────

pub(crate) fn choix_base(liste: &[(i64, String)], actuel: &str) -> Vec<Choix> {
    liste
        .iter()
        .map(|(id, l)| Choix {
            valeur: id.to_string(),
            libelle: l.clone(),
            choisi: id.to_string() == actuel,
        })
        .collect()
}

/// Libellé court d'un taux pour les petits menus des lignes : « Taux réduit (6,00 %) » → « 6 % »,
/// « TVA acquittée par autoliquidation (0,00 %) » → « 0 % autoliq. ». Seul l'affichage change.
pub(crate) fn taux_court(libelle: &str) -> String {
    let taux = libelle
        .rsplit_once('(')
        .map(|(_, reste)| reste.trim_end_matches(')').replace(",00", "").trim().to_string())
        .unwrap_or_else(|| libelle.to_string());
    let libelle = libelle.to_lowercase();
    if libelle.contains("autoliquidation") {
        format!("{taux} autoliq.")
    } else if libelle.contains("franchise") {
        "Franchise".to_string()
    } else {
        taux
    }
}

/// Libellés courts pour toute la liste des taux.
pub(crate) fn taux_courts(taux: &[(i64, String)]) -> Vec<(i64, String)> {
    taux.iter().map(|(id, l)| (*id, taux_court(l))).collect()
}

/// Les listes qui viennent de la base : conditions de paiement, taux de TVA, entreprises.
pub(crate) struct Options {
    pub(crate) conditions: Vec<(i64, String)>,
    pub(crate) taux_tva: Vec<(i64, String)>,
    entreprises: Vec<(i64, String)>,
}

impl Options {
    pub(crate) async fn charger(db: &mut PgConnection, genre: Genre) -> Result<Self, sqlx::Error> {
        let conditions: Vec<(i64, String, i32)> = sqlx::query_as(
            "SELECT id, \"type\"::text, jours FROM conditions_paiement
             ORDER BY CASE \"type\"::text WHEN 'apres_facturation' THEN 1 WHEN 'fin_de_mois' THEN 2 ELSE 3 END, jours",
        )
        .fetch_all(&mut *db)
        .await?;
        let conditions = conditions
            .into_iter()
            .map(|(id, genre_cond, jours)| {
                let texte = match genre_cond.as_str() {
                    "apres_facturation" => format!("{} après date de facturation", crate::commun::nombre(jours, "jour", "jours")),
                    "fin_de_mois" => format!("{} fin du mois", crate::commun::nombre(jours, "jour", "jours")),
                    _ => "Paiement comptant".to_string(),
                };
                (id, texte)
            })
            .collect();

        let taux: Vec<(i64, String, i32)> =
            // Une société en franchise n'a que le taux « Franchise de TVA » ; les autres ne l'ont jamais.
            sqlx::query_as("SELECT id, libelle, taux_centiemes FROM taux_tva WHERE actif AND franchise = $1 ORDER BY taux_centiemes")
                .bind(crate::societe::en_franchise(&mut *db).await?)
                .fetch_all(&mut *db)
                .await?;
        let taux_tva = taux
            .into_iter()
            .map(|(id, l, c)| (id, format!("{l} ({},{:02} %)", c / 100, c % 100)))
            .collect();

        let entreprises = if genre.est_contact() {
            sqlx::query_as("SELECT id, nom FROM entreprises ORDER BY lower(nom), id")
                .fetch_all(&mut *db)
                .await?
        } else {
            Vec::new()
        };

        Ok(Options { conditions, taux_tva, entreprises })
    }
}

// ─── La fiche telle qu'elle est en base, et telle qu'elle est dans le formulaire ──

#[derive(FromRow)]
struct Fiche {
    nom: String,
    profils: Vec<String>,
    numero_tva: Option<String>,
    peppol_id: Option<String>,
    identifiant_comptable: Option<String>,
    franco_cents: Option<i64>,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    facturation_identique: bool,
    facturation_rue: Option<String>,
    facturation_numero: Option<String>,
    facturation_code_postal: Option<String>,
    facturation_ville: Option<String>,
    facturation_pays: Option<String>,
    email: Option<String>,
    email_bis: Option<String>,
    telephone: Option<String>,
    site_web: Option<String>,
    iban: Option<String>,
    bic: Option<String>,
    conditions_paiement_id: Option<i64>,
    taux_tva_id: Option<i64>,
    source: Option<String>,
    langue: String,
    description: Option<String>,
    // Seulement pour un contact (vides pour une entreprise).
    civilite: Option<String>,
    prenom: Option<String>,
    entreprise_id: Option<i64>,
    telephone_secondaire: Option<String>,
}

/// Ce que le navigateur envoie : tout en texte. Une case cochée envoie une valeur, une case
/// décochée n'envoie rien.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct FicheForm {
    nom: String,
    prenom: String,
    civilite: String,
    entreprise_id: String,
    profils: Vec<String>,
    numero_tva: String,
    peppol_id: String,
    identifiant_comptable: String,
    franco: String,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    facturation_identique: Option<String>,
    facturation_rue: String,
    facturation_numero: String,
    facturation_code_postal: String,
    facturation_ville: String,
    facturation_pays: String,
    email: String,
    email_bis: String,
    telephone: String,
    telephone_secondaire: String,
    site_web: String,
    iban: String,
    bic: String,
    conditions_paiement_id: String,
    taux_tva_id: String,
    source: String,
    langue: String,
    description: String,
}

impl FicheForm {
    fn vierge() -> Self {
        FicheForm {
            pays: "BE".into(),
            langue: "fr".into(),
            facturation_identique: Some("on".into()),
            ..Default::default()
        }
    }
}

impl From<Fiche> for FicheForm {
    fn from(f: Fiche) -> Self {
        let t = |o: Option<String>| o.unwrap_or_default();
        let id = |o: Option<i64>| o.map(|n| n.to_string()).unwrap_or_default();
        FicheForm {
            nom: f.nom,
            prenom: t(f.prenom),
            civilite: t(f.civilite),
            entreprise_id: id(f.entreprise_id),
            profils: f.profils,
            numero_tva: t(f.numero_tva),
            peppol_id: t(f.peppol_id),
            identifiant_comptable: t(f.identifiant_comptable),
            franco: f.franco_cents.map(centimes_vers_texte).unwrap_or_default(),
            rue: f.rue,
            numero: f.numero,
            code_postal: f.code_postal,
            ville: f.ville,
            pays: f.pays,
            facturation_identique: f.facturation_identique.then(|| "on".to_string()),
            facturation_rue: t(f.facturation_rue),
            facturation_numero: t(f.facturation_numero),
            facturation_code_postal: t(f.facturation_code_postal),
            facturation_ville: t(f.facturation_ville),
            facturation_pays: t(f.facturation_pays),
            email: t(f.email),
            email_bis: t(f.email_bis),
            telephone: t(f.telephone),
            telephone_secondaire: t(f.telephone_secondaire),
            site_web: t(f.site_web),
            iban: t(f.iban),
            bic: t(f.bic),
            conditions_paiement_id: id(f.conditions_paiement_id),
            taux_tva_id: id(f.taux_tva_id),
            source: t(f.source),
            langue: f.langue,
            description: t(f.description),
        }
    }
}

/// La fiche vérifiée et nettoyée, prête à être enregistrée.
struct Valeurs {
    nom: String,
    profils: Vec<String>,
    numero_tva: Option<String>,
    peppol_id: Option<String>,
    identifiant_comptable: Option<String>,
    franco_cents: Option<i64>,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    facturation_identique: bool,
    facturation_rue: Option<String>,
    facturation_numero: Option<String>,
    facturation_code_postal: Option<String>,
    facturation_ville: Option<String>,
    facturation_pays: Option<String>,
    email: Option<String>,
    email_bis: Option<String>,
    telephone: Option<String>,
    site_web: Option<String>,
    iban: Option<String>,
    bic: Option<String>,
    conditions_paiement_id: Option<i64>,
    taux_tva_id: Option<i64>,
    source: Option<String>,
    langue: String,
    description: Option<String>,
    civilite: Option<String>,
    prenom: Option<String>,
    entreprise_id: Option<i64>,
    telephone_secondaire: Option<String>,
}

/// Les champs refusés d'une saisie, chacun avec son explication ; vide : tout est valable. Une seule liste pour la
/// création (tout ou rien) et pour l'enregistrement automatique d'une fiche existante (`retenir`).
fn refus(f: &FicheForm, genre: Genre, options: &Options) -> Vec<(&'static str, String)> {
    let mut r = Vec::new();
    if f.nom.trim().is_empty() {
        r.push(("nom", "Le nom est obligatoire.".to_string()));
    }
    for (texte, champ, nom) in [(&f.email, "email", "L'e-mail"), (&f.email_bis, "email_bis", "L'e-mail (bis)")] {
        let t = texte.trim();
        if !t.is_empty() && !email_valide(t) {
            r.push((champ, format!("{nom} n'est pas valide.")));
        }
    }
    if vide_en_none(&f.franco).is_some_and(|t| euros_vers_centimes(&t).is_none()) {
        r.push(("franco", "Le franco doit être un montant en euros (exemple : 1 150,00).".to_string()));
    }
    if f.profils.iter().filter(|p| !p.is_empty()).any(|p| !PROFILS.iter().any(|(v, _)| v == p)) {
        r.push(("profils", "Un des profils choisis n'existe pas.".to_string()));
    }
    if nombre(&f.conditions_paiement_id).is_some_and(|id| !options.conditions.iter().any(|(i, _)| *i == id)) {
        r.push(("conditions_paiement_id", "Cette condition de paiement n'existe pas.".to_string()));
    }
    if nombre(&f.taux_tva_id).is_some_and(|id| !options.taux_tva.iter().any(|(i, _)| *i == id)) {
        r.push(("taux_tva_id", "Ce taux de TVA n'existe pas.".to_string()));
    }
    if genre.est_contact() && nombre(&f.entreprise_id).is_some_and(|id| !options.entreprises.iter().any(|(i, _)| *i == id)) {
        r.push(("entreprise_id", "Cette entreprise n'existe pas.".to_string()));
    }
    if vide_en_none(&f.source).is_some_and(|s| !SOURCES.iter().any(|(v, _)| *v == s)) {
        r.push(("source", "Cette source n'existe pas.".to_string()));
    }
    if let Some(texte) = vide_en_none(&f.peppol_id)
        && let Err(e) = crate::peppol::lire(&texte)
    {
        r.push(("peppol_id", e));
    }
    if !LANGUES.iter().any(|(v, _)| *v == f.langue) {
        r.push(("langue", "Cette langue n'existe pas.".to_string()));
    }
    if vide_en_none(&f.civilite).is_some_and(|c| !CIVILITES.iter().any(|(v, _)| *v == c)) {
        r.push(("civilite", "Cette civilité n'existe pas.".to_string()));
    }
    for (pays, champ) in [(&f.pays, "pays"), (&f.facturation_pays, "facturation_pays")] {
        if !pays.is_empty() && !PAYS.iter().any(|(v, _)| v == pays) {
            r.push((champ, "Ce pays n'existe pas.".to_string()));
        }
    }
    r
}

/// Les explications des champs refusés, sans répéter deux fois la même.
fn messages(refus: Vec<(&'static str, String)>) -> Vec<String> {
    let mut m: Vec<String> = Vec::new();
    for (_, texte) in refus {
        if !m.contains(&texte) {
            m.push(texte);
        }
    }
    m
}

/// L'enregistrement automatique d'une fiche existante (audit du design du 23/10/2026, « rien ne se perd ») : chaque
/// valeur valable est gardée, une valeur refusée garde la précédente (`avant`) et son explication est rendue.
fn retenir(saisie: &FicheForm, avant: &FicheForm, genre: Genre, options: &Options) -> (FicheForm, Vec<String>) {
    let refuses = refus(saisie, genre, options);
    let mut r = saisie.clone();
    for (champ, _) in &refuses {
        match *champ {
            "nom" => r.nom = avant.nom.clone(),
            "email" => r.email = avant.email.clone(),
            "email_bis" => r.email_bis = avant.email_bis.clone(),
            "franco" => r.franco = avant.franco.clone(),
            "profils" => r.profils = avant.profils.clone(),
            "conditions_paiement_id" => r.conditions_paiement_id = avant.conditions_paiement_id.clone(),
            "taux_tva_id" => r.taux_tva_id = avant.taux_tva_id.clone(),
            "entreprise_id" => r.entreprise_id = avant.entreprise_id.clone(),
            "source" => r.source = avant.source.clone(),
            "peppol_id" => r.peppol_id = avant.peppol_id.clone(),
            "langue" => r.langue = avant.langue.clone(),
            "civilite" => r.civilite = avant.civilite.clone(),
            "pays" => r.pays = avant.pays.clone(),
            "facturation_pays" => r.facturation_pays = avant.facturation_pays.clone(),
            _ => {}
        }
    }
    (r, messages(refuses))
}

fn valider(f: &FicheForm, genre: Genre, options: &Options) -> Result<Valeurs, Vec<String>> {
    let refuses = refus(f, genre, options);
    if !refuses.is_empty() {
        return Err(messages(refuses));
    }
    let nom = f.nom.trim().to_string();
    let franco_cents = vide_en_none(&f.franco).and_then(|t| euros_vers_centimes(&t));
    let profils: Vec<String> = f.profils.iter().filter(|p| !p.is_empty()).cloned().collect();
    let conditions_paiement_id = nombre(&f.conditions_paiement_id);
    let taux_tva_id = nombre(&f.taux_tva_id);
    let entreprise_id = nombre(&f.entreprise_id);
    let source = vide_en_none(&f.source);
    let peppol_id = vide_en_none(&f.peppol_id).and_then(|t| crate::peppol::lire(&t).ok()).map(|i| i.to_string());
    let civilite = vide_en_none(&f.civilite);

    let identique = f.facturation_identique.is_some();
    // Si l'adresse de facturation est la même, on n'enregistre pas de seconde adresse.
    let facturation = |s: &str| if identique { None } else { vide_en_none(s) };
    let pays = if f.pays.is_empty() { "BE".to_string() } else { f.pays.clone() };

    Ok(Valeurs {
        nom,
        profils,
        numero_tva: vide_en_none(&f.numero_tva),
        peppol_id,
        identifiant_comptable: vide_en_none(&f.identifiant_comptable),
        franco_cents,
        rue: f.rue.trim().to_string(),
        numero: f.numero.trim().to_string(),
        code_postal: f.code_postal.trim().to_string(),
        ville: f.ville.trim().to_string(),
        pays,
        facturation_identique: identique,
        facturation_rue: facturation(&f.facturation_rue),
        facturation_numero: facturation(&f.facturation_numero),
        facturation_code_postal: facturation(&f.facturation_code_postal),
        facturation_ville: facturation(&f.facturation_ville),
        facturation_pays: facturation(&f.facturation_pays),
        email: vide_en_none(&f.email),
        email_bis: vide_en_none(&f.email_bis),
        telephone: vide_en_none(&f.telephone),
        site_web: vide_en_none(&f.site_web),
        iban: vide_en_none(&f.iban),
        bic: vide_en_none(&f.bic),
        conditions_paiement_id,
        taux_tva_id,
        source,
        langue: f.langue.clone(),
        description: vide_en_none(&f.description),
        civilite,
        prenom: vide_en_none(&f.prenom),
        entreprise_id: if genre.est_contact() { entreprise_id } else { None },
        telephone_secondaire: vide_en_none(&f.telephone_secondaire),
    })
}

// ─── Enregistrement ─────────────────────────────────────────────────────────

const COLONNES: [&str; 28] = [
    "nom", "profils", "numero_tva", "peppol_id", "identifiant_comptable", "franco_cents", "rue", "numero", "code_postal",
    "ville", "pays", "facturation_identique", "facturation_rue", "facturation_numero",
    "facturation_code_postal", "facturation_ville", "facturation_pays", "email", "email_bis", "telephone",
    "site_web", "iban", "bic", "conditions_paiement_id", "taux_tva_id", "source", "langue", "description",
];
const COLONNES_CONTACT: [&str; 4] = ["civilite", "prenom", "entreprise_id", "telephone_secondaire"];

fn colonnes(genre: Genre) -> Vec<&'static str> {
    let mut c = COLONNES.to_vec();
    if genre.est_contact() {
        c.extend(COLONNES_CONTACT);
    }
    c
}

/// Associe chaque valeur à sa colonne, dans le même ordre que `colonnes`.
fn lier<'q>(
    q: SqlQuery<'q, Postgres, PgArguments>,
    v: &Valeurs,
    genre: Genre,
) -> SqlQuery<'q, Postgres, PgArguments> {
    let q = q
        .bind(v.nom.clone())
        .bind(v.profils.clone())
        .bind(v.numero_tva.clone())
        .bind(v.peppol_id.clone())
        .bind(v.identifiant_comptable.clone())
        .bind(v.franco_cents)
        .bind(v.rue.clone())
        .bind(v.numero.clone())
        .bind(v.code_postal.clone())
        .bind(v.ville.clone())
        .bind(v.pays.clone())
        .bind(v.facturation_identique)
        .bind(v.facturation_rue.clone())
        .bind(v.facturation_numero.clone())
        .bind(v.facturation_code_postal.clone())
        .bind(v.facturation_ville.clone())
        .bind(v.facturation_pays.clone())
        .bind(v.email.clone())
        .bind(v.email_bis.clone())
        .bind(v.telephone.clone())
        .bind(v.site_web.clone())
        .bind(v.iban.clone())
        .bind(v.bic.clone())
        .bind(v.conditions_paiement_id)
        .bind(v.taux_tva_id)
        .bind(v.source.clone())
        .bind(v.langue.clone())
        .bind(v.description.clone());
    if genre.est_contact() {
        q.bind(v.civilite.clone())
            .bind(v.prenom.clone())
            .bind(v.entreprise_id)
            .bind(v.telephone_secondaire.clone())
    } else {
        q
    }
}

async fn inserer(db: &mut PgConnection, genre: Genre, v: &Valeurs) -> Result<(), sqlx::Error> {
    let cols = colonnes(genre);
    let marques: Vec<String> = (1..=cols.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        genre.table(),
        cols.join(", "),
        marques.join(", ")
    );
    lier(sqlx::query(sqlx::AssertSqlSafe(sql)), v, genre).execute(&mut *db).await?;
    Ok(())
}

/// Renvoie `false` si la fiche n'existe pas.
async fn modifier_en_base(db: &mut PgConnection, genre: Genre, id: i64, v: &Valeurs) -> Result<bool, sqlx::Error> {
    let cols = colonnes(genre);
    let affectations: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c} = ${}", i + 1)).collect();
    let sql = format!(
        "UPDATE {} SET {}, updated_at = now() WHERE id = ${}",
        genre.table(),
        affectations.join(", "),
        cols.len() + 1
    );
    let res = lier(sqlx::query(sqlx::AssertSqlSafe(sql)), v, genre).bind(id).execute(&mut *db).await?;
    Ok(res.rows_affected() > 0)
}

async fn charger_fiche(db: &mut PgConnection, genre: Genre, id: i64) -> Result<Option<Fiche>, sqlx::Error> {
    let communes = COLONNES.join(", ");
    let sql = match genre {
        Genre::Entreprise => format!(
            "SELECT {communes}, NULL::text AS civilite, NULL::text AS prenom, NULL::bigint AS entreprise_id,
                    NULL::text AS telephone_secondaire FROM entreprises WHERE id = $1"
        ),
        Genre::Contact => format!(
            "SELECT {communes}, civilite, prenom, entreprise_id, telephone_secondaire FROM contacts WHERE id = $1"
        ),
    };
    sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(&mut *db).await
}

// ─── Liste avec recherche ───────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Recherche {
    q: String,
    profil: String,
    page: String,
}

#[derive(FromRow)]
struct LigneBase {
    id: i64,
    nom: String,
    entreprise: Option<String>,
    profils: Vec<String>,
    telephone: Option<String>,
    telephone_secondaire: Option<String>,
    numero_tva: Option<String>,
    email: Option<String>,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
}

pub struct Ligne {
    pub id: i64,
    pub nom: String,
    pub entreprise: String,
    pub profils: Vec<String>,
    pub telephone: String,
    pub telephone_secondaire: String,
    pub numero_tva: String,
    pub email: String,
    pub adresse: String,
}

impl From<LigneBase> for Ligne {
    fn from(l: LigneBase) -> Self {
        let rue = format!("{} {}", l.rue, l.numero).trim().to_string();
        let ville = format!("{} {}", l.code_postal, l.ville).trim().to_string();
        let mut morceaux: Vec<String> = [rue, ville].into_iter().filter(|s| !s.is_empty()).collect();
        if !morceaux.is_empty() {
            morceaux.push(libelle(&PAYS, &l.pays));
        }
        Ligne {
            id: l.id,
            nom: l.nom,
            entreprise: l.entreprise.unwrap_or_default(),
            profils: l.profils.iter().map(|p| libelle(&PROFILS, p)).collect(),
            telephone: l.telephone.unwrap_or_default(),
            telephone_secondaire: l.telephone_secondaire.unwrap_or_default(),
            numero_tva: l.numero_tva.unwrap_or_default(),
            email: l.email.unwrap_or_default(),
            adresse: morceaux.join(", "),
        }
    }
}

pub struct Liste {
    pub est_contact: bool,
    pub base: String,
    pub titre: String,
    pub bouton_nouveau: String,
    pub lignes: Vec<Ligne>,
    pub q: String,
    pub profil: String,
    pub profils: Vec<Choix>,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
}

#[derive(Template)]
#[template(path = "liste.html")]
struct PageListe {
    liste: Liste,
}

/// Seulement le tableau : c'est ce que htmx remplace quand on tape dans la recherche.
#[derive(Template)]
#[template(path = "_tableau.html")]
struct TableauListe {
    liste: Liste,
}

async fn liste(
    genre: Genre,
    mut db: Base,
    recherche: Recherche,
    entetes: HeaderMap,
) -> Result<Html<String>, Erreur> {
    let q = recherche.q.trim().to_string();
    let profil = if PROFILS.iter().any(|(v, _)| *v == recherche.profil) { recherche.profil.clone() } else { String::new() };
    let page: i64 = recherche.page.trim().parse().unwrap_or(1).max(1);
    let motif = format!("%{}%", echapper_like(&q));

    let (select, depuis) = match genre {
        Genre::Entreprise => (
            "SELECT id, nom, NULL::text AS entreprise, profils, telephone, NULL::text AS telephone_secondaire,
                    numero_tva, email, rue, numero, code_postal, ville, pays",
            "FROM entreprises
             WHERE ($1 = '' OR nom ILIKE $2 OR email ILIKE $2 OR telephone ILIKE $2 OR numero_tva ILIKE $2)
               AND ($3 = '' OR $3 = ANY(profils))",
        ),
        Genre::Contact => (
            "SELECT c.id, trim(concat_ws(' ', c.prenom, c.nom)) AS nom, e.nom AS entreprise, c.profils,
                    c.telephone, c.telephone_secondaire, c.numero_tva, c.email, c.rue, c.numero,
                    c.code_postal, c.ville, c.pays",
            "FROM contacts c LEFT JOIN entreprises e ON e.id = c.entreprise_id
             WHERE ($1 = '' OR concat_ws(' ', c.prenom, c.nom) ILIKE $2 OR c.email ILIKE $2
                    OR c.telephone ILIKE $2 OR c.telephone_secondaire ILIKE $2 OR e.nom ILIKE $2)
               AND ($3 = '' OR $3 = ANY(c.profils))",
        ),
    };
    let ordre = if genre.est_contact() { "lower(c.nom), c.id" } else { "lower(nom), id" };

    let (total,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT count(*) {depuis}")))
        .bind(&q)
        .bind(&motif)
        .bind(&profil)
        .fetch_one(&mut *db)
        .await?;
    let pages = ((total + PAR_PAGE - 1) / PAR_PAGE).max(1);
    let page = page.min(pages);

    let lignes: Vec<LigneBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{select} {depuis} ORDER BY {ordre} LIMIT $4 OFFSET $5"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(&profil)
    .bind(PAR_PAGE)
    .bind((page - 1) * PAR_PAGE)
    .fetch_all(&mut *db)
    .await?;

    let liste = Liste {
        est_contact: genre.est_contact(),
        base: genre.base().to_string(),
        titre: if genre.est_contact() { "Contacts" } else { "Entreprises" }.to_string(),
        bouton_nouveau: if genre.est_contact() { "Nouveau contact" } else { "Nouvelle entreprise" }.to_string(),
        lignes: lignes.into_iter().map(Ligne::from).collect(),
        q,
        profils: choix_fixes(&PROFILS, std::slice::from_ref(&profil)),
        profil,
        page,
        pages,
        total,
    };

    // htmx demande uniquement le tableau ; un rechargement de la page demande tout.
    let partiel = entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request");
    let html = if partiel { TableauListe { liste }.render()? } else { PageListe { liste }.render()? };
    Ok(Html(html))
}

// ─── Formulaire ─────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "formulaire.html")]
struct PageFormulaire {
    est_contact: bool,
    /// Une fiche existante : elle s'enregistre toute seule ; une nouvelle fiche se crée par son bouton.
    existante: bool,
    enregistrement: crate::composants::Enregistrement,
    titre: String,
    action: String,
    retour: String,
    form: FicheForm,
    erreurs: Vec<String>,
    profils: Vec<Choix>,
    conditions: Vec<Choix>,
    taux_tva: Vec<Choix>,
    sources: Vec<Choix>,
    langues: Vec<Choix>,
    civilites: Vec<Choix>,
    pays: Vec<Choix>,
    pays_facturation: Vec<Choix>,
    entreprises: Vec<Choix>,
}

fn page_formulaire(
    genre: Genre,
    titre: String,
    action: String,
    form: FicheForm,
    erreurs: Vec<String>,
    options: &Options,
) -> PageFormulaire {
    let facturation_pays = if form.facturation_pays.is_empty() { "BE".to_string() } else { form.facturation_pays.clone() };
    PageFormulaire {
        est_contact: genre.est_contact(),
        existante: action != genre.base(),
        enregistrement: crate::composants::Enregistrement::default(),
        titre,
        action,
        retour: genre.base().to_string(),
        erreurs,
        profils: choix_fixes(&PROFILS, &form.profils),
        conditions: choix_base(&options.conditions, &form.conditions_paiement_id),
        taux_tva: choix_base(&options.taux_tva, &form.taux_tva_id),
        sources: choix_fixes(&SOURCES, std::slice::from_ref(&form.source)),
        langues: choix_fixes(&LANGUES, std::slice::from_ref(&form.langue)),
        civilites: choix_fixes(&CIVILITES, std::slice::from_ref(&form.civilite)),
        pays: choix_fixes(&PAYS, std::slice::from_ref(&form.pays)),
        pays_facturation: choix_fixes(&PAYS, &[facturation_pays]),
        entreprises: choix_base(&options.entreprises, &form.entreprise_id),
        form,
    }
}

fn titre_nouveau(genre: Genre) -> &'static str {
    if genre.est_contact() { "Nouveau contact" } else { "Nouvelle entreprise" }
}

async fn nouveau(genre: Genre, mut db: Base) -> Result<Html<String>, Erreur> {
    let options = Options::charger(&mut *db, genre).await?;
    let page = page_formulaire(
        genre,
        titre_nouveau(genre).to_string(),
        genre.base().to_string(),
        FicheForm::vierge(),
        Vec::new(),
        &options,
    );
    Ok(Html(page.render()?))
}

async fn creer(genre: Genre, mut db: Base, form: FicheForm) -> Result<Response, Erreur> {
    let options = Options::charger(&mut *db, genre).await?;
    match valider(&form, genre, &options) {
        Ok(valeurs) => {
            inserer(&mut *db, genre, &valeurs).await?;
            Ok(Redirect::to(genre.base()).into_response())
        }
        Err(erreurs) => {
            let page = page_formulaire(
                genre,
                titre_nouveau(genre).to_string(),
                genre.base().to_string(),
                form,
                erreurs,
                &options,
            );
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
        }
    }
}

/// Le titre d'une fiche existante : son nom (prénom compris pour un contact).
fn titre_fiche(genre: Genre, f: &FicheForm) -> String {
    let nom = if genre.est_contact() { format!("{} {}", f.prenom.trim(), f.nom.trim()) } else { f.nom.trim().to_string() };
    let nom = nom.trim();
    if nom.is_empty() { (if genre.est_contact() { "Contact" } else { "Entreprise" }).to_string() } else { nom.to_string() }
}

async fn page_modifier(genre: Genre, mut db: Base, id: i64) -> Result<Html<String>, Erreur> {
    let fiche: FicheForm = charger_fiche(&mut *db, genre, id).await?.ok_or_else(Erreur::introuvable)?.into();
    let options = Options::charger(&mut *db, genre).await?;
    let page = page_formulaire(genre, titre_fiche(genre, &fiche), format!("{}/{id}", genre.base()), fiche, Vec::new(), &options);
    Ok(Html(page.render()?))
}

/// Enregistre une fiche existante. Le formulaire s'envoie tout seul (htmx) à chaque changement : chaque valeur valable
/// est gardée, une valeur refusée garde la précédente avec son explication ; htmx ne reprend de la réponse que la note
/// de la barre du bas et le titre. Sans htmx (formulaire envoyé en entier), même règle, puis retour à la liste ou 422.
async fn enregistrer_modification(genre: Genre, mut db: Base, id: i64, entetes: HeaderMap, form: FicheForm) -> Result<Response, Erreur> {
    let avant: FicheForm = charger_fiche(&mut *db, genre, id).await?.ok_or_else(Erreur::introuvable)?.into();
    let options = Options::charger(&mut *db, genre).await?;
    let (retenue, mut erreurs) = retenir(&form, &avant, genre, &options);
    match valider(&retenue, genre, &options) {
        Ok(valeurs) => {
            modifier_en_base(&mut *db, genre, id, &valeurs).await?;
        }
        // Ne devrait pas arriver (chaque valeur refusée a repris la précédente) : rien n'est enregistré.
        Err(autres) => erreurs.extend(autres.into_iter().filter(|e| !erreurs.contains(e)).collect::<Vec<_>>()),
    }
    if crate::composants::par_htmx(&entetes) {
        let mut page = page_formulaire(genre, titre_fiche(genre, &retenue), format!("{}/{id}", genre.base()), form, Vec::new(), &options);
        page.enregistrement = crate::composants::Enregistrement::maintenant(&mut *db, erreurs).await?;
        return Ok(Html(page.render()?).into_response());
    }
    if erreurs.is_empty() {
        return Ok(Redirect::to(genre.base()).into_response());
    }
    let page = page_formulaire(genre, titre_fiche(genre, &retenue), format!("{}/{id}", genre.base()), form, erreurs, &options);
    Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
}

// ─── Routes ─────────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/entreprises",
            get(|db: Base, Query(r): Query<Recherche>, h: HeaderMap| liste(Genre::Entreprise, db, r, h))
                .post(|db: Base, Form(f): Form<FicheForm>| creer(Genre::Entreprise, db, f)),
        )
        .route("/entreprises/nouveau", get(|db: Base| nouveau(Genre::Entreprise, db)))
        .route(
            "/entreprises/{id}",
            axum::routing::post(|db: Base, Path(id): Path<i64>, h: HeaderMap, Form(f): Form<FicheForm>| {
                enregistrer_modification(Genre::Entreprise, db, id, h, f)
            }),
        )
        .route(
            "/entreprises/{id}/modifier",
            get(|db: Base, Path(id): Path<i64>| page_modifier(Genre::Entreprise, db, id)),
        )
        .route(
            "/contacts",
            get(|db: Base, Query(r): Query<Recherche>, h: HeaderMap| liste(Genre::Contact, db, r, h))
                .post(|db: Base, Form(f): Form<FicheForm>| creer(Genre::Contact, db, f)),
        )
        .route("/contacts/nouveau", get(|db: Base| nouveau(Genre::Contact, db)))
        .route(
            "/contacts/{id}",
            axum::routing::post(|db: Base, Path(id): Path<i64>, h: HeaderMap, Form(f): Form<FicheForm>| {
                enregistrer_modification(Genre::Contact, db, id, h, f)
            }),
        )
        .route(
            "/contacts/{id}/modifier",
            get(|db: Base, Path(id): Path<i64>| page_modifier(Genre::Contact, db, id)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libelles_courts_des_taux() {
        assert_eq!(taux_court("Taux réduit (6,00 %)"), "6 %");
        assert_eq!(taux_court("Taux normal (21,00 %)"), "21 %");
        assert_eq!(taux_court("Taux français (5,50 %)"), "5,50 %");
        assert_eq!(taux_court("TVA acquittée par autoliquidation (0,00 %)"), "0 % autoliq.");
        assert_eq!(taux_court("Sans parenthèse"), "Sans parenthèse");
    }

    #[test]
    fn montants_en_euros() {
        assert_eq!(euros_vers_centimes("1150"), Some(115_000));
        assert_eq!(euros_vers_centimes("1 150,50"), Some(115_050));
        assert_eq!(euros_vers_centimes("1150.5"), Some(115_050));
        assert_eq!(euros_vers_centimes("0,07"), Some(7));
        assert_eq!(euros_vers_centimes(",5"), Some(50));
        assert_eq!(euros_vers_centimes(""), None);
        assert_eq!(euros_vers_centimes("abc"), None);
        assert_eq!(euros_vers_centimes("-5"), None);
        assert_eq!(euros_vers_centimes("1,234"), None);
        assert_eq!(centimes_vers_texte(115_050), "1150,50");
        assert_eq!(centimes_vers_texte(7), "0,07");
    }

    #[test]
    fn adresses_email() {
        assert!(email_valide("jean@dupont.be"));
        assert!(!email_valide("jean@dupont"));
        assert!(!email_valide("jean dupont@x.be"));
        assert!(!email_valide("@x.be"));
        assert!(!email_valide("jean@x."));
    }

    #[test]
    fn jokers_de_recherche_neutralises() {
        assert_eq!(echapper_like("50%_a\\b"), "50\\%\\_a\\\\b");
    }
}
