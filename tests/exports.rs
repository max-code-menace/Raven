// Tests de l'étape 8-4 : export des factures pour le comptable (ZIP avec PDF, UBL 2.1 et récapitulatif),
// « uniquement non exportées », envoi au comptable (enregistré seulement : pas de clé Brevo pendant les tests).

mod commun;

use std::io::Read;

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

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, Option<String>, Vec<u8>) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let genre = r.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    (statut, lieu, genre, octets)
}

async fn lire(site: &Router, uri: &str) -> String {
    let (_, _, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    String::from_utf8_lossy(&corps).to_string()
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

/// Une facture validée de 121,00 € TTC ; renvoie son id.
async fn facture(pool: &PgPool, s: &Router) -> i64 {
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
    poster(s, &format!("/factures/{id}/valider"), "").await;
    id
}

fn contenu_du_zip(octets: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(octets.to_vec())).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut f = archive.by_index(i).unwrap();
            let mut contenu = Vec::new();
            f.read_to_end(&mut contenu).unwrap();
            (f.name().to_string(), contenu)
        })
        .collect()
}

#[sqlx::test]
async fn un_dossier_d_export_complet(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone, numero_tva) VALUES ('Ubique SRL', 'Ubique', 'info@ubique.be', '010', 'BE0123456789')")
        .execute(&pool)
        .await
        .unwrap();
    let s = site(&pool);
    let page = lire(&s, "/exports").await;
    assert!(page.contains("Aucun export pour l&#39;instant") || page.contains("Aucun export pour l'instant"));
    assert!(page.contains("href=\"/exports\"") && !page.contains("Export factures<small>"), "plus « bientôt » dans le menu");

    let a = facture(&pool, &s).await;
    facture(&pool, &s).await;
    // Une note de crédit pour la première facture.
    let (_, lieu) = poster(&s, &format!("/factures/{a}/crediter"), "").await;
    let note: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/factures/{note}/valider"), "").await;
    // Un brouillon n'est jamais exporté.
    poster(&s, "/factures", "").await;

    let jour = aujourdhui(&pool).await;
    let (_, lieu) = poster(&s, "/exports", &format!("debut={jour}&fin={jour}&format=les_deux&contenu=toutes")).await;
    assert_eq!(lieu.as_deref(), Some("/exports?ok=cree"));
    let (id, nombre): (i64, i32) = sqlx::query_as("SELECT id, nombre_pieces FROM exports").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 3);

    let (code, _, genre, octets) = envoyer(&s, Request::builder().uri(format!("/exports/{id}/telecharger")).body(Body::empty()).unwrap()).await;
    assert_eq!((code, genre.as_deref()), (StatusCode::OK, Some("application/zip")));
    let fichiers = contenu_du_zip(&octets);
    let noms: Vec<&str> = fichiers.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        noms,
        vec![
            "recapitulatif.csv",
            "PDF/Facture-125.pdf",
            "UBL/Facture-125.xml",
            "PDF/Facture-126.pdf",
            "UBL/Facture-126.xml",
            "PDF/Note-de-credit-6.pdf",
            "UBL/Note-de-credit-6.xml"
        ]
    );
    let csv = String::from_utf8(fichiers[0].1.clone()).unwrap();
    assert!(csv.contains("Facture simple;125;") && csv.contains(";100,00;21,00;121,00;") && csv.contains("Note de crédit;6;") && csv.contains(";-121,00;"));
    assert!(fichiers[1].1.starts_with(b"%PDF-"));
    let xml = String::from_utf8(fichiers[2].1.clone()).unwrap();
    // Le numéro part avec l'année : la numérotation repart à 1 chaque année.
    let annee = &jour[..4];
    for attendu in [format!("<cbc:ID>{annee}-125</cbc:ID>"), "<cbc:CompanyID>BE0123456789</cbc:CompanyID>".into(), "<cbc:CompanyID>BE0987654321</cbc:CompanyID>".into(), "<cbc:PayableAmount currencyID=\"EUR\">121.00</cbc:PayableAmount>".into()] {
        assert!(xml.contains(&attendu), "{attendu}");
    }
    let nc = String::from_utf8(fichiers[6].1.clone()).unwrap();
    assert!(nc.contains("<CreditNote") && nc.contains(&format!("<cac:InvoiceDocumentReference><cbc:ID>{annee}-125</cbc:ID>")));

    // Tout est déjà exporté : « uniquement non exportées » ne trouve rien.
    let (_, lieu) = poster(&s, "/exports", &format!("debut={jour}&fin={jour}&format=pdf&contenu=non_exportees")).await;
    assert_eq!(lieu.as_deref(), Some("/exports?erreur=vide"));
    facture(&pool, &s).await;
    poster(&s, "/exports", &format!("debut={jour}&fin={jour}&format=ubl&contenu=non_exportees")).await;
    let (dernier, nombre): (i64, i32) = sqlx::query_as("SELECT id, nombre_pieces FROM exports ORDER BY id DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 1);
    let (_, _, _, octets) = envoyer(&s, Request::builder().uri(format!("/exports/{dernier}/telecharger")).body(Body::empty()).unwrap()).await;
    let noms: Vec<String> = contenu_du_zip(&octets).into_iter().map(|(n, _)| n).collect();
    assert_eq!(noms, vec!["recapitulatif.csv", "UBL/Facture-127.xml"]);

    // Supprimer un dossier : ses pièces redeviennent non exportées.
    let (_, lieu) = poster(&s, &format!("/exports/{id}/supprimer"), "").await;
    assert_eq!(lieu.as_deref(), Some("/exports?ok=supprime"));
    poster(&s, "/exports", &format!("debut={jour}&fin={jour}&format=pdf&contenu=non_exportees")).await;
    let nombre: i32 = sqlx::query_scalar("SELECT nombre_pieces FROM exports ORDER BY id DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 3);
    let page = lire(&s, "/exports").await;
    assert!(page.contains("PDF et UBL 2.1") || page.contains("UBL 2.1"));
}

#[sqlx::test]
async fn demandes_refusees_et_envoi_au_comptable(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    assert_eq!(poster(&s, "/exports", "debut=2026-10-10&fin=2026-10-01&format=pdf&contenu=toutes").await.1.as_deref(), Some("/exports?erreur=periode"));
    assert_eq!(poster(&s, "/exports", "debut=x&fin=2026-10-01&format=pdf&contenu=toutes").await.1.as_deref(), Some("/exports?erreur=periode"));
    assert_eq!(poster(&s, "/exports", "debut=2026-10-01&fin=2026-10-10&format=xls&contenu=toutes").await.1.as_deref(), Some("/exports?erreur=choix"));
    assert_eq!(poster(&s, "/exports", "debut=2026-10-01&fin=2026-10-10&format=pdf&contenu=toutes").await.1.as_deref(), Some("/exports?erreur=vide"));

    facture(&pool, &s).await;
    let jour = aujourdhui(&pool).await;
    poster(&s, "/exports", &format!("debut={jour}&fin={jour}&format=pdf&contenu=toutes")).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM exports").fetch_one(&pool).await.unwrap();

    assert_eq!(poster(&s, &format!("/exports/{id}/envoyer"), "").await.1.as_deref(), Some("/exports?erreur=sans_comptable"));
    assert_eq!(poster(&s, "/exports/comptable", "email=pas-valable").await.1.as_deref(), Some("/exports?erreur=comptable"));
    assert_eq!(poster(&s, "/exports/comptable", "email=compta%40fiduciaire.be").await.1.as_deref(), Some("/exports?ok=comptable"));
    assert_eq!(poster(&s, &format!("/exports/{id}/envoyer"), "").await.1.as_deref(), Some("/exports?ok=enregistre"));
    let (modele, destinataires, pieces, export): (String, String, Vec<String>, Option<i64>) =
        sqlx::query_as("SELECT modele, destinataires, pieces, export_id FROM emails").fetch_one(&pool).await.unwrap();
    assert_eq!((modele.as_str(), destinataires.as_str(), export), ("export", "compta@fiduciaire.be", Some(id)));
    assert_eq!(pieces, vec![format!("Export-factures-{jour}-au-{jour}.zip")]);
    // Mode test : le dossier n'est pas marqué comme envoyé.
    let envoye: Option<String> = sqlx::query_scalar("SELECT envoye_le::text FROM exports WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(envoye, None);
    assert_eq!(poster(&s, "/exports/999999/envoyer", "").await.0, StatusCode::NOT_FOUND);
}
