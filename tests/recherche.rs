// Recherche partout (audit du design du 23/10/2026) : un numéro, une référence, un client ou un e-mail retrouvent les
// devis, factures, chantiers et contacts de l'organisation, avec un lien vers chacun.

mod commun;
#[path = "commun/donnees.rs"]
mod donnees;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower::ServiceExt;
use ubifacture::{AppState, app};

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let reponse = site.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let statut = reponse.status();
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    (statut, String::from_utf8(octets.to_vec()).unwrap())
}

#[sqlx::test]
async fn la_recherche_trouve_devis_factures_chantiers_et_contacts(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = app(AppState { db: pool.clone() });
    let d = donnees::remplir(&pool, &s).await;

    // Sans rien taper : la page explique quoi chercher, le champ est dans le menu de chaque page.
    let (statut, page) = lire(&s, "/recherche").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Raven cherche partout"));
    assert!(lire(&s, "/devis").await.1.contains("action=\"/recherche\""), "le champ de recherche est dans le menu");

    // Un numéro de devis avec l'année.
    let (_, page) = lire(&s, "/recherche?q=2026-59").await;
    assert!(page.contains("Devis n° 2026-59"), "{page}");
    assert!(page.contains(&format!("href=\"/devis/{}\"", d.devis[6])));

    // Un code de chantier dans la référence : le devis et son chantier.
    let (_, page) = lire(&s, "/recherche?q=L63").await;
    assert!(page.contains(&format!("href=\"/chantiers/{}\"", d.chantier)), "{page}");
    assert!(page.contains("L63 Toiture"));

    // Le nom d'un client : ses factures et sa fiche.
    let (_, page) = lire(&s, "/recherche?q=Lambert").await;
    assert!(page.contains(&format!("href=\"/factures/{}\"", d.facture)), "{page}");
    assert!(page.contains("/contacts/") && page.contains("/modifier\""), "la fiche du contact");

    // Rien trouvé : on le dit, avec une piste.
    let (_, page) = lire(&s, "/recherche?q=zzzzzz").await;
    assert!(page.contains("Rien trouvé pour « zzzzzz »"));
    // Les jokers SQL sont pris à la lettre.
    let (_, page) = lire(&s, "/recherche?q=%25").await;
    assert!(page.contains("Rien trouvé"));
}
