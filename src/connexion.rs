//! Connexion (avant la mise en ligne), comme Nodimmo : e-mail + mot de passe (Argon2), session par cookie.
//!
//! - Toutes les pages demandent d'être connecté, **sauf** la page de connexion, la page client en ligne
//!   (`/client/…`, ouverte par les clients avec leur lien secret) et `/sante`.
//! - Le cookie contient un jeton aléatoire ; la base n'en garde que l'empreinte SHA-256 (une copie de la base ne
//!   permet pas de se faire passer pour quelqu'un). Session de 30 jours.
//! - 5 essais ratés par quart d'heure et par e-mail, ensuite on attend.
//! - Les comptes se créent sur le site (`comptes` : inscription, puis lien de vérification envoyé par e-mail ; ou
//!   invitation dans une équipe) ou en ligne de commande (`cargo run -- utilisateur ajouter <e-mail>`). Un compte
//!   inscrit sur le site ne se connecte qu'après avoir confirmé son adresse e-mail.
//! - Tant que l'assistant de démarrage de l'organisation n'est pas fait, toutes les pages mènent à `/demarrage`.
//!
//! - Un utilisateur est **membre** d'une organisation ; sa session est ouverte dans cette organisation et chaque page
//!   travaille dans celle-ci (`base::Organisation`, posée par `exiger_connexion`).
//!
//! `crate::application` assemble le site protégé (utilisé par `main.rs`) ; `crate::app` reste le site sans
//! connexion pour les tests des écrans (organisation n° 1).

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
use sqlx::{PgConnection, PgPool};

use crate::{AppState, Erreur};

pub(crate) const COOKIE: &str = "ubifacture_session";
const DUREE_JOURS: i32 = 30;
const ESSAIS_MAX: i64 = 5;
pub(crate) const LONGUEUR_MIN: usize = 10;

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

/// Crée un compte, membre de l'organisation avec le rôle donné, dans la transaction de l'appelant. `verifie` : son
/// adresse e-mail est déjà confirmée (ligne de commande, invitation reçue à cette adresse). Erreur lisible si l'e-mail
/// n'est pas valable ou existe déjà, ou si le mot de passe est trop court.
pub(crate) async fn creer_compte(
    tx: &mut PgConnection,
    organisation: i64,
    email: &str,
    nom: &str,
    mot_de_passe: &str,
    role: &str,
    verifie: bool,
) -> Result<i64, String> {
    let email = email.trim();
    if !crate::commun::email_valide(email) {
        return Err(format!("« {email} » n'est pas une adresse e-mail."));
    }
    if !assez_long(mot_de_passe) {
        return Err(format!("Le mot de passe doit faire au moins {LONGUEUR_MIN} caractères."));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO utilisateurs (email, nom, mot_de_passe, email_verifie_le) VALUES ($1, $2, $3, CASE WHEN $4 THEN now() END) RETURNING id",
    )
    .bind(email)
    .bind(nom.trim())
    .bind(hacher(mot_de_passe))
    .bind(verifie)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| if e.to_string().contains("utilisateurs_email") { format!("{email} existe déjà.") } else { e.to_string() })?;
    sqlx::query("INSERT INTO membres (utilisateur_id, organisation_id, role) VALUES ($1, $2, $3::text::role_membre)")
        .bind(id)
        .bind(organisation)
        .bind(role)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Ajoute un utilisateur (ligne de commande), propriétaire de l'organisation donnée, adresse considérée comme
/// vérifiée. Erreur lisible si l'e-mail existe déjà, si le mot de passe est trop court ou si l'organisation n'existe pas.
pub async fn ajouter_utilisateur(db: &PgPool, organisation: i64, email: &str, nom: &str, mot_de_passe: &str) -> Result<(), String> {
    let existe: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organisations WHERE id = $1)")
        .bind(organisation)
        .fetch_one(db)
        .await
        .map_err(|e| e.to_string())?;
    if !existe {
        return Err(format!("Aucune organisation n° {organisation}."));
    }
    let mut tx = db.begin().await.map_err(|e| e.to_string())?;
    creer_compte(&mut tx, organisation, email, nom, mot_de_passe, "proprietaire", true).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
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

/// E-mail, nom et organisations (« 1 Ubique ») de chaque utilisateur.
pub async fn liste_utilisateurs(db: &PgPool) -> Result<Vec<(String, String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.email, u.nom, coalesce(string_agg(o.id || ' ' || o.nom, ', ' ORDER BY o.id), '')
         FROM utilisateurs u LEFT JOIN membres m ON m.utilisateur_id = u.id LEFT JOIN organisations o ON o.id = m.organisation_id
         GROUP BY u.id ORDER BY lower(u.email)",
    )
    .fetch_all(db)
    .await
}


// ─── Sessions ───────────────────────────────────────────────────────────────

pub(crate) fn empreinte(jeton: &str) -> Vec<u8> {
    Sha256::digest(jeton.as_bytes()).to_vec()
}

/// Un jeton aléatoire de 256 bits, en hexadécimal (sessions, liens envoyés par e-mail).
pub(crate) fn jeton_aleatoire() -> String {
    let mut octets = [0u8; 32];
    getrandom::fill(&mut octets).expect("le système ne fournit pas de hasard");
    hex::encode(octets)
}

/// Ouvre une session dans l'organisation et rend le cookie à poser.
pub(crate) async fn ouvrir(db: &PgPool, utilisateur_id: i64, organisation: i64, cookies: CookieJar) -> Result<CookieJar, sqlx::Error> {
    let jeton = jeton_aleatoire();
    sqlx::query("DELETE FROM sessions WHERE expire_le < now()").execute(db).await?;
    sqlx::query(
        "INSERT INTO sessions (empreinte, utilisateur_id, organisation_id, expire_le) VALUES ($1, $2, $3, now() + make_interval(days => $4))",
    )
    .bind(empreinte(&jeton))
    .bind(utilisateur_id)
    .bind(organisation)
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

/// L'utilisateur connecté, l'organisation de sa session (ids) et si son assistant de démarrage est fait, d'après le
/// cookie.
async fn utilisateur(db: &PgPool, cookies: &CookieJar) -> Result<Option<(i64, i64, bool)>, sqlx::Error> {
    let Some(cookie) = cookies.get(COOKIE) else { return Ok(None) };
    sqlx::query_as(
        "SELECT s.utilisateur_id, s.organisation_id, o.demarre_le IS NOT NULL
         FROM sessions s JOIN organisations o ON o.id = s.organisation_id WHERE s.empreinte = $1 AND s.expire_le > now()",
    )
        .bind(empreinte(cookie.value()))
        .fetch_optional(db)
        .await
}

/// Les adresses ouvertes sans connexion. Le back office (`/admin…`) n'est pas ouvert pour autant : il a sa propre
/// connexion (`administration::Admin`), et une session d'organisation ne l'ouvre jamais.
fn publique(chemin: &str) -> bool {
    // `/peppol/webhook` : les notifications de Recommand, signées (voir `factures_recues::webhook`).
    ["/connexion", "/sante", "/inscription", "/mot-de-passe-oublie", "/admin", "/peppol/webhook"].contains(&chemin)
        || chemin.starts_with("/admin/")
        || ["/client/", "/inscription/", "/mot-de-passe/", "/invitation/"].iter().any(|debut| chemin.starts_with(debut))
}

/// Ce qui reste ouvert tant que l'assistant de démarrage n'est pas fait.
fn avant_demarrage(chemin: &str) -> bool {
    chemin == "/demarrage" || chemin == "/deconnexion"
}

/// Envoie le navigateur ailleurs (y compris depuis une requête htmx).
fn rediriger(requete: &Request, adresse: &str) -> Response {
    if requete.headers().contains_key("hx-request") {
        (StatusCode::OK, [("HX-Redirect", adresse.to_string())]).into_response()
    } else {
        Redirect::to(adresse).into_response()
    }
}

/// N'accepte que des adresses internes simples pour revenir après la connexion (pas de renvoi vers un autre site).
pub fn suite_sure(suite: &str) -> Option<&str> {
    let interne = suite.starts_with('/') && !suite.starts_with("//");
    let caracteres_ok = suite.chars().all(|c| c.is_ascii_alphanumeric() || "/-_".contains(c));
    (interne && caracteres_ok).then_some(suite)
}

/// Garde de toutes les pages : sans session valable, renvoie vers la page de connexion (en retenant la page voulue).
pub async fn exiger_connexion(State(state): State<AppState>, cookies: CookieJar, mut requete: Request, suite: Next) -> Response {
    let chemin = requete.uri().path().to_string();
    if publique(&chemin) {
        return suite.run(requete).await;
    }
    match utilisateur(&state.db, &cookies).await {
        Ok(Some((_, _, false))) if !avant_demarrage(&chemin) => rediriger(&requete, "/demarrage"),
        Ok(Some((utilisateur, organisation, _))) => {
            requete.extensions_mut().insert(crate::base::Organisation(organisation));
            requete.extensions_mut().insert(crate::base::Utilisateur(utilisateur));
            let profil = match state.db.acquire().await {
                Ok(mut conn) => crate::profil::charger(&mut conn, organisation).await,
                Err(e) => Err(e),
            };
            match profil {
                Ok(profil) => crate::profil::avec(profil, suite.run(requete)).await,
                Err(e) => Erreur::from(e).into_response(),
            }
        }
        Ok(None) => {
            let adresse = match suite_sure(&chemin) {
                Some(c) if c != "/" => format!("/connexion?suite={c}"),
                _ => "/connexion".to_string(),
            };
            rediriger(&requete, &adresse)
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
    /// Un message après une action réussie (« mot_de_passe » : le mot de passe vient d'être changé).
    ok: String,
}

#[derive(Template)]
#[template(path = "connexion.html")]
struct Page {
    email: String,
    suite: String,
    erreur: &'static str,
    message: &'static str,
    /// Le mot de passe est bon mais l'adresse n'est pas encore confirmée : proposer de renvoyer le lien.
    non_verifie: bool,
}

async fn afficher(State(state): State<AppState>, cookies: CookieJar, Query(s): Query<Saisie>) -> Result<Response, Erreur> {
    if utilisateur(&state.db, &cookies).await?.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    let message = match s.ok.as_str() {
        "mot_de_passe" => "Votre mot de passe est changé : connectez-vous avec le nouveau.",
        _ => "",
    };
    Ok(Html(Page { email: String::new(), suite: s.suite, erreur: "", message, non_verifie: false }.render()?).into_response())
}

fn refus(s: Saisie, erreur: &'static str) -> Result<Response, Erreur> {
    let page = Page { email: s.email.trim().to_string(), suite: s.suite, erreur, message: "", non_verifie: false };
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
        return refus(s, "Trop d'essais ratés : attendez un quart d'heure avant de réessayer.");
    }
    let trouve: Option<(i64, String, bool)> =
        sqlx::query_as("SELECT id, mot_de_passe, email_verifie_le IS NOT NULL FROM utilisateurs WHERE lower(email) = lower($1)")
            .bind(&email)
            .fetch_optional(db)
            .await?;
    let (id, verifie) = match trouve {
        Some((id, empreinte, verifie)) if correct(&s.mot_de_passe, &empreinte) => (id, verifie),
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
    if !verifie {
        let page = Page {
            email: email.clone(),
            suite: s.suite,
            erreur: "Confirmez d'abord votre adresse e-mail : cliquez sur le lien que nous vous avons envoyé.",
            message: "",
            non_verifie: true,
        };
        return Ok((StatusCode::UNAUTHORIZED, Html(page.render()?)).into_response());
    }
    // Pour l'instant, une seule organisation par utilisateur en pratique : la plus ancienne.
    let organisation: Option<i64> =
        sqlx::query_scalar("SELECT organisation_id FROM membres WHERE utilisateur_id = $1 ORDER BY organisation_id LIMIT 1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    let Some(organisation) = organisation else {
        return refus(s, "Ce compte n'est rattaché à aucune organisation.");
    };
    let cookies = ouvrir(db, id, organisation, cookies).await?;
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
