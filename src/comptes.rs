//! Les comptes en libre-service (étape 2) : inscription, confirmation de l'adresse e-mail, mot de passe oublié.
//!
//! - **Inscription** (`/inscription`) : nom de l'entreprise, e-mail, mot de passe. Crée l'organisation (complète, voir
//!   `organisations::creer_dans`) et son propriétaire dans une seule transaction, puis envoie le lien de confirmation
//!   (valable 48 heures). Le compte ne se connecte qu'après avoir cliqué ce lien ; il est alors connecté et conduit à
//!   l'assistant de démarrage.
//! - **Mot de passe oublié** (`/mot-de-passe-oublie`) : un lien valable une heure ; le nouveau mot de passe ferme
//!   toutes les sessions ouvertes.
//! - **Sécurité** : les réponses ne disent jamais si une adresse a déjà un compte (même page dans tous les cas ; une
//!   adresse déjà inscrite reçoit un e-mail qui le lui dit). Les liens sont des jetons de 256 bits dont la base ne garde
//!   que l'empreinte, à usage unique. Un lien s'ouvre sur une page avec un bouton (les logiciels qui ouvrent les liens
//!   des e-mails pour les vérifier ne le consomment pas). 3 e-mails au plus par heure et par adresse ; 30 inscriptions
//!   au plus par heure sur tout le site ; un champ invisible piège les robots.

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sqlx::{Connection as _, PgConnection, PgPool};

use crate::connexion::{self, LONGUEUR_MIN, assez_long, empreinte, hacher};
use crate::emails_systeme::{self, Genre};
use crate::{AppState, Erreur, commun::email_valide, organisations};

const VALIDITE_VERIFICATION_MINUTES: i32 = 48 * 60;
const VALIDITE_MOT_DE_PASSE_MINUTES: i32 = 60;
const ENVOIS_MAX_PAR_HEURE: i64 = 3;
const INSCRIPTIONS_MAX_PAR_HEURE: i64 = 30;
const LONGUEUR_MAX_NOM: usize = 120;

// ─── Jetons des liens envoyés par e-mail ────────────────────────────────────

/// Crée un lien à usage unique pour ce compte et rend le jeton (la base n'en garde que l'empreinte).
async fn creer_jeton(db: &mut PgConnection, utilisateur: i64, genre: Genre, minutes: i32) -> Result<String, sqlx::Error> {
    let jeton = connexion::jeton_aleatoire();
    sqlx::query(
        "INSERT INTO jetons_compte (empreinte, genre, utilisateur_id, expire_le) VALUES ($1, $2, $3, now() + make_interval(mins => $4))",
    )
    .bind(empreinte(&jeton))
    .bind(code_jeton(genre))
    .bind(utilisateur)
    .bind(minutes)
    .execute(&mut *db)
    .await?;
    Ok(jeton)
}

fn code_jeton(genre: Genre) -> &'static str {
    if genre == Genre::MotDePasse { "mot_de_passe" } else { "verification" }
}

/// Le compte d'un lien encore valable (sans le consommer).
async fn jeton_valable(db: &PgPool, jeton: &str, genre: Genre) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT utilisateur_id FROM jetons_compte WHERE empreinte = $1 AND genre = $2 AND utilise_le IS NULL AND expire_le > now()")
        .bind(empreinte(jeton))
        .bind(code_jeton(genre))
        .fetch_optional(db)
        .await
}

/// Consomme le lien (une seule fois, même si on clique deux fois en même temps) et rend son compte.
async fn utiliser_jeton(tx: &mut PgConnection, jeton: &str, genre: Genre) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE jetons_compte SET utilise_le = now()
         WHERE empreinte = $1 AND genre = $2 AND utilise_le IS NULL AND expire_le > now() RETURNING utilisateur_id",
    )
    .bind(empreinte(jeton))
    .bind(code_jeton(genre))
    .fetch_optional(&mut *tx)
    .await
}

fn adresse(chemin: &str) -> String {
    format!("{}{chemin}", crate::emails::adresse_publique())
}

async fn envoyer_verification(db: &PgPool, utilisateur: i64, email: &str) -> Result<(), sqlx::Error> {
    let jeton = creer_jeton(&mut *db.acquire().await?, utilisateur, Genre::Verification, VALIDITE_VERIFICATION_MINUTES).await?;
    let texte = format!(
        "Bonjour,\n\nPour confirmer votre adresse e-mail et commencer à utiliser {site}, ouvrez ce lien (valable 48 heures) :\n{lien}\n\n\
         Si vous n'avez pas demandé de compte, ne tenez pas compte de ce message.\n\n{site}",
        site = emails_systeme::NOM_DU_SITE,
        lien = adresse(&format!("/inscription/verifier/{jeton}")),
    );
    emails_systeme::envoyer(db, Genre::Verification, email, "Confirmez votre adresse e-mail", &texte).await
}

// ─── Pages ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "inscription.html")]
struct PageInscription {
    entreprise: String,
    email: String,
    erreurs: Vec<String>,
}

/// Une page de message : un titre, un texte, et au besoin un bouton (formulaire envoyé à `action`) ou le formulaire
/// « renvoyer le lien ».
#[derive(Template)]
#[template(path = "compte_message.html")]
pub(crate) struct PageMessage {
    pub(crate) titre: &'static str,
    pub(crate) texte: String,
    pub(crate) action: String,
    pub(crate) bouton: &'static str,
    pub(crate) renvoyer: bool,
}

impl PageMessage {
    pub(crate) fn texte(titre: &'static str, texte: String) -> PageMessage {
        PageMessage { titre, texte, action: String::new(), bouton: "", renvoyer: false }
    }
}

#[derive(Template)]
#[template(path = "mot_de_passe.html")]
struct PageMotDePasse {
    /// Vide : la demande du lien (e-mail). Sinon : le choix du nouveau mot de passe pour ce lien.
    jeton: String,
    erreur: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Inscription {
    entreprise: String,
    email: String,
    mot_de_passe: String,
    confirmation: String,
    /// Champ invisible : seul un robot le remplit.
    site: String,
}

fn verifier_mot_de_passe(mot_de_passe: &str, confirmation: &str) -> Option<String> {
    if !assez_long(mot_de_passe) {
        Some(format!("Le mot de passe doit faire au moins {LONGUEUR_MIN} caractères."))
    } else if mot_de_passe != confirmation {
        Some("Les deux mots de passe ne sont pas identiques.".to_string())
    } else {
        None
    }
}

fn lien_envoye(email: &str) -> Result<Response, Erreur> {
    let texte = format!(
        "Si l'adresse {email} peut recevoir un compte, un e-mail vient d'y partir avec un lien de confirmation (valable 48 heures). \
         Pensez à regarder dans les indésirables."
    );
    Ok(Html(PageMessage::texte("Vérifiez votre boîte e-mail", texte).render()?).into_response())
}

async fn inscription(cookies: CookieJar) -> Result<Response, Erreur> {
    if cookies.get(connexion::COOKIE).is_some() {
        // Déjà connecté (ou ancienne session) : la connexion décide.
        return Ok(Redirect::to("/connexion").into_response());
    }
    Ok(Html(PageInscription { entreprise: String::new(), email: String::new(), erreurs: Vec::new() }.render()?).into_response())
}

async fn inscrire(State(state): State<AppState>, Form(f): Form<Inscription>) -> Result<Response, Erreur> {
    let db = &state.db;
    let entreprise = f.entreprise.trim().to_string();
    let email = f.email.trim().to_string();
    let mut erreurs = Vec::new();
    if entreprise.is_empty() || entreprise.chars().count() > LONGUEUR_MAX_NOM {
        erreurs.push("Indiquez le nom de votre entreprise (120 caractères au plus).".to_string());
    }
    if !email_valide(&email) {
        erreurs.push("Cette adresse e-mail n'est pas valable.".to_string());
    }
    erreurs.extend(verifier_mot_de_passe(&f.mot_de_passe, &f.confirmation));
    if !erreurs.is_empty() {
        let page = PageInscription { entreprise, email, erreurs };
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response());
    }
    if !f.site.is_empty() {
        return lien_envoye(&email);
    }
    let recentes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM organisations WHERE created_at > now() - interval '1 hour'").fetch_one(db).await?;
    if recentes >= INSCRIPTIONS_MAX_PAR_HEURE {
        let page = PageInscription { entreprise, email, erreurs: vec!["Beaucoup d'inscriptions en ce moment : réessayez dans une heure.".to_string()] };
        return Ok((StatusCode::TOO_MANY_REQUESTS, Html(page.render()?)).into_response());
    }
    let existe: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM utilisateurs WHERE lower(email) = lower($1))").bind(&email).fetch_one(db).await?;
    if existe {
        if emails_systeme::envois_recents(db, Genre::DejaInscrit, &email).await? < ENVOIS_MAX_PAR_HEURE {
            let texte = format!(
                "Bonjour,\n\nQuelqu'un (peut-être vous) a voulu créer un compte {site} avec cette adresse, qui en a déjà un.\n\
                 Pour vous connecter : {connexion}\nMot de passe oublié : {oubli}\n\n{site}",
                site = emails_systeme::NOM_DU_SITE,
                connexion = adresse("/connexion"),
                oubli = adresse("/mot-de-passe-oublie"),
            );
            emails_systeme::envoyer(db, Genre::DejaInscrit, &email, "Vous avez déjà un compte", &texte).await?;
        }
        return lien_envoye(&email);
    }
    let mut conn = db.acquire().await?;
    let mut tx = conn.begin().await?;
    let organisation = match organisations::creer_dans(&mut tx, &entreprise).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Inscription impossible : {e}");
            return Err(Erreur::interne());
        }
    };
    let utilisateur = match connexion::creer_compte(&mut tx, organisation, &email, "", &f.mot_de_passe, "proprietaire", false).await {
        Ok(id) => id,
        // Créé entre-temps (deux envois du formulaire) : même réponse.
        Err(_) => return lien_envoye(&email),
    };
    tx.commit().await?;
    drop(conn);
    envoyer_verification(db, utilisateur, &email).await?;
    lien_envoye(&email)
}

fn lien_perime(titre: &'static str) -> Result<Response, Erreur> {
    let page = PageMessage {
        titre,
        texte: "Ce lien n'est plus valable : il a déjà servi ou il est trop ancien.".to_string(),
        action: String::new(),
        bouton: "",
        renvoyer: titre == "Confirmation de l'adresse",
    };
    Ok((StatusCode::GONE, Html(page.render()?)).into_response())
}

async fn page_verifier(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    if jeton_valable(&state.db, &jeton, Genre::Verification).await?.is_none() {
        return lien_perime("Confirmation de l'adresse");
    }
    let page = PageMessage {
        titre: "Confirmation de l'adresse",
        texte: "Cliquez sur le bouton pour confirmer votre adresse e-mail et commencer.".to_string(),
        action: format!("/inscription/verifier/{jeton}"),
        bouton: "Confirmer mon adresse",
        renvoyer: false,
    };
    Ok(Html(page.render()?).into_response())
}

async fn verifier(State(state): State<AppState>, cookies: CookieJar, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    let db = &state.db;
    let mut conn = db.acquire().await?;
    let mut tx = conn.begin().await?;
    let Some(utilisateur) = utiliser_jeton(&mut tx, &jeton, Genre::Verification).await? else {
        return lien_perime("Confirmation de l'adresse");
    };
    sqlx::query("UPDATE utilisateurs SET email_verifie_le = coalesce(email_verifie_le, now()) WHERE id = $1").bind(utilisateur).execute(&mut *tx).await?;
    let organisation: Option<i64> =
        sqlx::query_scalar("SELECT organisation_id FROM membres WHERE utilisateur_id = $1 ORDER BY organisation_id LIMIT 1")
            .bind(utilisateur)
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    drop(conn);
    let Some(organisation) = organisation else {
        return Ok(Html(PageMessage::texte("Adresse confirmée", "Ce compte n'est rattaché à aucune organisation.".to_string()).render()?).into_response());
    };
    let cookies = connexion::ouvrir(db, utilisateur, organisation, cookies).await?;
    Ok((cookies, Redirect::to("/")).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Adresse {
    email: String,
}

async fn renvoyer(State(state): State<AppState>, Form(a): Form<Adresse>) -> Result<Response, Erreur> {
    let db = &state.db;
    let email = a.email.trim().to_string();
    let compte: Option<i64> =
        sqlx::query_scalar("SELECT id FROM utilisateurs WHERE lower(email) = lower($1) AND email_verifie_le IS NULL").bind(&email).fetch_optional(db).await?;
    if let Some(utilisateur) = compte
        && emails_systeme::envois_recents(db, Genre::Verification, &email).await? < ENVOIS_MAX_PAR_HEURE
    {
        envoyer_verification(db, utilisateur, &email).await?;
    }
    lien_envoye(&email)
}

// ─── Mot de passe oublié ────────────────────────────────────────────────────

async fn page_oubli() -> Result<Html<String>, Erreur> {
    Ok(Html(PageMotDePasse { jeton: String::new(), erreur: String::new() }.render()?))
}

async fn demander_lien(State(state): State<AppState>, Form(a): Form<Adresse>) -> Result<Response, Erreur> {
    let db = &state.db;
    let email = a.email.trim().to_string();
    let compte: Option<(i64, String)> =
        sqlx::query_as("SELECT id, email FROM utilisateurs WHERE lower(email) = lower($1)").bind(&email).fetch_optional(db).await?;
    if let Some((utilisateur, email_du_compte)) = compte
        && emails_systeme::envois_recents(db, Genre::MotDePasse, &email_du_compte).await? < ENVOIS_MAX_PAR_HEURE
    {
        let jeton = creer_jeton(&mut *db.acquire().await?, utilisateur, Genre::MotDePasse, VALIDITE_MOT_DE_PASSE_MINUTES).await?;
        let texte = format!(
            "Bonjour,\n\nPour choisir un nouveau mot de passe, ouvrez ce lien (valable une heure) :\n{lien}\n\n\
             Si vous n'avez rien demandé, ne tenez pas compte de ce message : votre mot de passe ne change pas.\n\n{site}",
            lien = adresse(&format!("/mot-de-passe/{jeton}")),
            site = emails_systeme::NOM_DU_SITE,
        );
        emails_systeme::envoyer(db, Genre::MotDePasse, &email_du_compte, "Nouveau mot de passe", &texte).await?;
    }
    let texte = format!("Si un compte existe avec l'adresse {email}, un e-mail vient d'y partir avec un lien (valable une heure).");
    Ok(Html(PageMessage::texte("Vérifiez votre boîte e-mail", texte).render()?).into_response())
}

async fn page_nouveau(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    if jeton_valable(&state.db, &jeton, Genre::MotDePasse).await?.is_none() {
        return lien_perime("Nouveau mot de passe");
    }
    Ok(Html(PageMotDePasse { jeton, erreur: String::new() }.render()?).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NouveauMotDePasse {
    mot_de_passe: String,
    confirmation: String,
}

async fn changer(State(state): State<AppState>, Path(jeton): Path<String>, Form(n): Form<NouveauMotDePasse>) -> Result<Response, Erreur> {
    if let Some(erreur) = verifier_mot_de_passe(&n.mot_de_passe, &n.confirmation) {
        if jeton_valable(&state.db, &jeton, Genre::MotDePasse).await?.is_none() {
            return lien_perime("Nouveau mot de passe");
        }
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(PageMotDePasse { jeton, erreur }.render()?)).into_response());
    }
    let mut conn = state.db.acquire().await?;
    let mut tx = conn.begin().await?;
    let Some(utilisateur) = utiliser_jeton(&mut tx, &jeton, Genre::MotDePasse).await? else {
        return lien_perime("Nouveau mot de passe");
    };
    // Le lien prouve aussi que l'adresse est la sienne.
    sqlx::query("UPDATE utilisateurs SET mot_de_passe = $2, email_verifie_le = coalesce(email_verifie_le, now()) WHERE id = $1")
        .bind(utilisateur)
        .bind(hacher(&n.mot_de_passe))
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE utilisateur_id = $1").bind(utilisateur).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM jetons_compte WHERE utilisateur_id = $1 AND genre = 'mot_de_passe' AND utilise_le IS NULL")
        .bind(utilisateur)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Redirect::to("/connexion?ok=mot_de_passe").into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/inscription", get(inscription).post(inscrire))
        .route("/inscription/verifier/{jeton}", get(page_verifier).post(verifier))
        .route("/inscription/renvoyer", post(renvoyer))
        .route("/mot-de-passe-oublie", get(page_oubli).post(demander_lien))
        .route("/mot-de-passe/{jeton}", get(page_nouveau).post(changer))
}
