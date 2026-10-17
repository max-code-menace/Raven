// Tests de l'étape 8-3 : rappels de paiement semi-automatiques (liste des factures à relancer, envoi d'un clic).
// Aucune clé Brevo pendant les tests : les rappels sont seulement enregistrés.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
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

/// Une facture validée de 121,00 € pour `client` (e-mail facultatif), échue depuis `retard` jours (négatif : pas encore échue).
async fn facture(pool: &PgPool, s: &Router, client: &str, email: &str, retard: i32) -> i64 {
    let (_, lieu, _) = poster(s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let e: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils, email) VALUES ($1, '{client}', NULLIF($2, '')) RETURNING id")
        .bind(client)
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(pool).await.unwrap();
    let jour: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}"), &format!("date={jour}&reference=Ref+{client}&client=e{e}&conditions_paiement_id={condition}")).await;
    poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    poster(s, &format!("/factures/{id}/valider"), "").await;
    sqlx::query("UPDATE factures SET date_echeance = (now() AT TIME ZONE 'Europe/Brussels')::date - $1 WHERE id = $2")
        .bind(retard)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn rappels(pool: &PgPool, id: i64) -> Vec<(String, String)> {
    sqlx::query_as("SELECT modele, statut FROM emails WHERE facture_id = $1 ORDER BY id").bind(id).fetch_all(pool).await.unwrap()
}

#[sqlx::test]
async fn les_factures_a_relancer_et_l_envoi_groupe(pool: PgPool) {
    let s = site(&pool);
    let a = facture(&pool, &s, "Alpha SA", "compta@alpha.be", 10).await; // à relancer (premier rappel 7 jours après l'échéance)
    let b = facture(&pool, &s, "Beta SA", "compta@beta.be", 3).await; // en retard, pas encore à relancer
    let c = facture(&pool, &s, "Gamma SA", "", 20).await; // à relancer, mais pas d'e-mail
    let _d = facture(&pool, &s, "Delta SA", "x@delta.be", -5).await; // pas encore échue

    let (code, page) = lire(&s, "/factures/rappels").await;
    assert_eq!(code, StatusCode::OK);
    let (relancer, autres) = page.split_once("Autres factures en retard").unwrap();
    assert!(relancer.contains("Alpha SA") && relancer.contains("Gamma SA") && !relancer.contains("Beta SA"));
    assert!(autres.contains("Beta SA") && !autres.contains("Delta SA"));
    assert!(relancer.contains("« Oubli de paiement ? »") && relancer.contains("pas d&#39;e-mail") || relancer.contains("pas d'e-mail"));
    assert!(relancer.contains(&format!("name=\"facture\" value=\"{a}\" checked")) && !relancer.contains(&format!("value=\"{c}\"")));

    // Rien de coché : refusé.
    let (_, lieu, _) = poster(&s, "/factures/rappels", "").await;
    assert_eq!(lieu.as_deref(), Some("/factures/rappels?erreur=rien"));

    // Envoi groupé : Alpha reçoit le rappel doux (enregistré, mode test), Gamma n'a pas d'e-mail.
    let (_, lieu, _) = poster(&s, "/factures/rappels", &format!("facture={a}&facture={c}")).await;
    assert_eq!(lieu.as_deref(), Some("/factures/rappels?envoyes=0&enregistres=1&echecs=0&sans_email=1"));
    assert_eq!(rappels(&pool, a).await, vec![("rappel_doux".to_string(), "enregistre".to_string())]);
    assert!(rappels(&pool, c).await.is_empty());
    let genre: String = sqlx::query_scalar("SELECT genre FROM facture_evenements WHERE facture_id = $1 ORDER BY id DESC LIMIT 1").bind(a).fetch_one(&pool).await.unwrap();
    assert_eq!(genre, "rappel");
    let (_, page) = lire(&s, "/factures/rappels?envoyes=0&enregistres=1&echecs=0&sans_email=1").await;
    assert!(page.contains("1 rappel(s) enregistré(s) en mode test"));
    // Relancée aujourd'hui : Alpha passe dans « Autres », prochain rappel dans 14 jours.
    let (relancer, autres) = page.split_once("Autres factures en retard").unwrap();
    assert!(!relancer.contains("Alpha SA") && autres.contains("Alpha SA") && autres.contains("1 rappel, le"));

    // Quinze jours plus tard : le rappel suivant est proposé, en version ferme.
    sqlx::query("UPDATE emails SET created_at = created_at - interval '15 days' WHERE facture_id = $1").bind(a).execute(&pool).await.unwrap();
    let (_, page) = lire(&s, "/factures/rappels").await;
    let (relancer, _) = page.split_once("Autres factures en retard").unwrap();
    assert!(relancer.contains("Alpha SA") && relancer.contains("« Rappel de paiement »"));
    poster(&s, "/factures/rappels", &format!("facture={a}")).await;
    assert_eq!(rappels(&pool, a).await.last().unwrap().0, "rappel_ferme");
    let _ = b;
}

#[sqlx::test]
async fn un_rappel_relu_avant_l_envoi(pool: PgPool) {
    let s = site(&pool);
    let id = facture(&pool, &s, "Alpha SA", "compta@alpha.be", 10).await;

    // La fiche de la facture le propose.
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Aucun rappel envoyé.") && page.contains(&format!("href=\"/factures/{id}/rappel\"")));

    let (code, page) = lire(&s, &format!("/factures/{id}/rappel")).await;
    assert_eq!(code, StatusCode::OK);
    let annee: i32 = sqlx::query_scalar("SELECT extract(year FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(&pool).await.unwrap();
    assert!(page.contains(&format!("Oubli de paiement ? Facture n° {annee}-125")) && page.contains("compta@alpha.be") && page.contains("121,00\u{a0}€"));
    let (_, lieu, _) = poster(&s, &format!("/factures/{id}/rappel"), "destinataires=compta%40alpha.be&objet=Oubli+de+paiement&corps=Bonjour").await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{id}?ok=email_enregistre").as_str()));
    assert_eq!(rappels(&pool, id).await, vec![("rappel_doux".to_string(), "enregistre".to_string())]);
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Rappels : 1 rappel, le") && page.contains("Rappel enregistré pour compta@alpha.be"));

    // Payée : plus de rappel possible.
    let methode: i64 = sqlx::query_scalar("SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").fetch_one(&pool).await.unwrap();
    let jour: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=complet&date={jour}&methode_id={methode}")).await;
    assert_eq!(lire(&s, &format!("/factures/{id}/rappel")).await.0, StatusCode::CONFLICT);
    let (_, page) = lire(&s, "/factures/rappels").await;
    assert!(page.contains("Aucune facture à relancer aujourd&#39;hui.") || page.contains("Aucune facture à relancer aujourd'hui."));
}

#[sqlx::test]
async fn les_delais_se_reglent(pool: PgPool) {
    let s = site(&pool);
    let id = facture(&pool, &s, "Alpha SA", "compta@alpha.be", 10).await;
    let (_, lieu, _) = poster(&s, "/parametres/rappels", "premier=15&suivant=10").await;
    assert_eq!(lieu.as_deref(), Some("/parametres/emails?ok=delais#rappels"));
    let (_, page) = lire(&s, "/factures/rappels").await;
    let (relancer, autres) = page.split_once("Autres factures en retard").unwrap();
    assert!(!relancer.contains("Alpha SA") && autres.contains("Alpha SA"), "10 jours de retard < 15 jours");
    let (_, lieu, _) = poster(&s, "/parametres/rappels", "premier=0&suivant=10").await;
    assert_eq!(lieu.as_deref(), Some("/parametres/emails?erreur=delais#rappels"));
    let (_, page) = lire(&s, "/parametres/emails").await;
    assert!(page.contains("value=\"15\"") && page.contains("Rappels de paiement"));
    let _ = id;
}
