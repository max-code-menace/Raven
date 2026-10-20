// Numérotation annuelle des devis : le numéro repart à 1 chaque année civile (« 2027-1 »), les devis existants
// gardent leur numéro.

mod commun;

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
use ubifacture::{AppState, app};

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, if lieu.is_empty() { String::from_utf8_lossy(&octets).to_string() } else { lieu })
}

async fn annee_courante(pool: &PgPool) -> i32 {
    sqlx::query_scalar("SELECT EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(pool).await.unwrap()
}

/// Le compteur comme si le dernier devis était le n° `valeur` de l'année `annee`.
async fn regler_compteur(pool: &PgPool, valeur: i64, annee: i32) {
    sqlx::query("UPDATE compteurs SET valeur = $1, annee = $2 WHERE nom = 'devis'").bind(valeur).bind(annee).execute(pool).await.unwrap();
}

/// (numéro, année de la date) du dernier devis créé.
async fn dernier(pool: &PgPool) -> (i64, i32) {
    sqlx::query_as("SELECT numero, EXTRACT(YEAR FROM date_devis)::int FROM devis ORDER BY id DESC LIMIT 1").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn un_nouveau_devis_continue_dans_l_annee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let annee = annee_courante(&pool).await;
    regler_compteur(&pool, 57, annee).await;
    let (statut, _) = poster(&site(&pool), "/devis", "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(dernier(&pool).await, (58, annee));
}

#[sqlx::test]
async fn la_numerotation_repart_a_1_une_nouvelle_annee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let annee = annee_courante(&pool).await;
    // Le dernier devis de l'année précédente était le n° 57 ; il garde son numéro.
    sqlx::query("INSERT INTO devis (numero, date_devis) VALUES (57, make_date($1, 12, 20))").bind(annee - 1).execute(&pool).await.unwrap();
    regler_compteur(&pool, 57, annee - 1).await;

    let site = site(&pool);
    poster(&site, "/devis", "").await;
    assert_eq!(dernier(&pool).await, (1, annee), "le premier devis de l'année est le n° 1");
    poster(&site, "/devis", "").await;
    assert_eq!(dernier(&pool).await, (2, annee));

    // L'ancien devis n'a pas bougé et le compteur est à l'année en cours.
    let ancien: (i64, i32) = sqlx::query_as("SELECT numero, EXTRACT(YEAR FROM date_devis)::int FROM devis WHERE numero = 57").fetch_one(&pool).await.unwrap();
    assert_eq!(ancien, (57, annee - 1));
    let compteur: (i64, Option<i32>) = sqlx::query_as("SELECT valeur, annee FROM compteurs WHERE nom = 'devis'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, (2, Some(annee)));

    // Les deux « n° 1 » de deux années différentes coexistent ; deux dans la même année, jamais.
    sqlx::query("INSERT INTO devis (numero, date_devis) VALUES (1, make_date($1, 3, 1))").bind(annee - 1).execute(&pool).await.unwrap();
    let doublon = sqlx::query("INSERT INTO devis (numero, date_devis) VALUES (1, make_date($1, 6, 1))").bind(annee).execute(&pool).await;
    assert!(doublon.is_err(), "le n° 1 de l'année existe déjà");
}

#[sqlx::test]
async fn dupliquer_un_devis_d_une_annee_passee_donne_le_prochain_numero_de_cette_annee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let annee = annee_courante(&pool).await;
    let ancien: i64 = sqlx::query_scalar("INSERT INTO devis (numero, date_devis) VALUES (57, make_date($1, 12, 20)) RETURNING id")
        .bind(annee - 1)
        .fetch_one(&pool)
        .await
        .unwrap();
    regler_compteur(&pool, 57, annee - 1).await;
    let (statut, lieu) = poster(&site(&pool), &format!("/devis/{ancien}/dupliquer"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER, "{lieu}");
    assert_eq!(dernier(&pool).await, (1, annee), "la copie est datée d'aujourd'hui : 2027-1 après 2026-57");
}

#[sqlx::test]
async fn la_date_d_un_devis_reste_dans_l_annee_de_son_numero(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let annee = annee_courante(&pool).await;
    let id: i64 = sqlx::query_scalar("INSERT INTO devis (numero, date_devis) VALUES (5, make_date($1, 6, 1)) RETURNING id").bind(annee).fetch_one(&pool).await.unwrap();
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(&pool).await.unwrap();
    let conditions: i64 =
        sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7").fetch_one(&pool).await.unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Client SA') RETURNING id").fetch_one(&pool).await.unwrap();
    let formulaire = |date: String| {
        format!(
            "reference=X&responsable=Marc&date_devis={date}&client=e{client}&taux_tva_id={tva}&conditions_paiement_id={conditions}\
             &facturation_identique=on&chantier_identique=on&chantier_pays=BE"
        )
    };
    let site = site(&pool);
    // Une autre date de la même année : acceptée.
    let (statut, _) = poster(&site, &format!("/devis/{id}"), &formulaire(format!("{annee}-02-01"))).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    // Une date d'une autre année : refusée, rien n'est changé.
    let (statut, page) = poster(&site, &format!("/devis/{id}"), &formulaire(format!("{}-02-01", annee + 1))).await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains(&format!("La date doit rester en {annee}")), "{page}");
    let date: String = sqlx::query_scalar("SELECT date_devis::text FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(date, format!("{annee}-02-01"));
}
