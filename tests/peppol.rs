// Étape 5-1 : l'adresse Peppol des clients (fiche, ou déduite du numéro de TVA belge) et les contrôles d'une facture
// validée avant son envoi par Peppol.

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

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let r = site.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let statut = r.status();
    (statut, texte(&r.into_body().collect().await.unwrap().to_bytes()))
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    (statut, lieu, texte(&r.into_body().collect().await.unwrap().to_bytes()))
}

/// La page, apostrophes rendues lisibles (« n&#39;est » → « n'est »).
fn texte(octets: &[u8]) -> String {
    String::from_utf8_lossy(octets).replace("&#39;", "'").replace("&#x27;", "'")
}

async fn societe(pool: &PgPool, tva: &str) {
    sqlx::query(
        "INSERT INTO societe (nom, nom_abrege, email, telephone, numero_tva) VALUES ('Ubique SRL', 'U', 'info@ubique.be', '010', $1)
         ON CONFLICT (organisation_id) DO UPDATE SET numero_tva = EXCLUDED.numero_tva",
    )
    .bind(tva)
    .execute(pool)
    .await
    .unwrap();
}

async fn client(pool: &PgPool, tva: Option<&str>, pays: &str, peppol_id: Option<&str>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, numero_tva, pays, peppol_id, rue, numero, code_postal, ville)
         VALUES ('Client SA', '{client}', $1, $2, $3, 'Rue A', '1', '1000', 'Bruxelles') RETURNING id",
    )
    .bind(tva)
    .bind(pays)
    .bind(peppol_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Une facture de 100 € HT à 21 % pour ce client, validée ; renvoie son id.
async fn facture_validee(pool: &PgPool, s: &Router, client: i64) -> i64 {
    let (_, lieu, _) = poster(s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(pool).await.unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference=Travaux&client=e{client}&conditions_paiement_id={condition}")).await;
    poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    let (statut, _, _) = poster(s, &format!("/factures/{id}/valider"), "").await;
    assert!(statut.is_redirection());
    id
}

#[sqlx::test]
async fn un_client_belge_est_pret_et_l_ubl_porte_les_numeros_d_entreprise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool, "BE 0123.456.749").await;
    let s = site(&pool);
    let c = client(&pool, Some("BE0987.654.394"), "BE", None).await;
    let id = facture_validee(&pool, &s, c).await;

    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Facturation électronique") && page.contains("0208:0987654394") && page.contains("Prête pour Peppol"), "{page}");

    // L'UBL émis (archive) donne l'adresse Peppol des deux parties : le numéro d'entreprise, schéma 0208.
    let (statut, ubl) = lire(&s, &format!("/factures/{id}/archive/ubl")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(ubl.contains("<cbc:EndpointID schemeID=\"0208\">0123456749</cbc:EndpointID>"), "{ubl}");
    assert!(ubl.contains("<cbc:EndpointID schemeID=\"0208\">0987654394</cbc:EndpointID>"), "{ubl}");
}

#[sqlx::test]
async fn ce_qui_empeche_l_envoi_est_dit_sur_la_facture(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool, "").await;
    let s = site(&pool);

    // Numéro de TVA du client faux (chiffres de contrôle) et société sans numéro de TVA.
    let faux = client(&pool, Some("BE0987654321"), "BE", None).await;
    let id = facture_validee(&pool, &s, faux).await;
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("À corriger avant l'envoi par Peppol"), "{page}");
    assert!(page.contains("Le numéro de TVA du client (BE0987654321) n'est pas valide"), "{page}");
    assert!(page.contains("Le numéro de TVA belge de votre société manque"), "{page}");
    assert!(!page.contains("Prête pour Peppol"));

    // Client étranger sans identifiant : à saisir sur sa fiche.
    societe(&pool, "BE0123456749").await;
    let etranger = client(&pool, Some("FR12345678901"), "FR", None).await;
    let id = facture_validee(&pool, &s, etranger).await;
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Client hors de Belgique : saisissez son identifiant Peppol"), "{page}");

    // Avec son identifiant sur la fiche, il est prêt.
    sqlx::query("UPDATE entreprises SET peppol_id = '0009:12345678900012' WHERE id = $1").bind(etranger).execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("0009:12345678900012") && page.contains("Prête pour Peppol"), "{page}");

    // Un particulier (sans numéro de TVA) reçoit sa facture par e-mail.
    let particulier = client(&pool, None, "BE", None).await;
    let id = facture_validee(&pool, &s, particulier).await;
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("un particulier ne reçoit pas de facture par Peppol"), "{page}");
    assert!(!page.contains("À corriger avant l'envoi"));
}

#[sqlx::test]
async fn un_brouillon_montre_deja_le_bloc_peppol(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool, "BE0123456749").await;
    let s = site(&pool);
    let (_, lieu, _) = poster(&s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    // Un brouillon pour un client belge : le bloc dit déjà où la facture partira, sans bouton d'envoi (pas de numéro).
    let c = client(&pool, Some("BE0987654394"), "BE", None).await;
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(&pool).await.unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference=Travaux&client=e{c}&conditions_paiement_id={condition}")).await;
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("id=\"peppol\"") && page.contains("0208:0987654394") && page.contains("Prête pour Peppol"), "{page}");
    assert!(!page.contains(&format!("action=\"/factures/{id}/peppol\"")), "pas d'envoi d'un brouillon");
    // Incomplet (aucune ligne) : « Valider… » reste visible, grisé, avec la raison ; le rappel d'enregistrer est là.
    assert!(page.contains("disabled title=\"Complétez d'abord les points du bloc « Avant de valider »\"") && page.contains("Enregistrer les informations » pour qu'elles comptent"), "{page}");
    // Ce qu'il faut corriger se voit aussi avant la validation.
    sqlx::query("UPDATE entreprises SET numero_tva = 'BE0987654321' WHERE id = $1").bind(c).execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("À corriger avant l'envoi par Peppol (vous pouvez déjà valider"), "{page}");
}

#[sqlx::test]
async fn l_identifiant_peppol_se_saisit_sur_la_fiche(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);

    // Mal écrit : refusé, avec l'explication.
    let (statut, _, page) = poster(&s, "/entreprises", "nom=Grossiste&peppol_id=5400000000001").await;
    assert!(!statut.is_redirection(), "{statut}");
    assert!(page.contains("L'identifiant Peppol s'écrit"), "{page}");
    let (_, _, page) = poster(&s, "/entreprises", "nom=Grossiste&peppol_id=0208:0123456789").await;
    assert!(page.contains("numéro d'entreprise belge n'est pas valide"), "{page}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM entreprises").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);

    // Bien écrit : enregistré au propre, pour une entreprise comme pour un contact.
    let (statut, _, page) = poster(&s, "/entreprises", "langue=fr&nom=Grossiste&peppol_id=+0208+%3A+0123.456.749+").await;
    assert!(statut.is_redirection(), "{statut} {page}");
    let enregistre: Option<String> = sqlx::query_scalar("SELECT peppol_id FROM entreprises WHERE nom = 'Grossiste'").fetch_one(&pool).await.unwrap();
    assert_eq!(enregistre.as_deref(), Some("0208:0123456749"));
    let (statut, _, _) = poster(&s, "/contacts", "langue=fr&nom=Dupont&peppol_id=0088:5400000000001").await;
    assert!(statut.is_redirection(), "{statut}");
    let enregistre: Option<String> = sqlx::query_scalar("SELECT peppol_id FROM contacts WHERE nom = 'Dupont'").fetch_one(&pool).await.unwrap();
    assert_eq!(enregistre.as_deref(), Some("0088:5400000000001"));

    // La fiche le réaffiche.
    let id: i64 = sqlx::query_scalar("SELECT id FROM entreprises WHERE nom = 'Grossiste'").fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/entreprises/{id}/modifier")).await;
    assert!(page.contains("value=\"0208:0123456749\""), "{page}");
}
