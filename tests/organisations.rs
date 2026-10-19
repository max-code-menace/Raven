// Tests des organisations (étape 1-1) : la connexion d'une page est liée à une organisation, la session est ouverte
// dans l'organisation dont l'utilisateur est membre.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, application, base::Base, connexion, db};

async fn reglage(conn: &mut sqlx::PgConnection) -> Option<String> {
    sqlx::query_scalar("SELECT nullif(current_setting('app.organisation_id', true), '')").fetch_one(conn).await.unwrap()
}

#[sqlx::test]
async fn l_organisation_1_existe_et_la_suivante_porte_le_2(pool: PgPool) {
    let (id, nom): (i64, String) = sqlx::query_as("SELECT id, nom FROM organisations").fetch_one(&pool).await.unwrap();
    assert_eq!((id, nom.as_str()), (1, "Organisation 1"), "base neuve : pas de société, nom par défaut");
    let suivante: i64 = sqlx::query_scalar("INSERT INTO organisations (nom) VALUES ('B') RETURNING id").fetch_one(&pool).await.unwrap();
    assert_eq!(suivante, 2);
    assert!(sqlx::query("INSERT INTO organisations (nom) VALUES ('  ')").execute(&pool).await.is_err(), "un nom est obligatoire");
}

#[sqlx::test]
async fn la_connexion_d_une_page_porte_son_organisation(pool: PgPool) {
    let mut base = Base::ouvrir(&pool, 7).await.unwrap();
    assert_eq!(base.organisation, 7);
    assert_eq!(reglage(&mut base).await.as_deref(), Some("7"));
    // Une transaction sur cette connexion garde le réglage.
    let mut tx = base.begin().await.unwrap();
    assert_eq!(reglage(&mut tx).await.as_deref(), Some("7"));
}

#[sqlx::test]
async fn une_connexion_rendue_a_la_reserve_oublie_son_organisation(_: PgPoolOptions, connexion: PgConnectOptions) {
    // La vraie réserve (`db::options`), réduite à une connexion pour retrouver la même ensuite.
    let pool = db::options().max_connections(1).connect_with(connexion).await.unwrap();
    drop(Base::ouvrir(&pool, 7).await.unwrap());
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(reglage(&mut conn).await, None, "jamais l'organisation de la page précédente");
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let cookie = r.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, cookie, String::from_utf8_lossy(&octets).to_string())
}

async fn lire(site: &Router, uri: &str, cookie: &str) -> StatusCode {
    let requete = Request::builder().uri(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap();
    site.clone().oneshot(requete).await.unwrap().status()
}

#[sqlx::test]
async fn la_session_est_ouverte_dans_l_organisation_du_membre(pool: PgPool) {
    sqlx::query("INSERT INTO organisations (nom) VALUES ('Deuxième')").execute(&pool).await.unwrap();
    connexion::ajouter_utilisateur(&pool, 2, "deux@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    let erreur = connexion::ajouter_utilisateur(&pool, 99, "perdu@exemple.be", "", "un-long-mot-de-passe").await.unwrap_err();
    assert_eq!(erreur, "Aucune organisation n° 99.");
    let utilisateurs: i64 = sqlx::query_scalar("SELECT count(*) FROM utilisateurs").fetch_one(&pool).await.unwrap();
    assert_eq!(utilisateurs, 1, "rien n'est créé pour une organisation inconnue");
    let role: String = sqlx::query_scalar("SELECT role::text FROM membres WHERE organisation_id = 2").fetch_one(&pool).await.unwrap();
    assert_eq!(role, "proprietaire");

    let s = application(AppState { db: pool.clone() });
    let bon = "email=deux%40exemple.be&mot_de_passe=un-long-mot-de-passe";
    let (_, cookie, _) = poster(&s, "/connexion", bon).await;
    let cookie = cookie.expect("cookie de session");
    let organisation: i64 = sqlx::query_scalar("SELECT organisation_id FROM sessions").fetch_one(&pool).await.unwrap();
    assert_eq!(organisation, 2);
    assert_eq!(lire(&s, "/factures", &cookie).await, StatusCode::OK, "les pages reçoivent l'organisation de la session");

    // Retiré de son organisation : la session se ferme et il ne peut plus se connecter.
    sqlx::query("DELETE FROM membres").execute(&pool).await.unwrap();
    assert_eq!(lire(&s, "/factures", &cookie).await, StatusCode::SEE_OTHER);
    let (statut, cookie, corps) = poster(&s, "/connexion", bon).await;
    assert!(statut == StatusCode::UNAUTHORIZED && cookie.is_none() && corps.contains("rattaché à aucune organisation"));
}
