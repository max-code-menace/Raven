// Tests des onglets Totaux (ajustement) et Visualisation (aperçu).

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
    (statut, lieu, String::from_utf8(octets.to_vec()).unwrap())
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

async fn taux(pool: &PgPool, centiemes: i32) -> i64 {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(centiemes).fetch_one(pool).await.unwrap()
}

/// Un brouillon avec deux postes : 600 € à 6 % et 400 € à 21 %.
async fn devis_a_deux_taux(pool: &PgPool) -> i64 {
    let (reduit, normal) = (taux(pool, 600).await, taux(pool, 2100).await);
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero, taux_tva_id) VALUES (1, $1) RETURNING id").bind(reduit).fetch_one(pool).await.unwrap();
    for (position, prix, t) in [(1, 6_000_000_000_i64 / 100, reduit), (2, 4_000_000_000_i64 / 100, normal)] {
        sqlx::query(
            "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, prix_unitaire_e5, taux_tva_id)
             VALUES ($1, $2, 'poste_libre', 'Poste', 100000, $3, $4)",
        )
        .bind(d)
        .bind(position)
        .bind(prix)
        .bind(t)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE devis SET montant_ht_cents = 100000, montant_ttc_cents = 100000 + 3600 + 8400 WHERE id = $1").bind(d).execute(pool).await.unwrap();
    d
}

async fn montants(pool: &PgPool, devis: i64) -> (i64, i64) {
    sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(devis).fetch_one(pool).await.unwrap()
}

// ─── Onglet Totaux ──────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_page_totaux_montre_les_montants_par_taux(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/totaux")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Sous-total HT") && page.contains("1\u{a0}000,00\u{a0}€"));
    assert!(page.contains("600,00\u{a0}€") && page.contains("36,00\u{a0}€") && page.contains("84,00\u{a0}€"));
    assert!(page.contains("1\u{a0}120,00\u{a0}€"));
    assert!(page.contains(&format!("tab tab-active\" href=\"/devis/{d}/totaux\"")));
    assert_eq!(lire(&site(&pool), "/devis/999/totaux").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn une_remise_en_pourcentage_se_repartit_et_met_le_devis_a_jour(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    let (statut, lieu, _) = poster(&s, &format!("/devis/{d}/ajustement"), "libelle=Remise+fid%C3%A9lit%C3%A9&sens=remise&mode=pourcentage&valeur=10").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu, Some(format!("/devis/{d}/totaux")));

    // 1 000 € − 10 % = 900 € : bases 540 € (6 %) et 360 € (21 %) ; TVA 32,40 + 75,60 ; TTC 1 008 €
    assert_eq!(montants(&pool, d).await, (90_000, 90_000 + 3_240 + 7_560));
    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    assert!(page.contains("Remise fidélité") && page.contains("-100,00\u{a0}€"));
    assert!(page.contains("540,00\u{a0}€") && page.contains("360,00\u{a0}€") && page.contains("1\u{a0}008,00\u{a0}€"));
    // Le formulaire montre l'ajustement enregistré.
    assert!(page.contains("value=\"Remise fidélité\"") && page.contains("value=\"10\""));
    assert!(page.contains("Retirer l'ajustement"));
}

#[sqlx::test]
async fn une_majoration_en_montant(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/ajustement"), "libelle=Frais&sens=majoration&mode=montant&valeur=50").await;
    // +50 € répartis 30 € / 20 € : bases 630 € et 420 € ; TVA 37,80 + 88,20 = 126 €
    assert_eq!(montants(&pool, d).await, (105_000, 105_000 + 3_780 + 8_820));
}

#[sqlx::test]
async fn l_ajustement_suit_les_changements_de_lignes(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/ajustement"), "sens=remise&mode=pourcentage&valeur=10").await;
    // On change un prix dans l'encodage : la remise de 10 % s'applique au nouveau sous-total.
    let ligne: i64 = sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 ORDER BY position LIMIT 1").bind(d).fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "quantite=1&prix_unitaire=1600").await;
    // sous-total 2 000 € (1 600 + 400) ; remise 200 € ; HT 1 800 €
    assert_eq!(montants(&pool, d).await.0, 180_000);
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("Sous-total HT") && page.contains("Ajustement") && page.contains("-200,00\u{a0}€"));
}

#[sqlx::test]
async fn retirer_l_ajustement_remet_les_totaux(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/ajustement"), "sens=remise&mode=montant&valeur=100").await;
    assert_eq!(montants(&pool, d).await.0, 90_000);
    let (statut, _, _) = poster(&s, &format!("/devis/{d}/ajustement/retirer"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(montants(&pool, d).await, (100_000, 112_000));
    let (mode, valeur): (String, i64) = sqlx::query_as("SELECT ajustement_mode, ajustement_valeur FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((mode.as_str(), valeur), ("aucun", 0));
}

#[sqlx::test]
async fn un_ajustement_invalide_est_explique_et_rien_n_est_enregistre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    for (corps, message) in [
        ("sens=remise&mode=pourcentage&valeur=150", "Une remise ne peut pas dépasser 100 %."),
        ("sens=remise&mode=montant&valeur=1500", "La remise dépasse le sous-total des lignes"),
        ("sens=remise&mode=montant&valeur=abc", "supérieur à 0"),
        ("sens=remise&mode=montant&valeur=0", "supérieur à 0"),
        ("sens=remise&mode=montant&valeur=5,123", "2 décimales"),
        ("sens=nimporte&mode=montant&valeur=5", "Choisissez remise ou majoration."),
        ("sens=remise&mode=autre&valeur=5", "Choisissez pourcentage ou montant."),
    ] {
        let (statut, _, page) = poster(&s, &format!("/devis/{d}/ajustement"), corps).await;
        assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY, "{corps}");
        assert!(page.contains(message), "{corps}");
    }
    assert_eq!(montants(&pool, d).await, (100_000, 112_000));
    let (mode,): (String,) = sqlx::query_as("SELECT ajustement_mode FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(mode, "aucun");
}

#[sqlx::test]
async fn la_valeur_tapee_revient_apres_une_erreur(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let (_, _, page) = poster(&site(&pool), &format!("/devis/{d}/ajustement"), "libelle=Mon+geste&sens=majoration&mode=montant&valeur=abc").await;
    assert!(page.contains("value=\"Mon geste\"") && page.contains("value=\"abc\""));
    assert!(page.contains("<option value=\"majoration\" selected>") && page.contains("<option value=\"montant\" selected>"));
}

#[sqlx::test]
async fn un_devis_envoye_n_a_plus_d_ajustement_modifiable(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    sqlx::query("UPDATE devis SET statut = 'accepte' WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/devis/{d}/ajustement"), "sens=remise&mode=montant&valeur=100").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/devis/{d}/ajustement/retirer"), "").await.0, StatusCode::CONFLICT);
    assert_eq!(montants(&pool, d).await, (100_000, 112_000));
    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    assert!(page.contains("Ce devis n'est plus un brouillon") && !page.contains("Enregistrer l'ajustement"));
}

#[sqlx::test]
async fn la_base_refuse_un_ajustement_incoherent(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_a_deux_taux(&pool).await;
    let res = sqlx::query("UPDATE devis SET ajustement_mode = 'aucun', ajustement_valeur = 500 WHERE id = $1").bind(d).execute(&pool).await;
    assert!(res.is_err());
    let res = sqlx::query("UPDATE devis SET ajustement_mode = 'inconnu' WHERE id = $1").bind(d).execute(&pool).await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn les_quatre_onglets_sont_relies(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}")).await;
    for lien in [format!("href=\"/devis/{d}\""), format!("href=\"/devis/{d}/encodage\""), format!("href=\"/devis/{d}/totaux\""), format!("href=\"/devis/{d}/apercu\"")] {
        assert!(page.contains(&lien), "{lien}");
    }
}
