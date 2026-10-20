// Test de la reprise des factures de Vertuoza (données fictives).

mod commun;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, app, reprise};

async fn jour(pool: &PgPool, decalage: i32) -> String {
    sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date + $1)::text").bind(decalage).fetch_one(pool).await.unwrap()
}

/// Deux factures de l'année et une note de crédit, comme relevées dans Vertuoza.
async fn fichier(pool: &PgPool) -> String {
    let (d1, e1, d2, e2, p1) = (jour(pool, -40).await, jour(pool, -33).await, jour(pool, -20).await, jour(pool, -13).await, jour(pool, -30).await);
    format!(
        r#"{{"factures": [
  {{"genre": "avancement", "numero": 117, "date": "{d1}", "echeance": "{e1}", "statut": "envoyee",
    "client": {{"nom": "Laurence Exemple", "type": "contact", "email": "laurence@exemple.be"}},
    "reference": "PL45", "chantier": "PL45", "responsable": "Mathieu Draux", "conditions_jours": 7,
    "vcs": "+++000/0000/11785+++",
    "lignes": [{{"genre": "titre", "description": "Toiture"}},
               {{"description": "Avancement 55,04 %", "quantite": "1", "unite": "fft", "prix_unitaire": "3634,47", "tva": 600, "total": "3 634,47"}}],
    "totaux": {{"ht": "3 634,47", "tva": "218,07", "ttc": "3 852,54"}},
    "paiements": [{{"date": "{p1}", "montant": "3852,54", "methode": "Virement"}}]}},
  {{"genre": "simple", "numero": 130, "date": "{d2}", "echeance": "{e2}",
    "client": {{"nom": "Client SA"}}, "reference": "Dépannage",
    "lignes": [{{"description": "Dépannage", "quantite": "1", "unite": "fft", "prix_unitaire": "100", "tva": 2100, "total": "100"}}],
    "totaux": {{"ht": "100", "tva": "21", "ttc": "121"}},
    "paiements": [{{"date": "{p1}", "montant": "21", "methode": "Bancontact"}}]}},
  {{"genre": "note_credit", "numero": 3, "date": "{d2}", "facture_origine": 130, "client": {{"nom": "Client SA"}},
    "reference": "Note de crédit pour la facture n°130",
    "lignes": [{{"description": "Geste commercial", "quantite": "1", "prix_unitaire": "10", "tva": 2100, "total": "10"}}],
    "totaux": {{"ht": "10", "tva": "2,10", "ttc": "12,10"}}}}
]}}"#
    )
}

#[sqlx::test]
async fn reprendre_les_factures_de_vertuoza(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let texte = fichier(&pool).await;

    // Essai : le rapport, mais rien n'est gardé.
    let r = reprise::importer(&pool, 1, &texte, false).await.unwrap();
    assert_eq!((r.factures, r.notes_credit, r.paiements, r.deja, r.enregistre), (2, 1, 2, 0, false));
    assert_eq!(r.clients_crees.len(), 2);
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM factures").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 0, "un essai n'enregistre rien");

    // Enregistrer.
    let r = reprise::importer(&pool, 1, &texte, true).await.unwrap();
    let rapport = r.to_string();
    assert!(rapport.contains("REPRISE ENREGISTRÉE") && rapport.contains("Compteur des factures : 124"), "{rapport}");
    // 3 852,54 payée ; 121,00 − 21,00 payé − 12,10 crédité = 87,90 à recevoir.
    assert_eq!(r.a_recevoir_cents, 8_790);
    let (numero, statut, vcs, reprise, chantier, ttc): (i64, String, Option<String>, bool, String, i64) = sqlx::query_as(
        "SELECT numero, statut::text, vcs, reprise, chantier_vertuoza, montant_ttc_cents FROM factures WHERE genre = 'avancement'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((numero, statut.as_str(), vcs.as_deref(), reprise, chantier.as_str(), ttc), (117, "envoyee", Some("+++000/0000/11785+++"), true, "PL45", 385_254));
    let etat: String = sqlx::query_scalar("SELECT et.etat FROM factures f JOIN factures_etats et ON et.facture_id = f.id WHERE f.numero = 117").fetch_one(&pool).await.unwrap();
    assert_eq!(etat, "payee");
    // Le compteur de factures passe au plus grand numéro repris de l'année ; celui des notes de crédit ne descend pas.
    let compteurs: Vec<(String, i64)> = sqlx::query_as("SELECT nom, valeur FROM compteurs WHERE nom IN ('facture', 'note_credit') ORDER BY nom").fetch_all(&pool).await.unwrap();
    assert_eq!(compteurs, vec![("facture".to_string(), 130), ("note_credit".to_string(), 5)]);

    // La fiche le dit ; la facture reprise n'entre pas dans l'export pour le comptable.
    let id: i64 = sqlx::query_scalar("SELECT id FROM factures WHERE numero = 130").fetch_one(&pool).await.unwrap();
    let r = app(AppState { db: pool.clone() }).oneshot(Request::builder().uri(format!("/factures/{id}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let page = String::from_utf8_lossy(&r.into_body().collect().await.unwrap().to_bytes()).to_string();
    assert!(page.contains("Facture reprise de Vertuoza"));

    // Relancer : rien en double.
    let r = reprise::importer(&pool, 1, &texte, true).await.unwrap();
    assert_eq!((r.factures, r.notes_credit, r.deja), (0, 0, 3));
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM factures").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 3);
}

#[sqlx::test]
async fn une_facture_deja_creee_ici_bloque_tout(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let texte = fichier(&pool).await;
    let d = jour(&pool, -20).await;
    sqlx::query("INSERT INTO factures (genre, numero, statut, date_facture) VALUES ('simple', 130, 'a_envoyer', $1::date)").bind(&d).execute(&pool).await.unwrap();
    let erreur = reprise::importer(&pool, 1, &texte, true).await.unwrap_err();
    assert!(erreur.contains("existe déjà dans Ubifacture"), "{erreur}");
    let reprises: i64 = sqlx::query_scalar("SELECT count(*) FROM factures WHERE reprise").fetch_one(&pool).await.unwrap();
    assert_eq!(reprises, 0, "rien n'est importé");
    assert!(reprise::importer(&pool, 1, "pas du json", false).await.is_err());
}
