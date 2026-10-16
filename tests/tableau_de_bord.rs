// Tests de l'étape 9-5 : les blocs du tableau de bord et la liste « À surveiller ».

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

/// Une facture validée de 121 € TTC, échue depuis `retard` jours.
async fn facture(pool: &PgPool, s: &Router, retard: i32) -> i64 {
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

async fn devis(pool: &PgPool, s: &Router, statut: &str, montant: i64) -> i64 {
    let lieu = poster(s, "/devis", "").await.unwrap();
    let id: i64 = lieu.strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    sqlx::query("UPDATE devis SET statut = $1::text::statut_devis, montant_ht_cents = $2 WHERE id = $3")
        .bind(statut)
        .bind(montant)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test]
async fn un_tableau_de_bord_vide(pool: PgPool) {
    let page = lire(&site(&pool), "/").await;
    for attendu in ["Mes devis", "Mes factures", "Mes chantiers", "Mes notes de crédit", "À surveiller", "Ce mois-ci", "Rien à signaler"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("arriveront avec les paiements"), "plus de bandeau « bientôt »");
}

#[sqlx::test]
async fn les_blocs_et_ce_qui_est_a_surveiller(pool: PgPool) {
    let s = site(&pool);
    facture(&pool, &s, 10).await; // en retard de 10 jours : à relancer (7 jours)
    facture(&pool, &s, 0).await; // à envoyer, pas en retard
    devis(&pool, &s, "envoye", 150_000).await;
    let vieux = devis(&pool, &s, "envoye", 50_000).await;
    sqlx::query("UPDATE devis SET statut_date = now() - interval '25 days' WHERE id = $1").bind(vieux).execute(&pool).await.unwrap();
    let signe = devis(&pool, &s, "signe", 300_000).await;
    sqlx::query("UPDATE devis SET signe_le = now(), signe_par = 'Anne Claes' WHERE id = $1").bind(signe).execute(&pool).await.unwrap();
    devis(&pool, &s, "brouillon", 80_000).await;

    let page = lire(&s, "/").await;
    for attendu in [
        &format!("5\u{a0}000,00{E}"),                        // en attente : 1 500 + 500 + 3 000
        "En attente de réponse</span> <span class=\"u-petit u-muet\">(3)</span>",
        &format!("Brouillons (1)</a><span>800,00{E}"),
        "En retard</span> <span class=\"u-petit u-muet\">(1)</span>",
        &format!("À envoyer (2)</a><span>242,00{E}"),
        &format!("Reste à encaisser (2)</a><span>242,00{E}"),
        "1 facture(s) en retard à relancer aujourd",
        "est signé en ligne par Anne Claes : il reste à l",
        "est envoyé depuis 25 jours, sans réponse.",
        "2 pièce(s) validée(s) pas encore envoyée(s) au client.",
        "href=\"/factures/rappels\"",
        &format!("Facturé HT</span><b>200,00{E}"),
    ] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("'", "&#39;")), "{attendu}");
    }
}
