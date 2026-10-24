// Mise en ligne (étape 7-1) : limites d'essais par adresse IP (connexion, inscription, mot de passe oublié, back office),
// en-têtes de sécurité de chaque réponse, réglages indispensables de la production.
// L'adresse du client est donnée comme le fait `main.rs` : celle qui ouvre la connexion (aucun serveur frontal en test).

mod commun;

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, administration, application, connexion, emails_systeme, securite};

const MDP: &str = "un-mot-de-passe-solide";
const A: &str = "203.0.113.7";
const B: &str = "198.51.100.4";

struct Reponse {
    statut: StatusCode,
    entetes: HeaderMap,
    corps: String,
}

async fn envoyer(site: &Router, methode: &str, chemin: &str, corps: &str, ip: Option<&str>) -> Reponse {
    let mut r = Request::builder().method(methode).uri(chemin);
    if methode == "POST" {
        r = r.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    if let Some(ip) = ip {
        r = r.extension(ConnectInfo(format!("{ip}:40000").parse::<SocketAddr>().unwrap()));
    }
    let reponse = site.clone().oneshot(r.body(Body::from(corps.to_string())).unwrap()).await.unwrap();
    let statut = reponse.status();
    let entetes = reponse.headers().clone();
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    Reponse { statut, entetes, corps: String::from_utf8_lossy(&octets).to_string() }
}

async fn site(options: PgPoolOptions, connexion: PgConnectOptions) -> (PgPool, Router) {
    let pool = commun::reserve(options, connexion).await;
    (pool.clone(), application(AppState { db: pool }))
}

/// Fait comme si cette adresse avait déjà essayé `n` fois.
async fn deja_essaye(pool: &PgPool, ip: &str, genre: &str, n: i32) {
    sqlx::query("INSERT INTO essais_ip (ip, genre) SELECT $1, $2 FROM generate_series(1, $3)")
        .bind(ip)
        .bind(genre)
        .bind(n)
        .execute(pool)
        .await
        .unwrap();
}

async fn essais(pool: &PgPool, ip: &str, genre: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM essais_ip WHERE ip = $1 AND genre = $2").bind(ip).bind(genre).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn les_connexions_ratees_sont_limitees_par_adresse_ip(options: PgPoolOptions, connexion: PgConnectOptions) {
    let (pool, s) = site(options, connexion).await;
    connexion::ajouter_utilisateur(&pool, 1, "max@exemple.be", "", MDP).await.unwrap();
    let bon = format!("email=max%40exemple.be&mot_de_passe={MDP}");

    // Une connexion ratée compte pour son adresse IP (en plus de l'adresse e-mail) ; une adresse inconnue ne compte pas.
    let r = envoyer(&s, "POST", "/connexion", "email=max%40exemple.be&mot_de_passe=mauvais-mot-de-passe", Some(A)).await;
    assert_eq!(r.statut, StatusCode::UNAUTHORIZED);
    assert_eq!(essais(&pool, A, "connexion").await, 1);
    envoyer(&s, "POST", "/connexion", "email=inconnu%40exemple.be&mot_de_passe=mauvais-mot-de-passe", None).await;
    let toutes: i64 = sqlx::query_scalar("SELECT count(*) FROM essais_ip").fetch_one(&pool).await.unwrap();
    assert_eq!(toutes, 1, "sans adresse IP connue, rien n'est compté");

    // 20 ratés depuis la même adresse, avec toutes sortes d'adresses e-mail : même le bon mot de passe attend.
    deja_essaye(&pool, A, "connexion", 19).await;
    let r = envoyer(&s, "POST", "/connexion", &bon, Some(A)).await;
    assert_eq!(r.statut, StatusCode::UNAUTHORIZED);
    assert!(r.corps.contains("depuis votre connexion internet") && !r.entetes.contains_key(header::SET_COOKIE), "{}", r.corps);

    // Une autre adresse n'est pas touchée.
    let r = envoyer(&s, "POST", "/connexion", &bon, Some(B)).await;
    assert!(r.statut.is_redirection() && r.entetes.contains_key(header::SET_COOKIE), "{}", r.corps);
}

#[sqlx::test]
async fn les_inscriptions_et_les_mots_de_passe_oublies_sont_limites_par_adresse_ip(options: PgPoolOptions, connexion: PgConnectOptions) {
    let (pool, s) = site(options, connexion).await;
    let inscription = |email: &str| format!("entreprise=Peinture+Dupont&email={email}&mot_de_passe={MDP}&confirmation={MDP}&site=");

    let r = envoyer(&s, "POST", "/inscription", &inscription("a1%40securite.be"), Some(A)).await;
    assert_eq!(r.statut, StatusCode::OK, "{}", r.corps);
    assert_eq!(essais(&pool, A, "inscription").await, 1, "chaque inscription envoyée compte");
    deja_essaye(&pool, A, "inscription", 9).await;
    let r = envoyer(&s, "POST", "/inscription", &inscription("a2%40securite.be"), Some(A)).await;
    assert_eq!(r.statut, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.corps.contains("Trop d") && r.corps.contains("inscriptions depuis votre connexion internet"), "{}", r.corps);
    let creee: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM utilisateurs WHERE email = 'a2@securite.be')").fetch_one(&pool).await.unwrap();
    assert!(!creee, "rien n'est créé");
    let r = envoyer(&s, "POST", "/inscription", &inscription("b1%40securite.be"), Some(B)).await;
    assert_eq!(r.statut, StatusCode::OK, "une autre adresse s'inscrit");

    // Mot de passe oublié : 10 demandes par heure, et aucun e-mail ne part au-delà.
    connexion::ajouter_utilisateur(&pool, 1, "oubli@securite.be", "", MDP).await.unwrap();
    deja_essaye(&pool, A, "mot_de_passe", 10).await;
    let r = envoyer(&s, "POST", "/mot-de-passe-oublie", "email=oubli%40securite.be", Some(A)).await;
    assert_eq!(r.statut, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.corps.contains("Trop de demandes depuis votre connexion internet"), "{}", r.corps);
    assert!(emails_systeme::boite_de_test("oubli@securite.be").is_empty(), "aucun lien n'est parti");
    let r = envoyer(&s, "POST", "/mot-de-passe-oublie", "email=oubli%40securite.be", Some(B)).await;
    assert_eq!(r.statut, StatusCode::OK);
    assert_eq!(emails_systeme::boite_de_test("oubli@securite.be").len(), 1);
    assert_eq!(essais(&pool, B, "mot_de_passe").await, 1);
}

#[sqlx::test]
async fn les_connexions_ratees_au_back_office_sont_limitees_par_adresse_ip(options: PgPoolOptions, connexion: PgConnectOptions) {
    let (pool, s) = site(options, connexion).await;
    administration::ajouter_administrateur(&pool, "admin@raven.be", "", MDP).await.unwrap();
    let bon = format!("email=admin%40raven.be&mot_de_passe={MDP}");

    envoyer(&s, "POST", "/admin/connexion", "email=admin%40raven.be&mot_de_passe=mauvais-mot-de-passe", Some(A)).await;
    assert_eq!(essais(&pool, A, "connexion_admin").await, 1);
    deja_essaye(&pool, A, "connexion_admin", 9).await;
    let r = envoyer(&s, "POST", "/admin/connexion", &bon, Some(A)).await;
    assert_eq!(r.statut, StatusCode::UNAUTHORIZED);
    assert!(r.corps.contains("depuis votre connexion internet"), "{}", r.corps);
    let r = envoyer(&s, "POST", "/admin/connexion", &bon, Some(B)).await;
    assert!(r.statut.is_redirection() && r.entetes.contains_key(header::SET_COOKIE));
}

#[sqlx::test]
async fn une_adresse_ip_n_est_gardee_qu_un_jour(options: PgPoolOptions, connexion: PgConnectOptions) {
    let (pool, _) = site(options, connexion).await;
    sqlx::query("INSERT INTO essais_ip (ip, genre, created_at) VALUES ($1, 'connexion', now() - interval '25 hours')").bind(B).execute(&pool).await.unwrap();
    securite::noter(&pool, &securite::Ip(A.to_string()), securite::Essai::Connexion).await.unwrap();
    let restes: Vec<String> = sqlx::query_scalar("SELECT ip FROM essais_ip").fetch_all(&pool).await.unwrap();
    assert_eq!(restes, vec![A.to_string()]);
    // Une limite ne compte que sa période : 20 essais d'il y a 20 minutes ne bloquent plus la connexion.
    sqlx::query("INSERT INTO essais_ip (ip, genre, created_at) SELECT $1, 'connexion', now() - interval '20 minutes' FROM generate_series(1, 30)")
        .bind(B)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!securite::trop_d_essais(&pool, &securite::Ip(B.to_string()), securite::Essai::Connexion).await.unwrap());
}

#[sqlx::test]
async fn chaque_reponse_porte_les_en_tetes_de_securite(options: PgPoolOptions, connexion: PgConnectOptions) {
    let (_, s) = site(options, connexion).await;
    // Une page, et une redirection (une page protégée sans être connecté).
    for chemin in ["/connexion", "/devis"] {
        let r = envoyer(&s, "GET", chemin, "", Some(A)).await;
        let lire = |nom: header::HeaderName| r.entetes.get(nom).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        assert_eq!(lire(header::X_CONTENT_TYPE_OPTIONS), "nosniff", "{chemin}");
        assert_eq!(lire(header::X_FRAME_OPTIONS), "SAMEORIGIN", "{chemin}");
        assert_eq!(lire(header::REFERRER_POLICY), "same-origin", "{chemin}");
        assert_eq!(lire(header::CACHE_CONTROL), "no-store", "{chemin}");
        assert!(lire(header::HeaderName::from_static("permissions-policy")).contains("camera=()"), "{chemin}");
        assert!(!r.entetes.contains_key(header::STRICT_TRANSPORT_SECURITY), "HSTS seulement en production (https)");
    }
    let r = envoyer(&s, "GET", "/connexion", "", None).await;
    let politique = r.entetes.get(header::CONTENT_SECURITY_POLICY).unwrap().to_str().unwrap().to_string();
    assert!(politique.contains("frame-ancestors 'self'") && politique.contains("form-action 'self'"), "{politique}");
}

#[sqlx::test]
async fn la_production_ne_demarre_pas_sans_ses_reglages(options: PgPoolOptions, connexion: PgConnectOptions) {
    let (pool, _) = site(options, connexion).await;
    let verifier = async |reglages: &[(&str, &str)]| {
        let variables: HashMap<String, String> = reglages.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        securite::problemes_de_production(&pool, |nom| variables.get(nom).cloned()).await.unwrap()
    };
    let complets = [
        ("ADRESSE_PUBLIQUE", "https://app.raven.be"),
        ("BREVO_CLE_API", "xkeysib-essai"),
        ("EMAIL_EXPEDITEUR", "bonjour@raven.be"),
        ("PROXIES_DE_CONFIANCE", "1"),
    ];
    assert_eq!(verifier(&complets).await, Vec::<String>::new(), "tout est là (et l'utilisateur de la base respecte les règles)");

    let rien = verifier(&[]).await;
    assert_eq!(rien.len(), 4, "{rien:?}");
    for nom in ["ADRESSE_PUBLIQUE", "BREVO_CLE_API", "EMAIL_EXPEDITEUR", "PROXIES_DE_CONFIANCE"] {
        assert!(rien.iter().any(|p| p.starts_with(nom)), "{nom} : {rien:?}");
    }
    let mut http = complets;
    http[0].1 = "http://app.raven.be";
    assert_eq!(verifier(&http).await.len(), 1, "le site doit être en https");
    let mut proxies = complets;
    proxies[3].1 = "beaucoup";
    assert_eq!(verifier(&proxies).await.len(), 1);
    let mut vide = complets;
    vide[2].1 = "  ";
    assert_eq!(verifier(&vide).await.len(), 1, "une valeur vide manque");
}
