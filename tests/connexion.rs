// Tests de la connexion : le site servi (`application`) demande d'être connecté, sauf la page de connexion,
// la page client en ligne et /sante.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;
use ubifacture::{AppState, application, connexion};

fn site(pool: &PgPool) -> Router {
    application(AppState { db: pool.clone() })
}

struct Reponse {
    statut: StatusCode,
    lieu: Option<String>,
    cookie: Option<String>,
    entetes: axum::http::HeaderMap,
    corps: String,
}

async fn envoyer(site: &Router, requete: Request<Body>) -> Reponse {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let entetes = r.headers().clone();
    let lieu = entetes.get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let cookie = entetes.get(header::SET_COOKIE).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    Reponse { statut, lieu, cookie, entetes, corps: String::from_utf8_lossy(&octets).to_string() }
}

async fn lire(site: &Router, uri: &str, cookie: Option<&str>) -> Reponse {
    let mut r = Request::builder().uri(uri);
    if let Some(c) = cookie {
        r = r.header(header::COOKIE, c);
    }
    envoyer(site, r.body(Body::empty()).unwrap()).await
}

async fn poster(site: &Router, uri: &str, corps: &str, cookie: Option<&str>) -> Reponse {
    let mut r = Request::builder().method("POST").uri(uri).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        r = r.header(header::COOKIE, c);
    }
    envoyer(site, r.body(Body::from(corps.to_string())).unwrap()).await
}

/// « ubifacture_session=abc; Path=/; … » → « ubifacture_session=abc »
fn valeur_cookie(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_string()
}

#[sqlx::test]
async fn sans_connexion_tout_mene_a_la_page_de_connexion(pool: PgPool) {
    let s = site(&pool);
    assert_eq!(lire(&s, "/", None).await.lieu.as_deref(), Some("/connexion"));
    assert_eq!(lire(&s, "/factures/3", None).await.lieu.as_deref(), Some("/connexion?suite=/factures/3"));
    assert_eq!(poster(&s, "/factures", "", None).await.lieu.as_deref(), Some("/connexion?suite=/factures"), "rien ne se crée sans connexion");
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM factures").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 0);
    // htmx : le navigateur change de page.
    let r = envoyer(&s, Request::builder().uri("/factures?q=x").header("hx-request", "true").body(Body::empty()).unwrap()).await;
    assert_eq!(r.entetes.get("HX-Redirect").map(|v| v.to_str().unwrap()), Some("/connexion?suite=/factures"));

    // Restent ouvertes : la page de connexion, la page client (lien secret), /sante.
    let r = lire(&s, "/connexion", None).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Se connecter") && r.corps.contains("name=\"mot_de_passe\""));
    assert_eq!(lire(&s, "/client/facture/inconnu", None).await.statut, StatusCode::NOT_FOUND);
    assert_eq!(lire(&s, "/sante", None).await.statut, StatusCode::OK);
}

#[sqlx::test]
async fn se_connecter_puis_se_deconnecter(pool: PgPool) {
    connexion::ajouter_utilisateur(&pool, "max@ubique.be", "Max", "un-long-mot-de-passe").await.unwrap();
    assert!(connexion::ajouter_utilisateur(&pool, "MAX@ubique.be", "", "un-long-mot-de-passe").await.is_err(), "pas deux fois le même e-mail");
    assert!(connexion::ajouter_utilisateur(&pool, "autre@ubique.be", "", "court").await.is_err(), "10 caractères au moins");
    let empreinte: String = sqlx::query_scalar("SELECT mot_de_passe FROM utilisateurs").fetch_one(&pool).await.unwrap();
    assert!(empreinte.starts_with("$argon2id$"), "jamais le mot de passe en clair");
    let s = site(&pool);

    let r = poster(&s, "/connexion", "email=max%40ubique.be&mot_de_passe=faux&suite=/factures", None).await;
    assert!(r.statut == StatusCode::UNAUTHORIZED && r.corps.contains("E-mail ou mot de passe incorrect.") && r.cookie.is_none());
    assert!(r.corps.contains("value=\"max@ubique.be\""), "l'e-mail tapé reste");

    let r = poster(&s, "/connexion", "email=Max%40Ubique.be&mot_de_passe=un-long-mot-de-passe&suite=/factures", None).await;
    assert_eq!((r.statut, r.lieu.as_deref()), (StatusCode::SEE_OTHER, Some("/factures")));
    let set_cookie = r.cookie.expect("cookie de session");
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("Secure") && set_cookie.contains("SameSite=Lax"));
    let cookie = valeur_cookie(&set_cookie);
    let r = lire(&s, "/", Some(&cookie)).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Tableau de bord") && r.corps.contains("Se déconnecter"));
    assert_eq!(lire(&s, "/connexion", Some(&cookie)).await.lieu.as_deref(), Some("/"), "déjà connecté");
    // Une suite vers un autre site est ignorée.
    let r = poster(&s, "/connexion", "email=max%40ubique.be&mot_de_passe=un-long-mot-de-passe&suite=//pirate.example", None).await;
    assert_eq!(r.lieu.as_deref(), Some("/"));

    // Se déconnecter ferme la session.
    let r = poster(&s, "/deconnexion", "", Some(&cookie)).await;
    assert_eq!(r.lieu.as_deref(), Some("/connexion"));
    assert_eq!(lire(&s, "/", Some(&cookie)).await.lieu.as_deref(), Some("/connexion"));
    // Un faux cookie ne passe pas.
    assert_eq!(lire(&s, "/", Some("ubifacture_session=0000")).await.lieu.as_deref(), Some("/connexion"));
}

#[sqlx::test]
async fn trop_d_essais_et_changement_de_mot_de_passe(pool: PgPool) {
    connexion::ajouter_utilisateur(&pool, "max@ubique.be", "", "un-long-mot-de-passe").await.unwrap();
    let s = site(&pool);
    let bon = "email=max%40ubique.be&mot_de_passe=un-long-mot-de-passe";
    let cookie = valeur_cookie(&poster(&s, "/connexion", bon, None).await.cookie.unwrap());

    // Changer le mot de passe ferme les sessions ouvertes.
    connexion::changer_mot_de_passe(&pool, "max@ubique.be", "nouveau-mot-de-passe").await.unwrap();
    assert_eq!(lire(&s, "/", Some(&cookie)).await.lieu.as_deref(), Some("/connexion"));
    assert_eq!(poster(&s, "/connexion", bon, None).await.statut, StatusCode::UNAUTHORIZED, "l'ancien ne marche plus");

    // 5 essais ratés (dont celui ci-dessus) : même le bon mot de passe attend un quart d'heure.
    for _ in 0..4 {
        poster(&s, "/connexion", "email=max%40ubique.be&mot_de_passe=faux", None).await;
    }
    let r = poster(&s, "/connexion", "email=max%40ubique.be&mot_de_passe=nouveau-mot-de-passe", None).await;
    assert!(r.statut == StatusCode::UNAUTHORIZED && r.corps.contains("Trop d&#39;essais") || r.corps.contains("Trop d'essais"));
    assert!(r.cookie.is_none());
    // Un e-mail inconnu donne la même réponse qu'un mauvais mot de passe.
    let r = poster(&s, "/connexion", "email=inconnu%40ubique.be&mot_de_passe=quelque-chose", None).await;
    assert!(r.corps.contains("E-mail ou mot de passe incorrect."));
}
