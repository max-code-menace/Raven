// Tests des changements faits pour retrouver les habitudes de Vertuoza (17/10/2026) : acompte depuis l'onglet
// Informations du devis, « Valider et envoyer » en un clic, bouton « Corriger (note de crédit) » sur une facture
// verrouillée, onglets de la facture, écran « Factures fournisseurs » (achats de tous les chantiers).

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

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, corps)
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    envoyer(site, requete).await
}

/// Un devis envoyé de 1 000,00 € HT à 6 %, avec client et conditions de paiement.
async fn devis_envoye(pool: &PgPool) -> i64 {
    let client: i64 =
        sqlx::query_scalar("INSERT INTO entreprises (nom, profils) VALUES ('Client SA', '{client}') RETURNING id").fetch_one(pool).await.unwrap();
    let devis: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, statut, reference, responsable, montant_ht_cents, client_entreprise_id, conditions_paiement_id)
         VALUES (57, '2026-10-01', 'envoye', 'PL45', 'Max', 100000, $1,
                 (SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30)) RETURNING id",
    )
    .bind(client)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id)
         VALUES ($1, 1, 'poste_libre', 'Carrelage', 100000, 'm²', 100000000, (SELECT id FROM taux_tva WHERE taux_centiemes = 600))",
    )
    .bind(devis)
    .execute(pool)
    .await
    .unwrap();
    devis
}

#[sqlx::test]
async fn un_acompte_depuis_l_onglet_informations_du_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let devis = devis_envoye(&pool).await;
    let (_, page) = lire(&s, &format!("/devis/{devis}")).await;
    assert!(page.contains("id=\"acomptes\"") && page.contains("Les acomptes se facturent une fois le devis accepté"));

    poster(&s, &format!("/devis/{devis}/statut/accepter"), "date=2026-10-05").await;
    let chantier: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/devis/{devis}")).await;
    assert!(page.contains(&format!("action=\"/chantiers/{chantier}/factures/acompte\"")) && page.contains("Ajouter un acompte"));

    // 30 % du devis : la facture d'acompte apparaît dans le bloc du devis.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{chantier}/factures/acompte"), "mode=pourcentage&valeur=30").await;
    assert!(lieu.unwrap().starts_with("/factures/"));
    let (_, page) = lire(&s, &format!("/devis/{devis}")).await;
    assert!(page.contains("Facture d&#39;acompte (brouillon)") || page.contains("Facture d'acompte (brouillon)"));
    assert!(page.contains("300,00\u{a0}€ HT"));
}

#[sqlx::test]
async fn valider_et_envoyer_puis_corriger(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let devis = devis_envoye(&pool).await;
    poster(&s, &format!("/devis/{devis}/statut/accepter"), "date=2026-10-05").await;
    let chantier: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(&pool).await.unwrap();
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{chantier}/factures/acompte"), "mode=montant&valeur=200").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();

    // La fiche a les onglets de Vertuoza et propose « Valider et envoyer par e-mail ».
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    for attendu in ["Parties de la facture", "href=\"#informations\"", "href=\"#lignes\"", "href=\"#totaux-facture\"", "Visualisation", "Valider et envoyer par e-mail"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    let (statut, lieu, _) = poster(&s, &format!("/factures/{id}/valider"), "puis=email").await;
    assert_eq!((statut, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/factures/{id}/email").as_str())), "l'envoi s'ouvre aussitôt");
    let numero: Option<i64> = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(numero.is_some());

    // Verrouillée : le bandeau propose directement la correction par note de crédit.
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Pièce verrouillée") && page.contains("Corriger (note de crédit)"));
    let (_, lieu, _) = poster(&s, &format!("/factures/{id}/crediter"), "").await;
    assert!(lieu.unwrap().ends_with("?ok=credit"));
}

#[sqlx::test]
async fn les_factures_fournisseurs(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let devis = devis_envoye(&pool).await;
    poster(&s, &format!("/devis/{devis}/statut/accepter"), "date=2026-10-05").await;
    let chantier: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(&pool).await.unwrap();

    let (code, page) = lire(&s, "/achats").await;
    assert_eq!(code, StatusCode::OK);
    assert!(page.contains("Factures fournisseurs") && page.contains("Aucun achat saisi") && page.contains("PL45 (n° 1)"));
    assert!(page.contains("href=\"/achats/recues?vue=a_payer\" aria-current=\"page\">À payer"), "rangé dans Paiements › À payer");

    let (_, lieu, _) = poster(&s, "/achats", &format!("chantier={chantier}&date=2026-10-10&montant=1250%2C50&categorie=fourniture&fournisseur=FACQ&description=261684166")).await;
    assert_eq!(lieu.as_deref(), Some("/achats?ok=achat"));
    poster(&s, "/achats", &format!("chantier={chantier}&date=2026-10-11&montant=-50&categorie=fourniture&fournisseur=FACQ&description=Avoir")).await;
    let (_, page) = lire(&s, "/achats?ok=achat").await;
    for attendu in ["FACQ", "261684166", "1\u{a0}250,50\u{a0}€", "1\u{a0}200,50\u{a0}€", "2 achat(s)", "L&#39;achat est enregistré"] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("&#39;", "'")), "{attendu}");
    }
    // L'achat compte dans le chantier.
    let total: i64 = sqlx::query_scalar("SELECT sum(montant_ht_cents)::bigint FROM chantier_achats WHERE chantier_id = $1").bind(chantier).fetch_one(&pool).await.unwrap();
    assert_eq!(total, 120_050);
    // La recherche filtre ; un chantier inconnu est refusé.
    let (_, page) = lire(&s, "/achats?q=Avoir").await;
    assert!(page.contains("1 achat(s)"));
    assert_eq!(poster(&s, "/achats", "chantier=999&date=2026-10-10&montant=10&categorie=autre").await.1.as_deref(), Some("/achats?erreur=achat"));
}
