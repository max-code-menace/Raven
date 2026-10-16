// Tests de l'étape 9-4 : statistiques réduites (chiffre d'affaires, encaissé, impayés, conversion des devis).

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;
use ubifacture::{AppState, app};

const E: &str = "\u{a0}€";

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

async fn lire(site: &Router, uri: &str) -> String {
    let r = site.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{uri}");
    String::from_utf8_lossy(&r.into_body().collect().await.unwrap().to_bytes()).to_string()
}

async fn poster(site: &Router, uri: &str, corps: &str) -> Option<String> {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string())
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

/// Une facture validée de 100 € HT (121 € TTC), échue depuis `retard` jours ; renvoie son id.
pub async fn facture(pool: &PgPool, s: &Router, retard: i32) -> i64 {
    let lieu = poster(s, "/factures", "").await.unwrap();
    let id: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils, email) VALUES ('Client SA', '{client}', 'c@client.be') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}"), &format!("date={}&client=e{client}&conditions_paiement_id={condition}", aujourdhui(pool).await)).await;
    poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    poster(s, &format!("/factures/{id}/valider"), "").await;
    sqlx::query("UPDATE factures SET date_echeance = date_echeance - $1 WHERE id = $2").bind(retard).bind(id).execute(pool).await.unwrap();
    id
}

/// Un devis de 1 000 € HT dans le statut demandé (daté d'aujourd'hui) ; renvoie son id.
pub async fn devis(pool: &PgPool, s: &Router, statut: &str) -> i64 {
    let lieu = poster(s, "/devis", "").await.unwrap();
    let id: i64 = lieu.strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    sqlx::query("UPDATE devis SET statut = $1::text::statut_devis, montant_ht_cents = 100000, montant_ttc_cents = 121000 WHERE id = $2")
        .bind(statut)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test]
async fn statistiques_de_la_periode(pool: PgPool) {
    let s = site(&pool);
    let page = lire(&s, "/statistiques").await;
    assert!(page.contains("Statistiques") && page.contains("Chiffre d&#39;affaires") || page.contains("Chiffre d'affaires"));
    assert!(page.contains("href=\"/statistiques\"") && !page.contains("Statistiques<small>"), "plus « bientôt » dans le menu");

    let a = facture(&pool, &s, 0).await;
    facture(&pool, &s, 10).await;
    let methode: i64 = sqlx::query_scalar("SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{a}/paiements"), &format!("genre=partiel&date={}&montant=21&methode_id={methode}", aujourdhui(&pool).await)).await;
    devis(&pool, &s, "accepte").await;
    devis(&pool, &s, "refuse").await;
    devis(&pool, &s, "envoye").await;
    devis(&pool, &s, "brouillon").await; // un brouillon n'est pas « émis »

    let page = lire(&s, "/statistiques?periode=mois").await;
    for attendu in [
        &format!("200,00{E}"),       // chiffre d'affaires HT
        &format!("242,00{E} TTC"),
        &format!("21,00{E}"),        // encaissé
        &format!("121,00{E}"),       // en retard
        "En retard : 1 facture(s)",
        &format!("100,00{E} (1)"),   // pas encore échue : 121 − 21
        "33,33 %",                   // 1 accepté sur 3 émis
        "1 accepté(s) sur 3 devis émis",
        &format!("Panier moyen (accepté)</span><span>1\u{a0}000,00{E}"),
        "Voir le tableau",
    ] {
        assert!(page.contains(attendu), "{attendu}");
    }
    // Une autre période : rien de facturé l'an passé.
    let page = lire(&s, "/statistiques?periode=annee_passee").await;
    assert!(page.contains(&format!("<span class=\"u-grand\">0,00{E}</span>")) && page.contains("0 accepté(s) sur 0 devis émis"));
    // Période personnalisée à l'envers : retombe sur cette année.
    let page = lire(&s, "/statistiques?periode=perso&du=2026-12-31&au=2026-01-01").await;
    assert!(page.contains("<option value=\"annee\" selected>"));
}
