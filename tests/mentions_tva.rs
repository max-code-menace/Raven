// Mentions légales de TVA (6 % rénovation, autoliquidation) : imprimées en entier sur le PDF et dans l'UBL,
// et refus d'une facture qui combine autoliquidation et 6 %.

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
use ubifacture::{AppState, app, mentions_tva};

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    (statut, lieu, octets)
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let (s, _, o) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, o)
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let (s, lieu, _) = envoyer(site, requete).await;
    (s, lieu)
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

async fn societe(pool: &PgPool) {
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone, numero_tva) VALUES ('Ubique SRL', 'Ubique', 'info@ubique.be', '010', 'BE0123456789')")
        .execute(pool)
        .await
        .unwrap();
}

/// Une facture en brouillon avec une ligne par taux demandé (« 21 », « 6 » ou « autoliq »), prête à valider.
async fn brouillon(pool: &PgPool, s: &Router, taux: &[&str]) -> i64 {
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
    for t in taux {
        poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
        let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
        let taux_id: i64 = match *t {
            "autoliq" => sqlx::query_scalar("SELECT id FROM taux_tva WHERE autoliquidation").fetch_one(pool).await.unwrap(),
            n => sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1 AND NOT autoliquidation")
                .bind(n.parse::<i32>().unwrap() * 100)
                .fetch_one(pool)
                .await
                .unwrap(),
        };
        poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux_id}")).await;
    }
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

/// L'UBL de la facture, tel qu'exporté pour le comptable.
async fn ubl_exporte(pool: &PgPool, s: &Router) -> String {
    let jour = aujourdhui(pool).await;
    poster(s, "/exports", &format!("debut={jour}&fin={jour}&format=ubl&contenu=toutes")).await;
    let export: i64 = sqlx::query_scalar("SELECT max(id) FROM exports").fetch_one(pool).await.unwrap();
    let (_, o) = lire(s, &format!("/exports/{export}/telecharger")).await;
    let fichiers = contenu_du_zip(&o);
    let (_, xml) = fichiers.iter().find(|(n, _)| n.ends_with(".xml")).unwrap();
    String::from_utf8(xml.clone()).unwrap()
}

#[sqlx::test]
async fn le_six_pour_cent_porte_sa_mention_complete(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let id = brouillon(&pool, &s, &["6"]).await;
    let (statut, _) = poster(&s, &format!("/factures/{id}/valider"), "").await;
    assert!(statut.is_redirection(), "{statut}");

    let xml = ubl_exporte(&pool, &s).await;
    // Le texte est échappé en XML (l'apostrophe devient &apos;) : on le compare une fois décodé.
    let decode = xml.replace("&apos;", "'").replace("&amp;", "&");
    assert!(decode.contains(mentions_tva::TAUX_6_RENOVATION), "la mention du 6 % entière doit être dans l'UBL");
    assert!(!decode.contains("Autoliquidation :"));

    // Le PDF se génère avec la mention (le modèle Typst l'accepte).
    let (statut, octets) = lire(&s, &format!("/factures/{id}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
}

#[sqlx::test]
async fn l_autoliquidation_porte_sa_mention_complete(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let id = brouillon(&pool, &s, &["autoliq"]).await;
    let (statut, _) = poster(&s, &format!("/factures/{id}/valider"), "").await;
    assert!(statut.is_redirection(), "{statut}");

    let xml = ubl_exporte(&pool, &s).await;
    let decode = xml.replace("&apos;", "'").replace("&amp;", "&");
    assert!(decode.contains(mentions_tva::AUTOLIQUIDATION));
    assert!(!decode.contains("Taux de TVA: En l'absence"));
    let (statut, octets) = lire(&s, &format!("/factures/{id}/pdf")).await;
    assert_eq!((statut, octets.starts_with(b"%PDF-")), (StatusCode::OK, true));
}

#[sqlx::test]
async fn a_21_pour_cent_aucune_mention(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let id = brouillon(&pool, &s, &["21"]).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;
    let xml = ubl_exporte(&pool, &s).await;
    assert!(!xml.contains("Autoliquidation") && !xml.contains("présumé reconnaître"));
}

#[sqlx::test]
async fn autoliquidation_et_six_pour_cent_sur_la_meme_facture_sont_refuses(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let id = brouillon(&pool, &s, &["autoliq", "6"]).await;
    let (statut, _) = poster(&s, &format!("/factures/{id}/valider"), "").await;
    assert!(!statut.is_redirection() && !statut.is_success(), "refusée : {statut}");
    let numero: Option<i64> = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(numero, None, "la facture reste un brouillon, sans numéro");
    // La fiche dit pourquoi.
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(String::from_utf8_lossy(&page).contains("Pas d&#39;autoliquidation et de TVA à 6 %") || String::from_utf8_lossy(&page).contains("Pas d'autoliquidation et de TVA à 6 %"));
}

/// Pour voir la mise en page à l'œil, sans toucher à la vraie base :
/// `$env:RENDU_DOSSIER = "C:\dossier"; cargo test --test mentions_tva -- --ignored`
#[sqlx::test]
#[ignore]
async fn rendu_visuel_des_mentions(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let Ok(dossier) = std::env::var("RENDU_DOSSIER") else { return };
    societe(&pool).await;
    let s = site(&pool);
    for (nom, taux) in [("six-pour-cent", vec!["6"]), ("autoliquidation", vec!["autoliq"])] {
        let id = brouillon(&pool, &s, &taux).await;
        poster(&s, &format!("/factures/{id}/valider"), "").await;
        for (i, png) in ubifacture::facture_pdf::pages_en_png(&mut *pool.acquire().await.unwrap(), id).await.unwrap().into_iter().enumerate() {
            std::fs::write(format!("{dossier}/mention-{nom}-{}.png", i + 1), png).unwrap();
        }
    }
}
