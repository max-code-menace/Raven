//! Le back office de Raven (étape 4-3), sous `/admin` : réservé aux **administrateurs de Raven** (Mahx et ceux qu'il
//! choisit), jamais aux organisations clientes.
//!
//! - **Comptes à part** (`administrateurs`), créés en ligne de commande seulement (`cargo run -- admin ajouter
//!   <e-mail>`) ; un administrateur n'est membre d'aucune organisation.
//! - **Connexion séparée** : page `/admin/connexion`, cookie `raven_admin` limité à `/admin` (HttpOnly, Secure,
//!   SameSite=Strict : jamais envoyé depuis un autre site), session de 12 heures, 5 essais ratés par quart d'heure et
//!   par e-mail. La session d'une organisation n'ouvre pas le back office, et celle du back office n'ouvre aucune page
//!   d'organisation.
//! - **Ce qu'il montre** : le catalogue commun (ajouter, corriger, retirer des ouvrages et des bibliothèques, puis
//!   publier la mise à jour) et la liste des organisations (nom, inscription, métier, nombre d'utilisateurs). Il ne lit
//!   **jamais** les tables de données des organisations (devis, factures, contacts, bibliothèques…).
//!
//! Le catalogue : un ajout attend la publication (`publie_le` vide) ; une correction de texte vaut tout de suite pour
//! les prochaines copies et ne change jamais un ouvrage déjà copié dans une organisation ; retirer écarte des prochaines
//! copies (un ajout pas encore publié est simplement effacé).

use std::sync::LazyLock;

use askama::Template;
use axum::{
    Router,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, request::Parts},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::{
    Form,
    cookie::{Cookie, CookieJar, SameSite},
};
use serde::Deserialize;
use sqlx::PgPool;

use crate::commun::Choix;
use crate::connexion::{correct, empreinte, hacher, jeton_aleatoire};
use crate::{AppState, Erreur};

pub(crate) const COOKIE: &str = "raven_admin";
const DUREE_HEURES: i32 = 12;
const ESSAIS_MAX: i64 = 5;
const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_NOM: usize = 100;
const LONGUEUR_MAX_UNITE: usize = 20;

// ─── Comptes (ligne de commande) ────────────────────────────────────────────

pub async fn ajouter_administrateur(db: &PgPool, email: &str, nom: &str, mot_de_passe: &str) -> Result<(), String> {
    let email = email.trim();
    if !crate::commun::email_valide(email) {
        return Err("Adresse e-mail invalide.".to_string());
    }
    if !crate::connexion::assez_long(mot_de_passe) {
        return Err(format!("Mot de passe trop court ({} caractères au moins).", crate::connexion::LONGUEUR_MIN));
    }
    let existe: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM administrateurs WHERE lower(email) = lower($1))")
        .bind(email)
        .fetch_one(db)
        .await
        .map_err(|e| e.to_string())?;
    if existe {
        return Err(format!("Un administrateur {email} existe déjà."));
    }
    sqlx::query("INSERT INTO administrateurs (email, nom, mot_de_passe) VALUES ($1, $2, $3)")
        .bind(email)
        .bind(nom.trim())
        .bind(hacher(mot_de_passe))
        .execute(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Change le mot de passe et ferme toutes les sessions de cet administrateur.
pub async fn changer_mot_de_passe(db: &PgPool, email: &str, mot_de_passe: &str) -> Result<(), String> {
    if !crate::connexion::assez_long(mot_de_passe) {
        return Err(format!("Mot de passe trop court ({} caractères au moins).", crate::connexion::LONGUEUR_MIN));
    }
    let id: Option<i64> = sqlx::query_scalar("UPDATE administrateurs SET mot_de_passe = $2 WHERE lower(email) = lower($1) RETURNING id")
        .bind(email.trim())
        .bind(hacher(mot_de_passe))
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?;
    let id = id.ok_or_else(|| format!("Aucun administrateur {email}."))?;
    sqlx::query("DELETE FROM sessions_admin WHERE administrateur_id = $1").bind(id).execute(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn retirer_administrateur(db: &PgPool, email: &str) -> Result<(), String> {
    let fait = sqlx::query("DELETE FROM administrateurs WHERE lower(email) = lower($1)")
        .bind(email.trim())
        .execute(db)
        .await
        .map_err(|e| e.to_string())?;
    if fait.rows_affected() == 0 {
        return Err(format!("Aucun administrateur {email}."));
    }
    Ok(())
}

pub async fn liste_administrateurs(db: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT email, nom FROM administrateurs ORDER BY lower(email)").fetch_all(db).await
}

// ─── Session ────────────────────────────────────────────────────────────────

/// L'administrateur connecté (son numéro). Sans session du back office valable, la page renvoie vers
/// `/admin/connexion`.
#[derive(Clone, Copy, Debug)]
pub struct Admin(pub i64);

async fn admin_du_cookie(db: &PgPool, cookies: &CookieJar) -> Result<Option<i64>, sqlx::Error> {
    let Some(cookie) = cookies.get(COOKIE) else { return Ok(None) };
    sqlx::query_scalar("SELECT administrateur_id FROM sessions_admin WHERE empreinte = $1 AND expire_le > now()")
        .bind(empreinte(cookie.value()))
        .fetch_optional(db)
        .await
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Admin, Response> {
        let cookies = CookieJar::from_headers(&parts.headers);
        match admin_du_cookie(&state.db, &cookies).await {
            Ok(Some(id)) => Ok(Admin(id)),
            Ok(None) => Err(Redirect::to("/admin/connexion").into_response()),
            Err(e) => Err(Erreur::from(e).into_response()),
        }
    }
}

fn cookie_de_session(jeton: String) -> Cookie<'static> {
    Cookie::build((COOKIE, jeton))
        .path("/admin")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::hours(i64::from(DUREE_HEURES)))
        .build()
}

// ─── Connexion ──────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "admin_connexion.html")]
struct PageConnexion {
    email: String,
    erreur: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaisieConnexion {
    email: String,
    mot_de_passe: String,
}

async fn page_connexion(State(state): State<AppState>, cookies: CookieJar) -> Result<Response, Erreur> {
    if admin_du_cookie(&state.db, &cookies).await?.is_some() {
        return Ok(Redirect::to("/admin/catalogue").into_response());
    }
    Ok(Html(PageConnexion { email: String::new(), erreur: "" }.render()?).into_response())
}

fn refus(email: &str, erreur: &'static str) -> Result<Response, Erreur> {
    Ok((StatusCode::UNAUTHORIZED, Html(PageConnexion { email: email.to_string(), erreur }.render()?)).into_response())
}

async fn se_connecter(State(state): State<AppState>, cookies: CookieJar, Form(s): Form<SaisieConnexion>) -> Result<Response, Erreur> {
    let db = &state.db;
    let email = s.email.trim().to_string();
    let essais: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connexions_admin_ratees WHERE lower(email) = lower($1) AND created_at > now() - interval '15 minutes'",
    )
    .bind(&email)
    .fetch_one(db)
    .await?;
    if essais >= ESSAIS_MAX {
        return refus(&email, "Trop d'essais ratés : attendez un quart d'heure avant de réessayer.");
    }
    let trouve: Option<(i64, String)> = sqlx::query_as("SELECT id, mot_de_passe FROM administrateurs WHERE lower(email) = lower($1)")
        .bind(&email)
        .fetch_optional(db)
        .await?;
    let id = match trouve {
        Some((id, empreinte_mdp)) if correct(&s.mot_de_passe, &empreinte_mdp) => id,
        autre => {
            if autre.is_none() {
                // Même durée de réponse qu'avec une adresse connue.
                static FAUSSE: LazyLock<String> = LazyLock::new(|| hacher("aucun-administrateur-n-a-ce-mot-de-passe"));
                correct(&s.mot_de_passe, &FAUSSE);
            }
            sqlx::query("INSERT INTO connexions_admin_ratees (email) VALUES ($1)").bind(&email).execute(db).await?;
            return refus(&email, "E-mail ou mot de passe incorrect.");
        }
    };
    sqlx::query("DELETE FROM connexions_admin_ratees WHERE lower(email) = lower($1) OR created_at < now() - interval '1 day'")
        .bind(&email)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM sessions_admin WHERE expire_le < now()").execute(db).await?;
    let jeton = jeton_aleatoire();
    sqlx::query("INSERT INTO sessions_admin (empreinte, administrateur_id, expire_le) VALUES ($1, $2, now() + make_interval(hours => $3))")
        .bind(empreinte(&jeton))
        .bind(id)
        .bind(DUREE_HEURES)
        .execute(db)
        .await?;
    Ok((cookies.add(cookie_de_session(jeton)), Redirect::to("/admin/catalogue")).into_response())
}

async fn se_deconnecter(State(state): State<AppState>, cookies: CookieJar) -> Result<Response, Erreur> {
    if let Some(cookie) = cookies.get(COOKIE) {
        sqlx::query("DELETE FROM sessions_admin WHERE empreinte = $1").bind(empreinte(cookie.value())).execute(&state.db).await?;
    }
    let cookies = cookies.remove(Cookie::build(COOKIE).path("/admin"));
    Ok((cookies, Redirect::to("/admin/connexion")).into_response())
}

async fn accueil(_: Admin) -> Redirect {
    Redirect::to("/admin/catalogue")
}

// ─── Catalogue ──────────────────────────────────────────────────────────────

/// L'état d'un ouvrage du catalogue : « Publié », « À publier » ou « Retiré » (et la classe de sa pastille).
fn etat(publie: bool, retire: bool) -> (&'static str, &'static str) {
    match (publie, retire) {
        (_, true) => ("Retiré", "s-refuse"),
        (false, false) => ("À publier", "s-brouillon"),
        (true, false) => ("Publié", "s-accepte"),
    }
}

/// L'état d'une bibliothèque, au féminin.
fn etat_bibliotheque(publiee: bool, retiree: bool) -> (&'static str, &'static str) {
    match (publiee, retiree) {
        (_, true) => ("Retirée", "s-refuse"),
        (false, false) => ("À publier", "s-brouillon"),
        (true, false) => ("Publiée", "s-accepte"),
    }
}

pub struct LigneBibliotheque {
    pub code: String,
    pub nom: String,
    pub metier: String,
    pub etat: &'static str,
    pub classe: &'static str,
    pub publies: i64,
    pub en_attente: i64,
    pub retires: i64,
}

pub struct LignePublication {
    pub quand: String,
    pub par: String,
    pub ouvrages: i32,
    pub bibliotheques: i32,
}

#[derive(Template)]
#[template(path = "admin_catalogue.html")]
struct PageCatalogue {
    bibliotheques: Vec<LigneBibliotheque>,
    metiers: Vec<Choix>,
    en_attente: i64,
    publications: Vec<LignePublication>,
    message: String,
    erreur: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    ouvrages: i64,
    bibliotheques: i64,
    erreur: String,
}

fn message(r: &Retour) -> String {
    match r.ok.as_str() {
        "publie" if r.ouvrages + r.bibliotheques == 0 => "Rien à publier.".to_string(),
        "publie" => format!(
            "Mise à jour publiée : {} et {}. Les organisations concernées voient « nouveaux ouvrages disponibles ».",
            crate::commun::nombre(r.ouvrages, "ouvrage", "ouvrages"),
            crate::commun::nombre(r.bibliotheques, "bibliothèque", "bibliothèques")
        ),
        "cree" => "Bibliothèque créée : elle sera proposée aux organisations à la prochaine publication.".to_string(),
        "modifie" => "Enregistré.".to_string(),
        "ajoute" => "Ouvrage ajouté : il sera copié chez les organisations à la prochaine publication.".to_string(),
        "retire" => "Retiré : il n'est plus copié (les organisations gardent leurs copies).".to_string(),
        "efface" => "Ajout effacé (il n'avait pas encore été publié).".to_string(),
        "retabli" => "Rétabli.".to_string(),
        _ => String::new(),
    }
}

async fn compter_en_attente(db: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM catalogue_ouvrages WHERE publie_le IS NULL AND retire_le IS NULL)
              + (SELECT count(*) FROM catalogue_bibliotheques WHERE publiee_le IS NULL AND retiree_le IS NULL)",
    )
    .fetch_one(db)
    .await
}

async fn catalogue(_: Admin, State(state): State<AppState>, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let db = &state.db;
    let lignes: Vec<(String, String, String, bool, bool, i64, i64, i64)> = sqlx::query_as(
        "SELECT b.code, b.nom, m.nom, b.publiee_le IS NOT NULL, b.retiree_le IS NOT NULL,
                count(c.id) FILTER (WHERE c.publie_le IS NOT NULL AND c.retire_le IS NULL),
                count(c.id) FILTER (WHERE c.publie_le IS NULL AND c.retire_le IS NULL),
                count(c.id) FILTER (WHERE c.retire_le IS NOT NULL)
         FROM catalogue_bibliotheques b JOIN catalogue_metiers m ON m.code = b.metier
         LEFT JOIN catalogue_ouvrages c ON c.bibliotheque = b.code
         GROUP BY b.code, b.nom, m.nom, m.position, b.position, b.publiee_le, b.retiree_le
         ORDER BY m.position, b.position",
    )
    .fetch_all(db)
    .await?;
    let metiers: Vec<(String, String)> = sqlx::query_as("SELECT code, nom FROM catalogue_metiers ORDER BY position").fetch_all(db).await?;
    let publications: Vec<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT to_char(p.publie_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), COALESCE(a.email, '—'), p.ouvrages, p.bibliotheques
         FROM catalogue_publications p LEFT JOIN administrateurs a ON a.id = p.administrateur_id
         ORDER BY p.publie_le DESC, p.id DESC LIMIT 10",
    )
    .fetch_all(db)
    .await?;
    let page = PageCatalogue {
        bibliotheques: lignes
            .into_iter()
            .map(|(code, nom, metier, publiee, retiree, publies, en_attente, retires)| {
                let (etat, classe) = etat_bibliotheque(publiee, retiree);
                LigneBibliotheque { code, nom, metier, etat, classe, publies, en_attente, retires }
            })
            .collect(),
        metiers: metiers.into_iter().enumerate().map(|(i, (valeur, libelle))| Choix { valeur, libelle, choisi: i == 0 }).collect(),
        en_attente: compter_en_attente(db).await?,
        publications: publications.into_iter().map(|(quand, par, ouvrages, bibliotheques)| LignePublication { quand, par, ouvrages, bibliotheques }).collect(),
        message: message(&r),
        erreur: r.erreur,
    };
    Ok(Html(page.render()?))
}

/// « Gyproc et plafonnage » → « gyproc_et_plafonnage » (le code d'une nouvelle bibliothèque, après celui du métier).
fn slug(nom: &str) -> String {
    let mut sortie = String::new();
    for c in nom.trim().to_lowercase().chars() {
        let remplace = match c {
            'à' | 'â' | 'ä' => "a",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'î' | 'ï' => "i",
            'ô' | 'ö' => "o",
            'ù' | 'û' | 'ü' => "u",
            'ç' => "c",
            'œ' => "oe",
            'æ' => "ae",
            c if c.is_ascii_alphanumeric() => {
                sortie.push(c);
                continue;
            }
            _ => "_",
        };
        if remplace == "_" {
            if !sortie.is_empty() && !sortie.ends_with('_') {
                sortie.push('_');
            }
        } else {
            sortie.push_str(remplace);
        }
    }
    let sortie = sortie.trim_end_matches('_').to_string();
    if sortie.is_empty() { "bibliotheque".to_string() } else { sortie }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaisieBibliotheque {
    metier: String,
    nom: String,
}

fn nom_valide(nom: &str) -> Option<String> {
    let nom = nom.trim();
    (!nom.is_empty() && nom.chars().count() <= LONGUEUR_MAX_NOM).then(|| nom.to_string())
}

async fn creer_bibliotheque(_: Admin, State(state): State<AppState>, Form(s): Form<SaisieBibliotheque>) -> Result<Response, Erreur> {
    let db = &state.db;
    let Some(nom) = nom_valide(&s.nom) else {
        return Ok(Redirect::to("/admin/catalogue?erreur=nom").into_response());
    };
    let metier: Option<String> = sqlx::query_scalar("SELECT code FROM catalogue_metiers WHERE code = $1").bind(&s.metier).fetch_optional(db).await?;
    let Some(metier) = metier else {
        return Ok(Redirect::to("/admin/catalogue?erreur=metier").into_response());
    };
    let base = format!("{metier}.{}", slug(&nom));
    let mut tx = db.begin().await?;
    sqlx::query("LOCK TABLE catalogue_bibliotheques IN SHARE ROW EXCLUSIVE MODE").execute(&mut *tx).await?;
    let mut code = base.clone();
    let mut n = 2;
    while sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM catalogue_bibliotheques WHERE code = $1)").bind(&code).fetch_one(&mut *tx).await? {
        code = format!("{base}_{n}");
        n += 1;
    }
    sqlx::query(
        "INSERT INTO catalogue_bibliotheques (code, metier, nom, position)
         VALUES ($1, $2, $3, (SELECT COALESCE(max(position), 0) + 1 FROM catalogue_bibliotheques))",
    )
    .bind(&code)
    .bind(&metier)
    .bind(&nom)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=cree")).into_response())
}

/// Publie tout ce qui attend : les ouvrages et les bibliothèques ajoutés (pas retirés). Les organisations qui ont coché
/// une bibliothèque voient ensuite « N nouveaux ouvrages disponibles ».
async fn publier(Admin(admin): Admin, State(state): State<AppState>) -> Result<Response, Erreur> {
    let mut tx = state.db.begin().await?;
    let bibliotheques = sqlx::query("UPDATE catalogue_bibliotheques SET publiee_le = now() WHERE publiee_le IS NULL AND retiree_le IS NULL")
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;
    let ouvrages = sqlx::query("UPDATE catalogue_ouvrages SET publie_le = now() WHERE publie_le IS NULL AND retire_le IS NULL")
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;
    if ouvrages + bibliotheques > 0 {
        sqlx::query("INSERT INTO catalogue_publications (administrateur_id, ouvrages, bibliotheques) VALUES ($1, $2, $3)")
            .bind(admin)
            .bind(ouvrages as i32)
            .bind(bibliotheques as i32)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Redirect::to(&format!("/admin/catalogue?ok=publie&ouvrages={ouvrages}&bibliotheques={bibliotheques}")).into_response())
}

// ─── Une bibliothèque du catalogue ──────────────────────────────────────────

pub struct LigneOuvrage {
    pub id: i64,
    pub position: i32,
    pub description: String,
    pub genre: &'static str,
    pub unite: String,
    pub etat: &'static str,
    pub classe: &'static str,
    pub retire: bool,
    pub publie: bool,
}

#[derive(Template)]
#[template(path = "admin_bibliotheque.html")]
struct PageBibliotheque {
    code: String,
    nom: String,
    metier: String,
    etat: &'static str,
    classe: &'static str,
    retiree: bool,
    ouvrages: Vec<LigneOuvrage>,
    genres: Vec<Choix>,
    en_attente: i64,
    message: String,
    erreur: String,
}

fn libelle_genre(code: &str) -> &'static str {
    crate::bibliotheque::GENRES.iter().find(|(c, _)| *c == code).map(|(_, l)| *l).unwrap_or("?")
}

fn genres(choisi: &str) -> Vec<Choix> {
    crate::bibliotheque::GENRES
        .iter()
        .map(|(v, l)| Choix { valeur: (*v).into(), libelle: (*l).into(), choisi: *v == choisi })
        .collect()
}

async fn bibliotheque(_: Admin, State(state): State<AppState>, Path(code): Path<String>, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let db = &state.db;
    let entete: Option<(String, String, bool, bool)> = sqlx::query_as(
        "SELECT b.nom, m.nom, b.publiee_le IS NOT NULL, b.retiree_le IS NOT NULL
         FROM catalogue_bibliotheques b JOIN catalogue_metiers m ON m.code = b.metier WHERE b.code = $1",
    )
    .bind(&code)
    .fetch_optional(db)
    .await?;
    let (nom, metier, publiee, retiree) = entete.ok_or_else(Erreur::introuvable)?;
    let lignes: Vec<(i64, i32, String, String, String, bool, bool)> = sqlx::query_as(
        "SELECT id, position, description, genre::text, unite, publie_le IS NOT NULL, retire_le IS NOT NULL
         FROM catalogue_ouvrages WHERE bibliotheque = $1 ORDER BY position",
    )
    .bind(&code)
    .fetch_all(db)
    .await?;
    let (etat_b, classe_b) = etat_bibliotheque(publiee, retiree);
    let page = PageBibliotheque {
        nom,
        metier,
        etat: etat_b,
        classe: classe_b,
        retiree,
        ouvrages: lignes
            .into_iter()
            .map(|(id, position, description, genre, unite, publie, retire)| {
                let (etat, classe) = etat(publie, retire);
                LigneOuvrage { id, position, description, genre: libelle_genre(&genre), unite, etat, classe, retire, publie }
            })
            .collect(),
        genres: genres("ouvrage"),
        en_attente: compter_en_attente(db).await?,
        message: message(&r),
        erreur: r.erreur,
        code,
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaisieNom {
    nom: String,
}

async fn renommer_bibliotheque(_: Admin, State(state): State<AppState>, Path(code): Path<String>, Form(s): Form<SaisieNom>) -> Result<Response, Erreur> {
    let Some(nom) = nom_valide(&s.nom) else {
        return Ok(Redirect::to(&format!("/admin/catalogue/{code}?erreur=nom")).into_response());
    };
    let fait = sqlx::query("UPDATE catalogue_bibliotheques SET nom = $2 WHERE code = $1").bind(&code).bind(&nom).execute(&state.db).await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=modifie")).into_response())
}

async fn retirer_bibliotheque(_: Admin, State(state): State<AppState>, Path(code): Path<String>) -> Result<Response, Erreur> {
    let fait = sqlx::query("UPDATE catalogue_bibliotheques SET retiree_le = now() WHERE code = $1 AND retiree_le IS NULL")
        .bind(&code)
        .execute(&state.db)
        .await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=retire")).into_response())
}

async fn retablir_bibliotheque(_: Admin, State(state): State<AppState>, Path(code): Path<String>) -> Result<Response, Erreur> {
    let fait = sqlx::query("UPDATE catalogue_bibliotheques SET retiree_le = NULL WHERE code = $1 AND retiree_le IS NOT NULL")
        .bind(&code)
        .execute(&state.db)
        .await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=retabli")).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaisieOuvrage {
    description: String,
    genre: String,
    unite: String,
}

/// (description, type, unité) vérifiés.
fn verifier_ouvrage(s: &SaisieOuvrage) -> Result<(String, &'static str, String), &'static str> {
    let description = s.description.trim();
    if description.is_empty() || description.chars().count() > LONGUEUR_MAX_DESCRIPTION {
        return Err("description");
    }
    let genre = crate::bibliotheque::GENRES.iter().map(|(c, _)| *c).find(|c| *c == s.genre.trim()).ok_or("genre")?;
    let unite = s.unite.trim();
    if unite.chars().count() > LONGUEUR_MAX_UNITE {
        return Err("unite");
    }
    Ok((description.to_string(), genre, unite.to_string()))
}

async fn ajouter_ouvrage(_: Admin, State(state): State<AppState>, Path(code): Path<String>, Form(s): Form<SaisieOuvrage>) -> Result<Response, Erreur> {
    let db = &state.db;
    let existe: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM catalogue_bibliotheques WHERE code = $1)").bind(&code).fetch_one(db).await?;
    if !existe {
        return Err(Erreur::introuvable());
    }
    let (description, genre, unite) = match verifier_ouvrage(&s) {
        Ok(v) => v,
        Err(champ) => return Ok(Redirect::to(&format!("/admin/catalogue/{code}?erreur={champ}")).into_response()),
    };
    let mut tx = db.begin().await?;
    // Une seule position par ouvrage dans sa bibliothèque.
    sqlx::query("SELECT code FROM catalogue_bibliotheques WHERE code = $1 FOR UPDATE").bind(&code).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite)
         VALUES ($1, (SELECT COALESCE(max(position), 0) + 1 FROM catalogue_ouvrages WHERE bibliotheque = $1), $2, $3::text::genre_ligne, $4)",
    )
    .bind(&code)
    .bind(&description)
    .bind(genre)
    .bind(&unite)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=ajoute")).into_response())
}

#[derive(Template)]
#[template(path = "admin_ouvrage.html")]
struct PageOuvrage {
    id: i64,
    code: String,
    bibliotheque: String,
    description: String,
    unite: String,
    genres: Vec<Choix>,
    publie: bool,
    erreur: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RetourOuvrage {
    erreur: String,
}

async fn ouvrage(_: Admin, State(state): State<AppState>, Path(id): Path<i64>, Query(r): Query<RetourOuvrage>) -> Result<Html<String>, Erreur> {
    let ligne: Option<(String, String, String, String, String, bool)> = sqlx::query_as(
        "SELECT c.bibliotheque, b.nom, c.description, c.genre::text, c.unite, c.publie_le IS NOT NULL
         FROM catalogue_ouvrages c JOIN catalogue_bibliotheques b ON b.code = c.bibliotheque WHERE c.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (code, bibliotheque, description, genre, unite, publie) = ligne.ok_or_else(Erreur::introuvable)?;
    Ok(Html(PageOuvrage { id, code, bibliotheque, description, unite, genres: genres(&genre), publie, erreur: r.erreur }.render()?))
}

/// Corrige un ouvrage du catalogue : vaut pour les prochaines copies, jamais pour les ouvrages déjà copiés.
async fn modifier_ouvrage(_: Admin, State(state): State<AppState>, Path(id): Path<i64>, Form(s): Form<SaisieOuvrage>) -> Result<Response, Erreur> {
    let (description, genre, unite) = match verifier_ouvrage(&s) {
        Ok(v) => v,
        Err(champ) => return Ok(Redirect::to(&format!("/admin/catalogue/ouvrages/{id}?erreur={champ}")).into_response()),
    };
    let code: Option<String> = sqlx::query_scalar(
        "UPDATE catalogue_ouvrages SET description = $2, genre = $3::text::genre_ligne, unite = $4 WHERE id = $1 RETURNING bibliotheque",
    )
    .bind(id)
    .bind(&description)
    .bind(genre)
    .bind(&unite)
    .fetch_optional(&state.db)
    .await?;
    let code = code.ok_or_else(Erreur::introuvable)?;
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=modifie")).into_response())
}

/// Retire un ouvrage publié (il n'est plus copié ; les copies restent chez les organisations) ; un ajout pas encore
/// publié n'a été copié nulle part : il est effacé.
async fn retirer_ouvrage(_: Admin, State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let mut tx = state.db.begin().await?;
    let ligne: Option<(String, bool)> =
        sqlx::query_as("SELECT bibliotheque, publie_le IS NOT NULL FROM catalogue_ouvrages WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let (code, publie) = ligne.ok_or_else(Erreur::introuvable)?;
    let ok = if publie {
        sqlx::query("UPDATE catalogue_ouvrages SET retire_le = COALESCE(retire_le, now()) WHERE id = $1").bind(id).execute(&mut *tx).await?;
        "retire"
    } else {
        sqlx::query("DELETE FROM catalogue_ouvrages WHERE id = $1").bind(id).execute(&mut *tx).await?;
        "efface"
    };
    tx.commit().await?;
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok={ok}")).into_response())
}

async fn retablir_ouvrage(_: Admin, State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let code: Option<String> = sqlx::query_scalar("UPDATE catalogue_ouvrages SET retire_le = NULL WHERE id = $1 AND retire_le IS NOT NULL RETURNING bibliotheque")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let code = code.ok_or_else(Erreur::introuvable)?;
    Ok(Redirect::to(&format!("/admin/catalogue/{code}?ok=retabli")).into_response())
}

// ─── Organisations ──────────────────────────────────────────────────────────

pub struct LigneOrganisation {
    pub id: i64,
    pub nom: String,
    pub inscrite: String,
    pub demarree: bool,
    pub metier: String,
    pub utilisateurs: i64,
}

#[derive(Template)]
#[template(path = "admin_organisations.html")]
struct PageOrganisations {
    organisations: Vec<LigneOrganisation>,
}

/// La liste des organisations : seulement ce qui est dans les tables communes (organisations, membres), jamais leurs
/// données.
async fn organisations(_: Admin, State(state): State<AppState>) -> Result<Html<String>, Erreur> {
    let lignes: Vec<(i64, String, String, bool, String, i64)> = sqlx::query_as(
        "SELECT o.id, o.nom, to_char(o.created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY'), o.demarre_le IS NOT NULL, o.profil_metier,
                (SELECT count(*) FROM membres m WHERE m.organisation_id = o.id)
         FROM organisations o ORDER BY o.id",
    )
    .fetch_all(&state.db)
    .await?;
    let page = PageOrganisations {
        organisations: lignes
            .into_iter()
            .map(|(id, nom, inscrite, demarree, metier, utilisateurs)| LigneOrganisation {
                id,
                nom,
                inscrite,
                demarree,
                metier: crate::commun::libelle(&crate::demarrage::PROFILS, &metier),
                utilisateurs,
            })
            .collect(),
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin", get(accueil))
        .route("/admin/connexion", get(page_connexion).post(se_connecter))
        .route("/admin/deconnexion", post(se_deconnecter))
        .route("/admin/catalogue", get(catalogue))
        .route("/admin/catalogue/bibliotheques", post(creer_bibliotheque))
        .route("/admin/catalogue/publier", post(publier))
        .route("/admin/catalogue/ouvrages/{id}", get(ouvrage).post(modifier_ouvrage))
        .route("/admin/catalogue/ouvrages/{id}/retirer", post(retirer_ouvrage))
        .route("/admin/catalogue/ouvrages/{id}/retablir", post(retablir_ouvrage))
        .route("/admin/catalogue/{code}", get(bibliotheque).post(renommer_bibliotheque))
        .route("/admin/catalogue/{code}/ouvrages", post(ajouter_ouvrage))
        .route("/admin/catalogue/{code}/retirer", post(retirer_bibliotheque))
        .route("/admin/catalogue/{code}/retablir", post(retablir_bibliotheque))
        .route("/admin/organisations", get(organisations))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_code_d_une_nouvelle_bibliotheque() {
        assert_eq!(slug("Gyproc et plafonnage"), "gyproc_et_plafonnage");
        assert_eq!(slug("  Élagage & abattage  "), "elagage_abattage");
        assert_eq!(slug("Maçonnerie – gros œuvre"), "maconnerie_gros_oeuvre");
        assert_eq!(slug("!!!"), "bibliotheque");
    }

    #[test]
    fn la_saisie_d_un_ouvrage_du_catalogue() {
        let s = |d: &str, g: &str, u: &str| SaisieOuvrage { description: d.into(), genre: g.into(), unite: u.into() };
        assert_eq!(verifier_ouvrage(&s(" Pose de plinthes ", "ouvrage", " m ")), Ok(("Pose de plinthes".into(), "ouvrage", "m".into())));
        assert_eq!(verifier_ouvrage(&s("", "ouvrage", "m")), Err("description"));
        assert_eq!(verifier_ouvrage(&s("x", "titre", "m")), Err("genre"), "un titre n'est pas un ouvrage");
        assert_eq!(verifier_ouvrage(&s("x", "ouvrage", "une unité beaucoup trop longue")), Err("unite"));
    }
}
