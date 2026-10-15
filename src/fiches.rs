//! Entreprises et contacts : liste avec recherche, création, modification.
//! Les deux écrans fonctionnent pareil ; `Genre` dit duquel on parle.

use askama::Template;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::{FromRow, PgPool, Postgres, postgres::PgArguments, query::Query as SqlQuery};

use crate::{AppState, Erreur};
use crate::commun::{Choix, PAYS, choix_fixes, email_valide, libelle, vide_en_none};

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

/// Les caractères % et _ sont des jokers dans une recherche SQL : on les neutralise.
fn echapper_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn nombre(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}

// ─── Listes de choix pour les menus déroulants ─────────────────────────────

fn choix_base(liste: &[(i64, String)], actuel: &str) -> Vec<Choix> {
    liste
        .iter()
        .map(|(id, l)| Choix {
            valeur: id.to_string(),
            libelle: l.clone(),
            choisi: id.to_string() == actuel,
        })
        .collect()
}

/// Les listes qui viennent de la base : conditions de paiement, taux de TVA, entreprises.
struct Options {
    conditions: Vec<(i64, String)>,
    taux_tva: Vec<(i64, String)>,
    entreprises: Vec<(i64, String)>,
}

impl Options {
    async fn charger(db: &PgPool, genre: Genre) -> Result<Self, sqlx::Error> {
        let conditions: Vec<(i64, String, i32)> = sqlx::query_as(
            "SELECT id, \"type\"::text, jours FROM conditions_paiement
             ORDER BY CASE \"type\"::text WHEN 'apres_facturation' THEN 1 WHEN 'fin_de_mois' THEN 2 ELSE 3 END, jours",
        )
        .fetch_all(db)
        .await?;
        let conditions = conditions
            .into_iter()
            .map(|(id, genre_cond, jours)| {
                let texte = match genre_cond.as_str() {
                    "apres_facturation" => format!("{jours} jour(s) après date de facturation"),
                    "fin_de_mois" => format!("{jours} jour(s) fin du mois"),
                    _ => "Paiement comptant".to_string(),
                };
                (id, texte)
            })
            .collect();

        let taux: Vec<(i64, String, i32)> =
            sqlx::query_as("SELECT id, libelle, taux_centiemes FROM taux_tva WHERE actif ORDER BY taux_centiemes")
                .fetch_all(db)
                .await?;
        let taux_tva = taux
            .into_iter()
            .map(|(id, l, c)| (id, format!("{l} ({},{:02} %)", c / 100, c % 100)))
            .collect();

        let entreprises = if genre.est_contact() {
            sqlx::query_as("SELECT id, nom FROM entreprises ORDER BY lower(nom), id")
                .fetch_all(db)
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
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct FicheForm {
    nom: String,
    prenom: String,
    civilite: String,
    entreprise_id: String,
    profils: Vec<String>,
    numero_tva: String,
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

fn valider(f: &FicheForm, genre: Genre, options: &Options) -> Result<Valeurs, Vec<String>> {
    let mut erreurs = Vec::new();

    let nom = f.nom.trim().to_string();
    if nom.is_empty() {
        erreurs.push("Le nom est obligatoire.".to_string());
    }

    for (texte, champ) in [(&f.email, "L'e-mail"), (&f.email_bis, "L'e-mail (bis)")] {
        let t = texte.trim();
        if !t.is_empty() && !email_valide(t) {
            erreurs.push(format!("{champ} n'est pas valide."));
        }
    }

    let franco_cents = match vide_en_none(&f.franco) {
        None => None,
        Some(t) => match euros_vers_centimes(&t) {
            Some(c) => Some(c),
            None => {
                erreurs.push("Le franco doit être un montant en euros (exemple : 1 150,00).".to_string());
                None
            }
        },
    };

    let profils: Vec<String> = f.profils.iter().filter(|p| !p.is_empty()).cloned().collect();
    if profils.iter().any(|p| !PROFILS.iter().any(|(v, _)| v == p)) {
        erreurs.push("Un des profils choisis n'existe pas.".to_string());
    }

    let conditions_paiement_id = nombre(&f.conditions_paiement_id);
    if let Some(id) = conditions_paiement_id
        && !options.conditions.iter().any(|(i, _)| *i == id)
    {
        erreurs.push("Cette condition de paiement n'existe pas.".to_string());
    }
    let taux_tva_id = nombre(&f.taux_tva_id);
    if let Some(id) = taux_tva_id
        && !options.taux_tva.iter().any(|(i, _)| *i == id)
    {
        erreurs.push("Ce taux de TVA n'existe pas.".to_string());
    }
    let entreprise_id = nombre(&f.entreprise_id);
    if genre.est_contact()
        && let Some(id) = entreprise_id
        && !options.entreprises.iter().any(|(i, _)| *i == id)
    {
        erreurs.push("Cette entreprise n'existe pas.".to_string());
    }

    let source = vide_en_none(&f.source);
    if source.as_deref().is_some_and(|s| !SOURCES.iter().any(|(v, _)| *v == s)) {
        erreurs.push("Cette source n'existe pas.".to_string());
    }
    if !LANGUES.iter().any(|(v, _)| *v == f.langue) {
        erreurs.push("Cette langue n'existe pas.".to_string());
    }
    let civilite = vide_en_none(&f.civilite);
    if civilite.as_deref().is_some_and(|c| !CIVILITES.iter().any(|(v, _)| *v == c)) {
        erreurs.push("Cette civilité n'existe pas.".to_string());
    }
    for pays in [&f.pays, &f.facturation_pays] {
        if !pays.is_empty() && !PAYS.iter().any(|(v, _)| v == pays) {
            erreurs.push("Ce pays n'existe pas.".to_string());
            break;
        }
    }

    if !erreurs.is_empty() {
        return Err(erreurs);
    }

    let identique = f.facturation_identique.is_some();
    // Si l'adresse de facturation est la même, on n'enregistre pas de seconde adresse.
    let facturation = |s: &str| if identique { None } else { vide_en_none(s) };
    let pays = if f.pays.is_empty() { "BE".to_string() } else { f.pays.clone() };

    Ok(Valeurs {
        nom,
        profils,
        numero_tva: vide_en_none(&f.numero_tva),
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

const COLONNES: [&str; 27] = [
    "nom", "profils", "numero_tva", "identifiant_comptable", "franco_cents", "rue", "numero", "code_postal",
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

async fn inserer(db: &PgPool, genre: Genre, v: &Valeurs) -> Result<(), sqlx::Error> {
    let cols = colonnes(genre);
    let marques: Vec<String> = (1..=cols.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        genre.table(),
        cols.join(", "),
        marques.join(", ")
    );
    lier(sqlx::query(sqlx::AssertSqlSafe(sql)), v, genre).execute(db).await?;
    Ok(())
}

/// Renvoie `false` si la fiche n'existe pas.
async fn modifier_en_base(db: &PgPool, genre: Genre, id: i64, v: &Valeurs) -> Result<bool, sqlx::Error> {
    let cols = colonnes(genre);
    let affectations: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c} = ${}", i + 1)).collect();
    let sql = format!(
        "UPDATE {} SET {}, updated_at = now() WHERE id = ${}",
        genre.table(),
        affectations.join(", "),
        cols.len() + 1
    );
    let res = lier(sqlx::query(sqlx::AssertSqlSafe(sql)), v, genre).bind(id).execute(db).await?;
    Ok(res.rows_affected() > 0)
}

async fn charger_fiche(db: &PgPool, genre: Genre, id: i64) -> Result<Option<Fiche>, sqlx::Error> {
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
    sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(db).await
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
    state: AppState,
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
        .fetch_one(&state.db)
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
    .fetch_all(&state.db)
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

async fn nouveau(genre: Genre, state: AppState) -> Result<Html<String>, Erreur> {
    let options = Options::charger(&state.db, genre).await?;
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

async fn creer(genre: Genre, state: AppState, form: FicheForm) -> Result<Response, Erreur> {
    let options = Options::charger(&state.db, genre).await?;
    match valider(&form, genre, &options) {
        Ok(valeurs) => {
            inserer(&state.db, genre, &valeurs).await?;
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

async fn page_modifier(genre: Genre, state: AppState, id: i64) -> Result<Html<String>, Erreur> {
    let fiche = charger_fiche(&state.db, genre, id).await?.ok_or_else(Erreur::introuvable)?;
    let options = Options::charger(&state.db, genre).await?;
    let page = page_formulaire(
        genre,
        if genre.est_contact() { "Modifier le contact" } else { "Modifier l'entreprise" }.to_string(),
        format!("{}/{id}", genre.base()),
        fiche.into(),
        Vec::new(),
        &options,
    );
    Ok(Html(page.render()?))
}

async fn enregistrer_modification(genre: Genre, state: AppState, id: i64, form: FicheForm) -> Result<Response, Erreur> {
    let options = Options::charger(&state.db, genre).await?;
    match valider(&form, genre, &options) {
        Ok(valeurs) => {
            if modifier_en_base(&state.db, genre, id, &valeurs).await? {
                Ok(Redirect::to(genre.base()).into_response())
            } else {
                Err(Erreur::introuvable())
            }
        }
        Err(erreurs) => {
            let page = page_formulaire(
                genre,
                if genre.est_contact() { "Modifier le contact" } else { "Modifier l'entreprise" }.to_string(),
                format!("{}/{id}", genre.base()),
                form,
                erreurs,
                &options,
            );
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
        }
    }
}

// ─── Routes ─────────────────────────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/entreprises",
            get(|State(s): State<AppState>, Query(r): Query<Recherche>, h: HeaderMap| liste(Genre::Entreprise, s, r, h))
                .post(|State(s): State<AppState>, Form(f): Form<FicheForm>| creer(Genre::Entreprise, s, f)),
        )
        .route("/entreprises/nouveau", get(|State(s): State<AppState>| nouveau(Genre::Entreprise, s)))
        .route(
            "/entreprises/{id}",
            axum::routing::post(|State(s): State<AppState>, Path(id): Path<i64>, Form(f): Form<FicheForm>| {
                enregistrer_modification(Genre::Entreprise, s, id, f)
            }),
        )
        .route(
            "/entreprises/{id}/modifier",
            get(|State(s): State<AppState>, Path(id): Path<i64>| page_modifier(Genre::Entreprise, s, id)),
        )
        .route(
            "/contacts",
            get(|State(s): State<AppState>, Query(r): Query<Recherche>, h: HeaderMap| liste(Genre::Contact, s, r, h))
                .post(|State(s): State<AppState>, Form(f): Form<FicheForm>| creer(Genre::Contact, s, f)),
        )
        .route("/contacts/nouveau", get(|State(s): State<AppState>| nouveau(Genre::Contact, s)))
        .route(
            "/contacts/{id}",
            axum::routing::post(|State(s): State<AppState>, Path(id): Path<i64>, Form(f): Form<FicheForm>| {
                enregistrer_modification(Genre::Contact, s, id, f)
            }),
        )
        .route(
            "/contacts/{id}/modifier",
            get(|State(s): State<AppState>, Path(id): Path<i64>| page_modifier(Genre::Contact, s, id)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

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
