//! Protections du serveur pour la mise en ligne (étape 7-1) :
//! - l'adresse IP de chaque demande (`Ip`), lue de façon sûre derrière les serveurs frontaux de l'hébergeur ;
//! - des limites d'essais par adresse IP (connexion, inscription, mot de passe oublié, back office), en plus des limites
//!   par adresse e-mail ;
//! - les en-têtes de sécurité de toutes les réponses ;
//! - la liste de ce qui manque pour démarrer en production (`problemes_de_production`).
//!
//! Les réglages sont lus une seule fois par `main.rs` (`configurer`) ; les tests gardent ceux de départ (pas de
//! production, aucun serveur frontal).

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request},
    http::{HeaderMap, HeaderValue, header, request::Parts},
    middleware::Next,
    response::Response,
};
use sqlx::PgExecutor;

struct Reglages {
    production: bool,
    /// Le nombre de serveurs frontaux (« reverse proxy » de l'hébergeur) entre Internet et Raven.
    proxies: usize,
}

static REGLAGES: OnceLock<Reglages> = OnceLock::new();

/// Appelé une fois par `main.rs`, avant de servir le site.
pub fn configurer(production: bool, proxies: usize) {
    let _ = REGLAGES.set(Reglages { production, proxies });
}

fn reglages() -> &'static Reglages {
    REGLAGES.get_or_init(|| Reglages { production: false, proxies: 0 })
}

/// Raven tourne en production (`PRODUCTION=1`) : site en https, en-tête HSTS.
pub fn production() -> bool {
    reglages().production
}

// ─── Adresse IP ─────────────────────────────────────────────────────────────

/// L'adresse IP du client, posée sur chaque demande par `poser_ip` ; vide si elle est inconnue (tests des écrans).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ip(pub String);

impl<S: Send + Sync> FromRequestParts<S> for Ip {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Ip, Self::Rejection> {
        Ok(parts.extensions.get::<Ip>().cloned().unwrap_or_default())
    }
}

/// L'adresse du client. Sans serveur frontal (`proxies` = 0), c'est celle qui a ouvert la connexion, et
/// `X-Forwarded-For` est ignoré (n'importe qui peut l'écrire). Derrière des serveurs frontaux, chacun ajoute à droite de
/// `X-Forwarded-For` l'adresse qui lui a parlé : la bonne est la `proxies`-ième en partant de la droite ; ce qui est plus
/// à gauche a pu être écrit par le client lui-même.
pub fn ip_de(entetes: &HeaderMap, connexion: Option<IpAddr>, proxies: usize) -> String {
    let ip = if proxies == 0 {
        connexion
    } else {
        let liste: Vec<&str> = entetes
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect();
        match liste.len() {
            0 => connexion,
            n => liste[n.saturating_sub(proxies)].parse().ok(),
        }
    };
    ip.map(|ip| ip.to_canonical().to_string()).unwrap_or_default()
}

/// Pose l'adresse IP du client sur la demande (`Ip`), pour les limites d'essais et le dossier de preuve d'une signature.
pub async fn poser_ip(mut requete: Request, suite: Next) -> Response {
    let connexion = requete.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    let ip = ip_de(requete.headers(), connexion, reglages().proxies);
    requete.extensions_mut().insert(Ip(ip));
    suite.run(requete).await
}

// ─── Limites d'essais par adresse IP ────────────────────────────────────────

/// Ce qu'on compte par adresse IP. Les limites sont larges : un bureau entier derrière la même connexion internet
/// ne les atteint pas, un robot qui essaie des centaines d'adresses e-mail, si.
#[derive(Clone, Copy, Debug)]
pub enum Essai {
    /// Une connexion ratée (mauvais mot de passe ou adresse inconnue).
    Connexion,
    /// Une connexion ratée au back office de Raven.
    ConnexionAdmin,
    /// Une inscription envoyée.
    Inscription,
    /// Une demande de lien « mot de passe oublié ».
    MotDePasse,
}

impl Essai {
    fn code(self) -> &'static str {
        match self {
            Essai::Connexion => "connexion",
            Essai::ConnexionAdmin => "connexion_admin",
            Essai::Inscription => "inscription",
            Essai::MotDePasse => "mot_de_passe",
        }
    }

    /// Le nombre d'essais permis pendant `minutes`.
    pub fn maximum(self) -> i64 {
        match self {
            Essai::Connexion => 20,
            Essai::ConnexionAdmin | Essai::Inscription | Essai::MotDePasse => 10,
        }
    }

    fn minutes(self) -> i32 {
        match self {
            Essai::Connexion | Essai::ConnexionAdmin => 15,
            Essai::Inscription | Essai::MotDePasse => 60,
        }
    }

    /// Le message affiché quand la limite est atteinte.
    pub fn message(self) -> &'static str {
        match self {
            Essai::Connexion | Essai::ConnexionAdmin => {
                "Trop d'essais ratés depuis votre connexion internet : attendez un quart d'heure avant de réessayer."
            }
            Essai::Inscription => "Trop d'inscriptions depuis votre connexion internet : réessayez dans une heure.",
            Essai::MotDePasse => "Trop de demandes depuis votre connexion internet : réessayez dans une heure.",
        }
    }
}

/// La limite est atteinte pour cette adresse. Une adresse inconnue n'est jamais limitée (les limites par e-mail restent).
pub async fn trop_d_essais(db: impl PgExecutor<'_>, ip: &Ip, essai: Essai) -> Result<bool, sqlx::Error> {
    if ip.0.is_empty() {
        return Ok(false);
    }
    let nombre: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM essais_ip WHERE genre = $1 AND ip = $2 AND created_at > now() - make_interval(mins => $3)",
    )
    .bind(essai.code())
    .bind(&ip.0)
    .bind(essai.minutes())
    .fetch_one(db)
    .await?;
    Ok(nombre >= essai.maximum())
}

/// Compte un essai pour cette adresse, et efface ceux de plus d'un jour (une adresse IP n'est gardée qu'un jour).
pub async fn noter(db: impl PgExecutor<'_> + Copy, ip: &Ip, essai: Essai) -> Result<(), sqlx::Error> {
    if ip.0.is_empty() {
        return Ok(());
    }
    sqlx::query("DELETE FROM essais_ip WHERE created_at < now() - interval '1 day'").execute(db).await?;
    sqlx::query("INSERT INTO essais_ip (ip, genre) VALUES ($1, $2)").bind(&ip.0).bind(essai.code()).execute(db).await?;
    Ok(())
}

// ─── En-têtes de sécurité ───────────────────────────────────────────────────

/// Ce qu'une page de Raven a le droit de charger (étape 7-2, depuis que tout est servi par Raven lui-même) :
/// - **scripts : seulement ceux de Raven** (htmx), aucun script écrit dans la page ni venu d'ailleurs. Si quelqu'un
///   arrivait un jour à glisser du code dans un texte affiché (nom de client, remarque…), le navigateur refuserait de
///   l'exécuter ;
/// - styles de Raven et styles écrits dans les gabarits (`<style>`, `style="…"`) ;
/// - images de Raven (logos) et images décrites dans la page (`data:`, icônes de DaisyUI) ;
/// - polices, demandes htmx et cadres (le PDF du devis) : Raven seulement ; aucun plugin (`object-src`) ;
/// - personne d'autre ne peut afficher Raven dans un cadre, ni détourner l'adresse de base ou l'envoi d'un formulaire.
const POLITIQUE: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
    font-src 'self'; connect-src 'self'; frame-src 'self'; object-src 'none'; frame-ancestors 'self'; base-uri 'self'; \
    form-action 'self'";

/// Les en-têtes de sécurité, ajoutés à chaque réponse sans remplacer ceux qu'une page a déjà posés :
/// - `X-Content-Type-Options` : le navigateur ne devine pas le type d'un fichier (un fichier envoyé reste un fichier) ;
/// - `X-Frame-Options` et `frame-ancestors` : aucun autre site ne peut afficher Raven dans un cadre (le PDF du devis
///   s'affiche dans un cadre de Raven lui-même) ;
/// - `Referrer-Policy` : l'adresse d'une page (le lien secret de la page client) n'est jamais donnée à un autre site ;
/// - `Permissions-Policy` : ni caméra, ni micro, ni position, ni paiement par le navigateur ;
/// - `Cache-Control: no-store` : devis, factures et chiffres ne restent pas dans le cache d'un ordinateur partagé ;
/// - `Strict-Transport-Security`, en production seulement : le navigateur n'ouvre plus Raven qu'en https ;
/// - `Content-Security-Policy` sur les pages HTML (`POLITIQUE`).
pub async fn entetes(requete: Request, suite: Next) -> Response {
    let mut reponse = suite.run(requete).await;
    let html = reponse
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let h = reponse.headers_mut();
    let mut poser = |nom: header::HeaderName, valeur: &'static str| {
        h.entry(nom).or_insert(HeaderValue::from_static(valeur));
    };
    poser(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    poser(header::X_FRAME_OPTIONS, "SAMEORIGIN");
    poser(header::REFERRER_POLICY, "same-origin");
    poser(header::HeaderName::from_static("permissions-policy"), "camera=(), microphone=(), geolocation=(), payment=()");
    poser(header::CACHE_CONTROL, "no-store");
    if html {
        poser(header::CONTENT_SECURITY_POLICY, POLITIQUE);
    }
    if production() {
        poser(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000");
    }
    reponse
}

// ─── Démarrage en production ────────────────────────────────────────────────

/// Ce qui manque pour démarrer en production ; vide si tout est prêt. `variable` lit une variable d'environnement.
/// `main.rs` refuse de démarrer tant que la liste n'est pas vide : mieux vaut un site arrêté qu'un site qui n'envoie pas
/// ses e-mails ou dont la base ne protège plus les organisations les unes des autres.
pub async fn problemes_de_production(db: impl PgExecutor<'_>, variable: impl Fn(&str) -> Option<String>) -> Result<Vec<String>, sqlx::Error> {
    let lire = |nom: &str| variable(nom).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let mut problemes = Vec::new();
    if !lire("ADRESSE_PUBLIQUE").is_some_and(|a| a.starts_with("https://")) {
        problemes.push("ADRESSE_PUBLIQUE doit être l'adresse du site en https:// (liens des e-mails et de la page client).".to_string());
    }
    if lire("BREVO_CLE_API").is_none() {
        problemes.push("BREVO_CLE_API manque : aucun e-mail ne partirait (inscription, devis, factures, rappels).".to_string());
    }
    if !lire("EMAIL_EXPEDITEUR").is_some_and(|e| crate::commun::email_valide(&e)) {
        problemes.push("EMAIL_EXPEDITEUR manque ou n'est pas une adresse e-mail (validée chez Brevo).".to_string());
    }
    match lire("PROXIES_DE_CONFIANCE").map(|v| v.parse::<usize>()) {
        Some(Ok(n)) if n <= 5 => {}
        Some(_) => problemes.push("PROXIES_DE_CONFIANCE doit être un nombre de 0 à 5.".to_string()),
        None => problemes.push(
            "PROXIES_DE_CONFIANCE manque : le nombre de serveurs frontaux de l'hébergeur devant Raven (1 en général). Sans \
             lui, tous les visiteurs auraient la même adresse IP et les limites d'essais les bloqueraient ensemble."
                .to_string(),
        ),
    }
    // Le second filet de l'isolation : l'utilisateur de la base doit être soumis aux règles par organisation (RLS).
    let contourne: bool = sqlx::query_scalar("SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user").fetch_one(db).await?;
    if contourne {
        problemes.push(
            "L'utilisateur de la base est super-utilisateur ou contourne la sécurité par ligne (BYPASSRLS) : les organisations \
             ne seraient plus protégées les unes des autres. Utilisez un compte ordinaire (voir scripts/creer-base.sql)."
                .to_string(),
        );
    }
    Ok(problemes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avec(xff: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in xff {
            h.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn sans_serveur_frontal_l_adresse_est_celle_de_la_connexion() {
        let connexion = Some("198.51.100.4".parse().unwrap());
        assert_eq!(ip_de(&avec(&["203.0.113.7"]), connexion, 0), "198.51.100.4", "X-Forwarded-For écrit par le client : ignoré");
        assert_eq!(ip_de(&HeaderMap::new(), None, 0), "", "adresse inconnue");
        let v6_de_v4 = Some("::ffff:198.51.100.4".parse().unwrap());
        assert_eq!(ip_de(&HeaderMap::new(), v6_de_v4, 0), "198.51.100.4", "une adresse IPv4 écrite en IPv6 est remise en IPv4");
    }

    #[test]
    fn derriere_un_serveur_frontal_l_adresse_est_la_derniere_ajoutee() {
        let frontal = Some("10.0.0.1".parse().unwrap());
        assert_eq!(ip_de(&avec(&["203.0.113.7"]), frontal, 1), "203.0.113.7");
        assert_eq!(ip_de(&avec(&["1.2.3.4, 203.0.113.7"]), frontal, 1), "203.0.113.7", "1.2.3.4 a été écrit par le client");
        assert_eq!(ip_de(&avec(&["1.2.3.4", "203.0.113.7"]), frontal, 1), "203.0.113.7", "plusieurs en-têtes : mis bout à bout");
        assert_eq!(ip_de(&avec(&["1.2.3.4, 203.0.113.7, 10.0.0.2"]), frontal, 2), "203.0.113.7", "deux serveurs frontaux");
        assert_eq!(ip_de(&avec(&["203.0.113.7"]), frontal, 2), "203.0.113.7", "moins d'adresses que de serveurs frontaux");
        assert_eq!(ip_de(&avec(&["2001:db8::1"]), frontal, 1), "2001:db8::1");
        assert_eq!(ip_de(&HeaderMap::new(), frontal, 1), "10.0.0.1", "sans en-tête : l'adresse de la connexion");
        assert_eq!(ip_de(&avec(&["<script>alert(1)</script>"]), frontal, 1), "", "une valeur qui n'est pas une adresse est ignorée");
    }
}
