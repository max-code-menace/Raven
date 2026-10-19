// Archive légale (conservation de 10 ans) : le PDF et l'UBL d'une facture validée sont fabriqués une fois pour toutes,
// ne changent plus, et sont ce qui sort de l'application.

use std::io::Read;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;
use ubifacture::{AppState, app, archive};

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, Option<String>, Vec<u8>) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let genre = r.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    (statut, lieu, genre, octets)
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let (s, _, g, o) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, g, o)
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let (s, lieu, _, _) = envoyer(site, requete).await;
    (s, lieu)
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

/// Une facture de 100 € HT à 21 %, en brouillon ; renvoie son id.
async fn brouillon(pool: &PgPool, s: &Router) -> i64 {
    let (_, lieu) = poster(s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let client: i64 = sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, numero_tva, rue, numero, code_postal, ville) VALUES ('Client SA', '{client}', 'BE0987654321', 'Rue A', '1', '1000', 'Bruxelles') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}"), &format!("date={}&reference=Travaux&client=e{client}&conditions_paiement_id={condition}", aujourdhui(pool).await)).await;
    poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    id
}

async fn societe(pool: &PgPool, nom: &str) {
    sqlx::query("INSERT INTO societe (id, nom, nom_abrege, email, telephone, numero_tva) VALUES (1, $1, 'U', 'info@ubique.be', '010', 'BE0123456789')
                 ON CONFLICT (id) DO UPDATE SET nom = EXCLUDED.nom")
        .bind(nom)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test]
async fn la_validation_archive_le_pdf_et_l_ubl(pool: PgPool) {
    societe(&pool, "Ubique SRL").await;
    let s = site(&pool);
    let id = brouillon(&pool, &s).await;

    // Un brouillon n'a pas d'archive.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM facture_documents").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);

    let (statut, _) = poster(&s, &format!("/factures/{id}/valider"), "").await;
    assert!(statut.is_redirection());
    let documents: Vec<(String, String, String)> =
        sqlx::query_as("SELECT genre, nom, empreinte FROM facture_documents WHERE facture_id = $1 ORDER BY genre").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(documents.iter().map(|d| (d.0.as_str(), d.1.as_str())).collect::<Vec<_>>(), vec![("pdf", "Facture-125.pdf"), ("ubl", "Facture-125.xml")]);
    assert!(documents.iter().all(|d| d.2.len() == 64));
    assert!(archive::verifier(&pool, id).await.unwrap().iter().all(|(_, intact)| *intact));

    // La fiche montre l'archive, avec l'empreinte et les liens.
    let (_, _, page) = lire(&s, &format!("/factures/{id}")).await;
    let page = String::from_utf8_lossy(&page).to_string();
    assert!(page.contains("Archive légale") && page.contains(&documents[0].2) && page.contains(&format!("/factures/{id}/archive/pdf")), "{page}");
}

#[sqlx::test]
async fn ce_qui_sort_est_l_archive_meme_si_la_societe_change(pool: PgPool) {
    societe(&pool, "Ubique SRL").await;
    let s = site(&pool);
    let id = brouillon(&pool, &s).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;

    let (_, _, pdf_avant) = lire(&s, &format!("/factures/{id}/pdf")).await;
    let (_, _, ubl_avant) = lire(&s, &format!("/factures/{id}/archive/ubl")).await;
    assert!(String::from_utf8_lossy(&ubl_avant).contains("<cbc:RegistrationName>Ubique SRL</cbc:RegistrationName>"));

    // La société change de nom après l'émission : la facture émise ne bouge pas.
    societe(&pool, "Autre Nom SRL").await;
    let (_, genre, pdf_apres) = lire(&s, &format!("/factures/{id}/pdf")).await;
    assert_eq!((genre.as_deref(), pdf_apres == pdf_avant), (Some("application/pdf"), true), "le PDF servi est l'archive");
    let (statut, genre, ubl_apres) = lire(&s, &format!("/factures/{id}/archive/ubl")).await;
    assert_eq!((statut, genre.as_deref(), ubl_apres == ubl_avant), (StatusCode::OK, Some("application/xml"), true));
    assert!(!String::from_utf8_lossy(&ubl_apres).contains("Autre Nom SRL"));

    // Une nouvelle facture, elle, porte le nouveau nom.
    let b = brouillon(&pool, &s).await;
    poster(&s, &format!("/factures/{b}/valider"), "").await;
    let (_, _, ubl_b) = lire(&s, &format!("/factures/{b}/archive/ubl")).await;
    assert!(String::from_utf8_lossy(&ubl_b).contains("Autre Nom SRL"));

    // Le téléchargement du PDF archivé est un fichier attaché au bon nom.
    let (_, _, reponse) = lire(&s, &format!("/factures/{id}/archive/pdf")).await;
    assert!(reponse.starts_with(b"%PDF-"));
    let (statut, _, _) = lire(&s, &format!("/factures/{id}/archive/xyz")).await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn l_archive_ne_se_modifie_ni_ne_se_supprime(pool: PgPool) {
    societe(&pool, "Ubique SRL").await;
    let s = site(&pool);
    let id = brouillon(&pool, &s).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;

    let modif = sqlx::query("UPDATE facture_documents SET contenu = '\\x00' WHERE facture_id = $1").bind(id).execute(&pool).await;
    assert!(modif.is_err(), "modifier une archive est refusé par la base");
    let suppr = sqlx::query("DELETE FROM facture_documents WHERE facture_id = $1").bind(id).execute(&pool).await;
    assert!(suppr.is_err(), "supprimer une archive est refusé par la base");
    // Seule une facture numérotée a une archive.
    let b = brouillon(&pool, &s).await;
    let ajout = sqlx::query("INSERT INTO facture_documents (facture_id, genre, nom, contenu, empreinte) VALUES ($1, 'pdf', 'x.pdf', '\\x00', repeat('a', 64))")
        .bind(b)
        .execute(&pool)
        .await;
    assert!(ajout.is_err(), "pas d'archive pour un brouillon");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM facture_documents WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 2);
}

#[sqlx::test]
async fn une_facture_validee_sans_archive_s_archive_au_premier_usage(pool: PgPool) {
    societe(&pool, "Ubique SRL").await;
    let s = site(&pool);
    let id = brouillon(&pool, &s).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;
    // Cas d'une facture validée avant cette migration : on simule l'absence d'archive (déclencheur coupé le temps du test).
    sqlx::query("ALTER TABLE facture_documents DISABLE TRIGGER facture_documents_immuables").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM facture_documents").execute(&pool).await.unwrap();
    sqlx::query("ALTER TABLE facture_documents ENABLE TRIGGER facture_documents_immuables").execute(&pool).await.unwrap();

    let (statut, _, pdf) = lire(&s, &format!("/factures/{id}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(pdf.starts_with(b"%PDF-"));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM facture_documents WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 2, "le PDF et l'UBL sont archivés au premier usage");
}

#[sqlx::test]
async fn l_export_sert_les_documents_archives(pool: PgPool) {
    societe(&pool, "Ubique SRL").await;
    let s = site(&pool);
    let id = brouillon(&pool, &s).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;
    societe(&pool, "Autre Nom SRL").await;
    let jour = aujourdhui(&pool).await;
    poster(&s, "/exports", &format!("debut={jour}&fin={jour}&format=les_deux&contenu=toutes")).await;
    let export: i64 = sqlx::query_scalar("SELECT max(id) FROM exports").fetch_one(&pool).await.unwrap();
    let (_, _, octets) = lire(&s, &format!("/exports/{export}/telecharger")).await;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(octets)).unwrap();
    let mut xml = String::new();
    zip.by_name("UBL/Facture-125.xml").unwrap().read_to_string(&mut xml).unwrap();
    assert!(xml.contains("Ubique SRL") && !xml.contains("Autre Nom SRL"), "l'export contient l'UBL émis, pas un UBL refait");
    let archive_xml: Vec<u8> = sqlx::query_scalar("SELECT contenu FROM facture_documents WHERE facture_id = $1 AND genre = 'ubl'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(xml.as_bytes(), archive_xml.as_slice());
}
