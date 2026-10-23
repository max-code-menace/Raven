// Test de l'import des entreprises et des contacts (transition depuis Vertuoza).

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

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, String) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, lieu, String::from_utf8_lossy(&octets).to_string())
}

async fn televerser(site: &Router, uri: &str, octets: &[u8]) -> (StatusCode, Option<String>, String) {
    let limite = "----limite-ubifacture";
    let mut corps = format!("--{limite}\r\nContent-Disposition: form-data; name=\"fichier\"; filename=\"export.csv\"\r\nContent-Type: text/csv\r\n\r\n").into_bytes();
    corps.extend_from_slice(octets);
    corps.extend_from_slice(format!("\r\n--{limite}--\r\n").as_bytes());
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={limite}"))
        .body(Body::from(corps))
        .unwrap();
    envoyer(site, requete).await
}

#[sqlx::test]
async fn importer_entreprises_puis_contacts(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (code, _, page) = envoyer(&s, Request::builder().uri("/contacts").body(Body::empty()).unwrap()).await;
    assert!(code == StatusCode::OK && page.contains("href=\"/contacts/import\""));

    let entreprises = "Nom;Profils;Téléphone;Numéro TVA;E-mail;Adresse\n\
                       Delpire SRL;Architecte;;BE0123456789;info@delpire.be;Rue Haute 3, 1000 Bruxelles 🇧🇪 Belgique\n\
                       CEP asbl;Client;;;;1170 Watermael boitsfort\n";
    let (_, lieu, _) = televerser(&s, "/entreprises/import", entreprises.as_bytes()).await;
    assert_eq!(lieu.as_deref(), Some("/entreprises/import?ajoutees=2&deja=0"));
    let (rue, numero, cp, ville, profils): (String, String, String, String, Vec<String>) =
        sqlx::query_as("SELECT rue, numero, code_postal, ville, profils FROM entreprises WHERE nom = 'Delpire SRL'").fetch_one(&pool).await.unwrap();
    assert_eq!((rue.as_str(), numero.as_str(), cp.as_str(), ville.as_str(), profils), ("Rue Haute", "3", "1000", "Bruxelles", vec!["architecte".to_string()]));

    let contacts = "Nom;Entreprise;Profils;Téléphone;Téléphone secondaire;E-mail;Adresse\n\
                    Pauline Delpire;Delpire SRL;Architecte;0470000000;;pauline@delpire.be;🇧🇪 Belgique\n\
                    Thierry Migeal;;Client;0477446832;;mgl@exemple.be;Rue du beau site 12, 1390 Grez-doiceau 🇧🇪 Belgique\n\
                    Thierry Migeal;;Client;;;mgl@exemple.be;Rue du Beau Site 12, 1390 Grez-Doiceau 🇧🇪 Belgique\n";
    let (_, lieu, _) = televerser(&s, "/contacts/import", contacts.as_bytes()).await;
    assert_eq!(lieu.as_deref(), Some("/contacts/import?ajoutees=2&deja=1"), "le doublon de Vertuoza n'est pas recopié");
    let lie: bool = sqlx::query_scalar("SELECT entreprise_id IS NOT NULL FROM contacts WHERE nom = 'Pauline Delpire'").fetch_one(&pool).await.unwrap();
    assert!(lie, "le contact est relié à son entreprise");

    // Relancer n'ajoute rien ; un e-mail faux est signalé sur la page.
    let (_, lieu, _) = televerser(&s, "/contacts/import", contacts.as_bytes()).await;
    assert_eq!(lieu.as_deref(), Some("/contacts/import?ajoutees=0&deja=3"));
    let (code, _, page) = televerser(&s, "/contacts/import", "Nom;E-mail\nAnne Claes;pas-un-mail\n".as_bytes()).await;
    assert!(code == StatusCode::OK && page.contains("1\u{a0}fiche ajoutée") && page.contains("pas-un-mail"));
    let (code, _, _) = televerser(&s, "/contacts/import", b"a;b\n1;2\n").await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
}
