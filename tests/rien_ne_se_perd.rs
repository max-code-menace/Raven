// Rien ne se perd quand on change d'onglet tout de suite (23/10/2026, demande de Mahx : « si on change d'onglet et
// qu'on revient ensuite sur le devis, tout doit être resté tel quel »). Le navigateur abandonne la demande en partant ;
// le serveur doit quand même aller jusqu'au bout de l'enregistrement (`lib::jusqu_au_bout`).

mod commun;
#[path = "commun/donnees.rs"]
mod donnees;

use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, header},
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower::ServiceExt;
use ubifacture::{AppState, app};

#[sqlx::test]
async fn une_ligne_changee_juste_avant_de_quitter_la_page_est_gardee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = app(AppState { db: pool.clone() });
    let d = donnees::remplir(&pool, &s).await;
    let devis = d.devis[6];
    let (ligne, avant): (i64, i64) = sqlx::query_as(
        "SELECT id, prix_unitaire_e5 FROM devis_lignes WHERE devis_id = $1 AND prix_unitaire_e5 IS NOT NULL ORDER BY position LIMIT 1",
    )
    .bind(devis)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(avant, 4_700_000);

    // Le navigateur envoie le nouveau prix… puis quitte la page avant la réponse : la demande est abandonnée dès
    // qu'elle a commencé.
    let requete = Request::builder()
        .method("POST")
        .uri(format!("/devis/{devis}/lignes/{ligne}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from("prix_unitaire=47"))
        .unwrap();
    let abandon = tokio::time::timeout(Duration::ZERO, s.clone().oneshot(requete)).await;
    assert!(abandon.is_err(), "la demande est bien abandonnée avant sa réponse");

    // Le serveur va quand même jusqu'au bout.
    let mut prix = avant;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        prix = sqlx::query_scalar("SELECT prix_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(ligne).fetch_one(&pool).await.unwrap();
        if prix != avant {
            break;
        }
    }
    assert_eq!(prix, 4_700_000, "le prix tapé avant de changer d'onglet est gardé");
}
