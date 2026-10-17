//! Connexion (avant la mise en ligne), comme Nodimmo : e-mail + mot de passe (Argon2), session par cookie.
//!
//! - Toutes les pages demandent d'être connecté, **sauf** la page de connexion, la page client en ligne
//!   (`/client/…`, ouverte par les clients avec leur lien secret) et `/sante`.
//! - Le cookie contient un jeton aléatoire ; la base n'en garde que l'empreinte SHA-256 (une copie de la base ne
//!   permet pas de se faire passer pour quelqu'un). Session de 30 jours.
//! - 5 essais ratés par quart d'heure et par e-mail, ensuite on attend.
//! - Les utilisateurs se créent en ligne de commande (`cargo run -- utilisateur ajouter <e-mail>`), jamais depuis le
//!   site : il n'y a pas de page d'inscription.
//!
//! `crate::application` assemble le site protégé (utilisé par `main.rs`) ; `crate::app` reste le site sans
//! connexion pour les tests des écrans.

use std::sync::LazyLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use askama::Template;
use axum::{
    Router,
    extract::{Form, Query, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::{AppState, Erreur};

const COOKIE: &str = "ubifacture_session";
const DUREE_JOURS: i32 = 30;
const ESSAIS_MAX: i64 = 5;
const LONGUEUR_MIN: usize = 10;

// ─── Mots de passe ──────────────────────────────────────────────────────────

pub fn hacher(mot_de_passe: &str) -> String {
    Argon2::default().hash_password(mot_de_passe.as_bytes()).expect("hachage Argon2 impossible").to_string()
}

pub fn correct(mot_de_passe: &str, empreinte: &str) -> bool {
    PasswordHash::new(empreinte).is_ok_and(|h| Argon2::default().verify_password(mot_de_passe.as_bytes(), &h).is_ok())
}

/// E-mail inconnu : on vérifie quand même un faux mot de passe, pour que la réponse prenne le même temps.
fn perdre_le_meme_temps(mot_de_passe: &str) {
    static FAUSSE: LazyLock<String> = LazyLock::new(|| hacher("aucun-utilisateur-n-a-ce-mot-de-passe"));
    correct(mot_de_passe, &FAUSSE);
}

/// Un mot de passe accepté : au moins 10 caractères.
pub fn assez_long(mot_de_passe: &str) -> bool {
    mot_de_passe.chars().count() >= LONGUEUR_MIN
}

// ─── Utilisateurs (ligne de commande) ───────────────────────────────────────

/// Ajoute un utilisateur. Erreur lisible si l'e-mail existe déjà ou si le mot de passe est trop court.
pub async fn ajouter_utilisateur(db: &PgPool, email: &str, nom: &str, mot_de_passe: &str) -> Result<(), String> {
    let email = email.trim();
    if !crate::commun::email_valide(email) {
        return Err(format!("« {email} » n'est pas une adresse e-mail."));
    }
    if !assez_long(mot_de_passe) {
        return Err(format!("Le mot de passe doit faire au moins {LONGUEUR_MIN} caractères."));
    }
    sqlx::query("INSERT INTO utilisateurs (email, nom, mot_de_passe) VALUES ($1, $2, $3)")
        .bind(email)
        .bind(nom.trim())
        .bind(hacher(mot_de_passe))
        .execute(db)
        .await
        .map_err(|e| if e.to_string().contains("utilisateurs_email") { format!("{email} existe déjà.") } else { e.to_string() })?;
    Ok(())
}

/// Change le mot de passe et ferme toutes les sessions de cet utilisateur.
pub async fn changer_mot_de_passe(db: &PgPool, email: &str, mot_de_passe: &str) -> Result<(), String> {
    if !assez_long(mot_de_passe) {
        return Err(format!("Le mot de passe doit faire au moins {LONGUEUR_MIN} caractères."));
    }
    let id: Option<i64> = sqlx::query_scalar("UPDATE utilisateurs SET mot_de_passe = $2 WHERE lower(email) = lower($1) RETURNING id")
        .bind(email.trim())
        .bind(hacher(mot_de_passe))
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?;
    let id = id.ok_or_else(|| format!("Aucun utilisateur {email}."))?;
    sqlx::query("DELETE FROM sessions WHERE utilisateur_id = $1").bind(id).execute(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn liste_utilisateurs(db: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT email, nom FROM utilisateurs ORDER BY lower(email)").fetch_all(db).await
}

// ─── Sessions ───────────────────────────────────────────────────────────────

fn empreinte(jeton: &str) -> Vec<u8> {
    Sha256::digest(jeton.as_bytes()).to_vec()
}

async fn ouvrir(db: &PgPool, utilisateur_id: i64, cookies: CookieJar) -> Result<CookieJar, sqlx::Error> {
    let mut octets = [0u8; 32];
    getrandom::fill(&mut octets).expect("le système ne fournit pas de hasard");
    let jeton = hex::encode(octets);
    sqlx::query("DELETE FROM sessions WHERE expire_le < now()").execute(db).await?;
    sqlx::query("INSERT INTO sessions (empreinte, utilisateur_id, expire_le) VALUES ($1, $2, now() + make_interval(days => $3))")
        .bind(empreinte(&jeton))
        .bind(utilisateur_id)
        .bind(DUREE_JOURS)
        .execute(db)
        .await?;
    let cookie = Cookie::build((COOKIE, jeton))
        .path("/")
        .http_only(true) // invisible pour le JavaScript de la page
        .secure(true) // envoyé seulement en HTTPS (ou sur 127.0.0.1 en local)
        .same_site(SameSite::Lax) // pas envoyé par les formulaires d'autres sites
        .max_age(time::Duration::days(i64::from(DUREE_JOURS)));
    Ok(cookies.add(cookie))
}

/// L'utilisateur connecté (id), d'après le cookie.
async fn utilisateur(db: &PgPool, cookies: &CookieJar) -> Result<Option<i64>, sqlx::Error> {
    let Some(cookie) = cookies.get(COOKIE) else { return Ok(None) };
    sqlx::query_scalar("SELECT utilisateur_id FROM sessions WHERE empreinte = $1 AND expire_le > now()")
        .bind(empreinte(cookie.value()))
        .fetch_optional(db)
        .await
}

/// Les adresses ouvertes sans connexion.
fn publique(chemin: &str) -> bool {
    chemin == "/connexion" || chemin == "/sante" || chemin.starts_with("/client/")
}

/// N'accepte que des adresses internes simples pour revenir après la connexion (pas de renvoi vers un autre site).
pub fn suite_sure(suite: &str) -> Option<&str> {
    let interne = suite.starts_with('/') && !suite.starts_with("//");
    let caracteres_ok = suite.chars().all(|c| c.is_ascii_alphanumeric() || "/-_".contains(c));
    (interne && caracteres_ok).then_some(suite)
}

/// Garde de toutes les pages : sans session valable, renvoie vers la page de connexion (en retenant la page voulue).
pub async fn exiger_connexion(State(state): State<AppState>, cookies: CookieJar, requete: Request, suite: Next) -> Response {
    let chemin = requete.uri().path().to_string();
    if publique(&chemin) {
        return suite.run(requete).await;
    }
    match utilisateur(&state.db, &cookies).await {
        Ok(Some(_)) => suite.run(requete).await,
        Ok(None) => {
            let adresse = match suite_sure(&chemin) {
                Some(c) if c != "/" => format!("/connexion?suite={c}"),
                _ => "/connexion".to_string(),
            };
            if requete.headers().contains_key("hx-request") {
                (StatusCode::OK, [("HX-Redirect", adresse)]).into_response()
            } else {
                Redirect::to(&adresse).into_response()
            }
        }
        Err(e) => Erreur::from(e).into_response(),
    }
}

// ─── Page de connexion ──────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct Saisie {
    email: String,
    mot_de_passe: String,
    /// La page à ouvrir après la connexion.
    suite: String,
}

#[derive(Template)]
#[template(path = "connexion.html")]
struct Page {
    email: String,
    suite: String,
    erreur: &'static str,
}

async fn afficher(State(state): State<AppState>, cookies: CookieJar, Query(s): Query<Saisie>) -> Result<Response, Erreur> {
    if utilisateur(&state.db, &cookies).await?.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Html(Page { email: String::new(), suite: s.suite, erreur: "" }.render()?).into_response())
}

fn refus(s: Saisie, erreur: &'static str) -> Result<Response, Erreur> {
    let page = Page { email: s.email.trim().to_string(), suite: s.suite, erreur };
    Ok((StatusCode::UNAUTHORIZED, Html(page.render()?)).into_response())
}

async fn envoyer(State(state): State<AppState>, cookies: CookieJar, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let db = &state.db;
    let email = s.email.trim().to_string();
    let essais: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connexions_ratees WHERE lower(email) = lower($1) AND created_at > now() - interval '15 minutes'",
    )
    .bind(&email)
    .fetch_one(db)
    .await?;
    if essais >= ESSAIS_MAX {
        return refus(s, "Trop d'essais ratés : attends un quart d'heure avant de réessayer.");
    }
    let trouve: Option<(i64, String)> =
        sqlx::query_as("SELECT id, mot_de_passe FROM utilisateurs WHERE lower(email) = lower($1)").bind(&email).fetch_optional(db).await?;
    let id = match trouve {
        Some((id, empreinte)) if correct(&s.mot_de_passe, &empreinte) => id,
        autre => {
            if autre.is_none() {
                perdre_le_meme_temps(&s.mot_de_passe);
            }
            sqlx::query("INSERT INTO connexions_ratees (email) VALUES ($1)").bind(&email).execute(db).await?;
            return refus(s, "E-mail ou mot de passe incorrect.");
        }
    };
    sqlx::query("DELETE FROM connexions_ratees WHERE lower(email) = lower($1) OR created_at < now() - interval '1 day'")
        .bind(&email)
        .execute(db)
        .await?;
    let cookies = ouvrir(db, id, cookies).await?;
    let suite = suite_sure(&s.suite).unwrap_or("/").to_string();
    Ok((cookies, Redirect::to(&suite)).into_response())
}

async fn deconnecter(State(state): State<AppState>, cookies: CookieJar) -> Result<Response, Erreur> {
    if let Some(cookie) = cookies.get(COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE empreinte = $1").bind(empreinte(cookie.value())).execute(&state.db).await?;
    }
    let cookies = cookies.remove(Cookie::build(COOKIE).path("/"));
    Ok((cookies, Redirect::to("/connexion")).into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/connexion", get(afficher).post(envoyer)).route("/deconnexion", post(deconnecter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mot_de_passe_hache_puis_verifie() {
        let e = hacher("correct-cheval-pile");
        assert!(e.starts_with("$argon2id$"));
        assert!(correct("correct-cheval-pile", &e) && !correct("mauvais", &e) && !correct("x", "pas une empreinte"));
        assert!(assez_long("dix-lettres") && !assez_long("court"));
    }

    #[test]
    fn adresses_publiques_et_suite() {
        assert!(publique("/connexion") && publique("/client/facture/abc") && publique("/sante"));
        assert!(!publique("/") && !publique("/factures") && !publique("/clients") && !publique("/deconnexion"));
        assert_eq!(suite_sure("/factures/12"), Some("/factures/12"));
        assert_eq!(suite_sure("//pirate.example"), None);
        assert_eq!(suite_sure("https://pirate.example"), None);
        assert_eq!(suite_sure("/factures?x=<script>"), None);
    }
}
