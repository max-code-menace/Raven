// Vitrine pour vérifier la mise en page à l'œil, ignorée par `cargo test`.
// Remplit une base TEMPORAIRE avec des données fictives (jamais la vraie base) et sert l'application
// sur http://127.0.0.1:3002 pendant trois heures :
//   cargo test --test vitrine -- --ignored --nocapture

mod commun;
#[path = "commun/donnees.rs"]
mod donnees;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use ubifacture::{AppState, app};

#[sqlx::test]
#[ignore]
async fn vitrine(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = app(AppState { db: pool.clone() });
    let d = donnees::remplir(&pool, &s).await;
    println!("Aperçu d'import : http://127.0.0.1:3002/devis/{}/import/{}", d.devis[6], d.import);
    let ecoute = tokio::net::TcpListener::bind("127.0.0.1:3002").await.expect("port 3002 occupé");
    println!("Pages client : http://127.0.0.1:3002/client/devis/{} et http://127.0.0.1:3002/client/facture/{}", d.jeton_devis, d.jeton_facture);
    println!("Vitrine prête sur http://127.0.0.1:3002 (chantier {}, facture {}, avancement {})", d.chantier, d.facture, d.avancement_brouillon);
    let serveur = axum::serve(ecoute, s);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3 * 3600), serveur).await;
}
