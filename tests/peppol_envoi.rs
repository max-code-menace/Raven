// Étape 5-2 : envoi des factures par Peppol, par l'API de Recommand. Les tests parlent à un **faux serveur Recommand**
// (lancé ici, sur la machine) : jamais au vrai.
//
// Le faux serveur répond comme le vrai (relevé dans le bac à sable le 21/10/2026) : une société créée n'est pas
// vérifiée ; un envoi d'une société non vérifiée est refusé (403) ; le destinataire décide de la suite :
//   0208:0987654394 → livré tout de suite ; 0208:0100000070 → en cours, puis livré quand on relit le document ;
//   0208:0100000169 → refusé par la validation (400, règle BR-CO-16).

mod commun;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode, header},
    routing::{get, post},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, application, base::Base, connexion, organisations, peppol};

// ─── Le faux serveur Recommand ──────────────────────────────────────────────

#[derive(Default)]
struct Faux {
    /// Société → vérifiée.
    societes: HashMap<String, bool>,
    /// Les corps reçus par POST /api/v1/companies.
    inscriptions: Vec<Value>,
    /// (société, corps) reçus par POST /api/v1/{société}/send.
    envois: Vec<(String, Value)>,
    /// Documents en cours (livrés quand on les relit).
    en_cours: Vec<String>,
}

type Etat = Arc<Mutex<Faux>>;

fn autorise(entetes: &HeaderMap) -> bool {
    // « cle:secret » en base 64.
    entetes.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some("Basic Y2xlOnNlY3JldA==")
}

fn refus(code: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "success": false, "errors": { "root": [message] } })))
}

async fn creer_societe(State(e): State<Etat>, entetes: HeaderMap, Json(corps): Json<Value>) -> (StatusCode, Json<Value>) {
    if !autorise(&entetes) {
        return refus(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let mut f = e.lock().unwrap();
    let id = format!("c_{}", f.inscriptions.len() + 1 + 1000 * std::process::id() as usize);
    f.societes.insert(id.clone(), false);
    f.inscriptions.push(corps);
    (StatusCode::OK, Json(json!({ "success": true, "company": { "id": id, "isVerified": false }, "verificationUrl": format!("https://faux.recommand/verifier/{id}") })))
}

async fn lire_societe(State(e): State<Etat>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    match e.lock().unwrap().societes.get(&id) {
        Some(v) => (StatusCode::OK, Json(json!({ "success": true, "company": { "id": id, "isVerified": v } }))),
        None => refus(StatusCode::NOT_FOUND, "Company not found"),
    }
}

async fn envoyer_document(State(e): State<Etat>, Path(societe): Path<String>, Json(corps): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut f = e.lock().unwrap();
    if f.societes.get(&societe) != Some(&true) {
        return refus(StatusCode::FORBIDDEN, "Company must be verified before sending or receiving documents. Please verify your company first.");
    }
    f.envois.push((societe.clone(), corps.clone()));
    let id = format!("doc_{}", f.envois.len());
    match corps["recipient"].as_str() {
        Some("0208:0100000169") => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "errors": {
                "root": ["Document validation failed. Please ensure your document complies with all requirements (e.g. EN16931, PEPPOL BIS 3.0, etc.)."],
                "/:Invoice[1]/cac:LegalMonetaryTotal[1]": ["BR-CO-16: Amount due for payment (BT-115) = Invoice total amount with VAT (BT-112)."]
            } })),
        ),
        Some("0208:0100000070") => {
            f.en_cours.push(id.clone());
            (StatusCode::OK, Json(json!({ "success": true, "id": id, "deliveryStatus": "pending", "deliveries": [{ "status": "pending", "failure": null }] })))
        }
        _ => (StatusCode::OK, Json(json!({ "success": true, "id": id, "deliveryStatus": "delivered", "deliveries": [{ "status": "delivered", "failure": null }] }))),
    }
}

async fn lire_document(State(e): State<Etat>, Path(id): Path<String>) -> Json<Value> {
    e.lock().unwrap().en_cours.retain(|d| *d != id);
    Json(json!({ "success": true, "document": { "id": id, "deliveryStatus": "delivered", "deliveries": [{ "status": "delivered", "failure": null }] } }))
}

/// Le faux serveur, lancé une fois pour tout le fichier, et Ubifacture configuré pour lui parler.
fn faux() -> Etat {
    static FAUX: OnceLock<Etat> = OnceLock::new();
    FAUX.get_or_init(|| {
        let etat: Etat = Arc::default();
        let routes = Router::new()
            .route("/api/v1/companies", post(creer_societe))
            .route("/api/v1/companies/{id}", get(lire_societe))
            .route("/api/v1/{societe}/send", post(envoyer_document))
            .route("/api/v1/documents/{id}", get(lire_document))
            .with_state(etat.clone());
        let ecoute = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        ecoute.set_nonblocking(true).unwrap();
        let adresse = format!("http://{}", ecoute.local_addr().unwrap());
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
                axum::serve(tokio::net::TcpListener::from_std(ecoute).unwrap(), routes).await.unwrap();
            })
        });
        peppol::configurer(Some("cle".into()), Some("secret".into()), Some(adresse));
        etat
    })
    .clone()
}

/// La vérification d'identité faite chez Recommand.
fn verifier(etat: &Etat, societe: &str) {
    etat.lock().unwrap().societes.insert(societe.to_string(), true);
}

// ─── Le site ────────────────────────────────────────────────────────────────

struct Reponse {
    statut: StatusCode,
    lieu: Option<String>,
    corps: String,
}

async fn envoyer(site: &Router, methode: &str, chemin: &str, corps: &str, cookie: &str) -> Reponse {
    let mut r = Request::builder().method(methode).uri(chemin).header(header::COOKIE, cookie);
    if methode == "POST" {
        r = r.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let reponse = site.clone().oneshot(r.body(Body::from(corps.to_string())).unwrap()).await.unwrap();
    let statut = reponse.status();
    let lieu = reponse.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    Reponse { statut, lieu, corps: String::from_utf8_lossy(&octets).replace("&#39;", "'") }
}

async fn lire(site: &Router, chemin: &str, cookie: &str) -> Reponse {
    envoyer(site, "GET", chemin, "", cookie).await
}

async fn poster(site: &Router, chemin: &str, corps: &str, cookie: &str) -> Reponse {
    envoyer(site, "POST", chemin, corps, cookie).await
}

/// L'organisation n° 2 (TVA BE0123456749, adresse complète), son propriétaire connecté : le cookie.
async fn organisation(pool: &PgPool, site: &Router) -> String {
    let id = organisations::creer(pool, "Atelier B").await.unwrap();
    connexion::ajouter_utilisateur(pool, id, "b@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    let cookie = connecter(site, "b%40exemple.be").await;
    let demarrage = "nom=Atelier+B&pays=BE&email=b%40exemple.be&telephone=010&numero_tva=BE0123456749&iban=BE68+5390+0754+7034&regime_tva=assujetti&profil_metier=batiment";
    assert_eq!(poster(site, "/demarrage", demarrage, &cookie).await.lieu.as_deref(), Some("/"));
    let mut db = Base::ouvrir(pool, id).await.unwrap();
    sqlx::query("UPDATE societe SET rue = 'Rue des Ateliers', numero = '3', code_postal = '1300', ville = 'Wavre' WHERE organisation_id = app_organisation()")
        .execute(&mut *db)
        .await
        .unwrap();
    cookie
}

async fn connecter(site: &Router, email: &str) -> String {
    let requete = Request::builder()
        .method("POST")
        .uri("/connexion")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("email={email}&mot_de_passe=un-long-mot-de-passe")))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
}

/// Une facture validée de 100 € HT à 21 % pour un client de ce numéro de TVA : son id.
async fn facture(pool: &PgPool, site: &Router, cookie: &str, tva: &str) -> i64 {
    let mut db = Base::ouvrir(pool, 2).await.unwrap();
    let client: i64 = sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, numero_tva, rue, numero, code_postal, ville) VALUES ('Client SA', '{client}', $1, 'Rue A', '1', '1000', 'Bruxelles') RETURNING id",
    )
    .bind(tva)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(&mut *db).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(&mut *db).await.unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&mut *db).await.unwrap();
    let lieu = poster(site, "/factures", "", cookie).await.lieu.unwrap();
    let id: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(site, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference=Travaux&client=e{client}&conditions_paiement_id={condition}"), cookie).await;
    poster(site, &format!("/factures/{id}/lignes"), "genre=poste", cookie).await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&mut *db).await.unwrap();
    poster(site, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}"), cookie).await;
    assert!(poster(site, &format!("/factures/{id}/valider"), "", cookie).await.statut.is_redirection());
    id
}

async fn statut_envoi(pool: &PgPool, facture: i64) -> Vec<(String, String)> {
    let mut db = Base::ouvrir(pool, 2).await.unwrap();
    sqlx::query_as("SELECT statut, erreur FROM envois_peppol WHERE facture_id = $1 ORDER BY id").bind(facture).fetch_all(&mut *db).await.unwrap()
}

// ─── Les tests ──────────────────────────────────────────────────────────────

#[sqlx::test]
async fn inscription_verification_puis_envoi_livre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;

    // Avant l'inscription : la facture ne peut pas partir, la page de réglages propose l'inscription.
    let id = facture(&pool, &s, &cookie, "BE0987654394").await;
    let r = lire(&s, &format!("/factures/{id}"), &cookie).await;
    assert!(r.corps.contains("Prête pour Peppol") && r.corps.contains("inscrivez et vérifiez d'abord votre entreprise"), "{}", r.corps);
    assert!(!r.corps.contains("Envoyer par Peppol"));
    assert_eq!(poster(&s, &format!("/factures/{id}/peppol"), "", &cookie).await.statut, StatusCode::CONFLICT);
    let r = lire(&s, "/parametres/peppol", &cookie).await;
    assert!(r.corps.contains("Inscrire mon entreprise") && r.corps.contains("0208:0123456749"), "{}", r.corps);

    // Inscription : envoi seulement, sous le numéro d'entreprise ; le lien de vérification s'affiche.
    assert_eq!(poster(&s, "/parametres/peppol/inscrire", "", &cookie).await.lieu.as_deref(), Some("/parametres/peppol?ok=inscrite"));
    let societe: String = sqlx::query_scalar("SELECT peppol_societe_id FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    let inscription = etat.lock().unwrap().inscriptions.iter().find(|c| c["name"] == "Atelier B").cloned().unwrap();
    assert_eq!(inscription["enterpriseNumber"], "0123456749");
    assert_eq!(inscription["vatNumber"], "BE0123456749");
    assert_eq!(inscription["isSmpRecipient"], false, "envoi seulement : la réception reste ailleurs");
    assert_eq!((inscription["address"].as_str(), inscription["postalCode"].as_str(), inscription["city"].as_str()), (Some("Rue des Ateliers 3"), Some("1300"), Some("Wavre")));
    let r = lire(&s, "/parametres/peppol", &cookie).await;
    assert!(r.corps.contains("À vérifier") && r.corps.contains(&format!("https://faux.recommand/verifier/{societe}")), "{}", r.corps);
    assert!(!lire(&s, &format!("/factures/{id}"), &cookie).await.corps.contains("Envoyer par Peppol"), "pas vérifiée : pas de bouton");

    // Vérification faite chez Recommand : la page le relit.
    verifier(&etat, &societe);
    let r = lire(&s, "/parametres/peppol", &cookie).await;
    assert!(r.corps.contains("inscrite et vérifiée"), "{}", r.corps);

    // Envoi : l'UBL émis part vers le client, la facture passe à « Envoyée ».
    let r = lire(&s, &format!("/factures/{id}"), &cookie).await;
    assert_eq!(r.corps.matches(&format!("action=\"/factures/{id}/peppol\"")).count(), 2, "bouton dans le bloc et dans la barre du bas");
    let r = poster(&s, &format!("/factures/{id}/peppol"), "", &cookie).await;
    assert_eq!(r.lieu, Some(format!("/factures/{id}?ok=peppol#peppol")));
    let (societe_envoi, corps) = etat.lock().unwrap().envois.iter().find(|(s, _)| *s == societe).cloned().unwrap();
    assert_eq!(societe_envoi, societe);
    assert_eq!((corps["recipient"].as_str(), corps["documentType"].as_str()), (Some("0208:0987654394"), Some("xml")));
    let xml = corps["document"].as_str().unwrap();
    let mut db = Base::ouvrir(&pool, 2).await.unwrap();
    let archive: Vec<u8> = sqlx::query_scalar("SELECT contenu FROM facture_documents WHERE facture_id = $1 AND genre = 'ubl'").bind(id).fetch_one(&mut *db).await.unwrap();
    assert_eq!(xml.as_bytes(), archive.as_slice(), "ce qui part est l'archive légale");
    assert_eq!(statut_envoi(&pool, id).await, vec![("livre".to_string(), String::new())]);
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM factures WHERE id = $1").bind(id).fetch_one(&mut *db).await.unwrap();
    assert_eq!(statut, "envoyee");
    let r = lire(&s, &format!("/factures/{id}?ok=peppol"), &cookie).await;
    assert!(r.corps.contains("La pièce est partie par Peppol.") && r.corps.contains("Livrée") && r.corps.contains("Par Peppol, à 0208:0987654394"), "{}", r.corps);
    assert!(!r.corps.contains("Envoyer par Peppol") && !r.corps.contains("Renvoyer par Peppol"), "déjà partie");
    assert_eq!(poster(&s, &format!("/factures/{id}/peppol"), "", &cookie).await.statut, StatusCode::CONFLICT, "pas deux fois");
}

#[sqlx::test]
async fn valider_et_envoyer_par_peppol_en_un_clic(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;
    poster(&s, "/parametres/peppol/inscrire", "", &cookie).await;
    let societe: String = sqlx::query_scalar("SELECT peppol_societe_id FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    verifier(&etat, &societe);
    lire(&s, "/parametres/peppol", &cookie).await;

    // Un brouillon prêt : le menu « Valider… » propose l'envoi par Peppol.
    let mut db = Base::ouvrir(&pool, 2).await.unwrap();
    let client: i64 = sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, numero_tva, rue, numero, code_postal, ville) VALUES ('Client SA', '{client}', 'BE0987654394', 'Rue A', '1', '1000', 'Bruxelles') RETURNING id",
    )
    .fetch_one(&mut *db)
    .await
    .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(&mut *db).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(&mut *db).await.unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&mut *db).await.unwrap();
    let lieu = poster(&s, "/factures", "", &cookie).await.lieu.unwrap();
    let id: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference=Travaux&client=e{client}&conditions_paiement_id={condition}"), &cookie).await;
    poster(&s, &format!("/factures/{id}/lignes"), "genre=poste", &cookie).await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&mut *db).await.unwrap();
    poster(&s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}"), &cookie).await;
    let r = lire(&s, &format!("/factures/{id}"), &cookie).await;
    assert!(r.corps.contains("Valider et envoyer par Peppol") && r.corps.contains("Elle partira par Peppol une fois validée"), "{}", r.corps);

    // Un clic : validée, numérotée et livrée.
    assert_eq!(poster(&s, &format!("/factures/{id}/valider"), "puis=peppol", &cookie).await.lieu, Some(format!("/factures/{id}?ok=peppol#peppol")));
    assert_eq!(statut_envoi(&pool, id).await, vec![("livre".to_string(), String::new())]);
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM factures WHERE id = $1").bind(id).fetch_one(&mut *db).await.unwrap();
    assert_eq!(statut, "envoyee");
}

#[sqlx::test]
async fn un_refus_est_explique_et_l_envoi_peut_etre_refait(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;
    poster(&s, "/parametres/peppol/inscrire", "", &cookie).await;
    let societe: String = sqlx::query_scalar("SELECT peppol_societe_id FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    verifier(&etat, &societe);
    lire(&s, "/parametres/peppol", &cookie).await;

    let id = facture(&pool, &s, &cookie, "BE0100000169").await;
    let r = poster(&s, &format!("/factures/{id}/peppol"), "", &cookie).await;
    assert_eq!(r.lieu, Some(format!("/factures/{id}?erreur=peppol#peppol")));
    let envois = statut_envoi(&pool, id).await;
    assert_eq!(envois.len(), 1);
    assert_eq!(envois[0].0, "echec");
    assert!(envois[0].1.contains("BR-CO-16"), "{}", envois[0].1);
    let mut db = Base::ouvrir(&pool, 2).await.unwrap();
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM factures WHERE id = $1").bind(id).fetch_one(&mut *db).await.unwrap();
    assert_eq!(statut, "a_envoyer", "un échec ne marque pas la facture envoyée");
    let r = lire(&s, &format!("/factures/{id}?erreur=peppol"), &cookie).await;
    assert!(r.corps.contains("L'envoi par Peppol a échoué") && r.corps.contains("BR-CO-16") && r.corps.contains("Renvoyer par Peppol"), "{}", r.corps);
    assert!(r.corps.contains("Échec de l'envoi par Peppol"), "noté dans l'historique");
}

#[sqlx::test]
async fn un_envoi_en_cours_se_relit(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;
    poster(&s, "/parametres/peppol/inscrire", "", &cookie).await;
    let societe: String = sqlx::query_scalar("SELECT peppol_societe_id FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    verifier(&etat, &societe);
    lire(&s, "/parametres/peppol", &cookie).await;

    let id = facture(&pool, &s, &cookie, "BE0100000070").await;
    poster(&s, &format!("/factures/{id}/peppol"), "", &cookie).await;
    assert_eq!(statut_envoi(&pool, id).await, vec![("en_cours".to_string(), String::new())]);
    let r = lire(&s, &format!("/factures/{id}"), &cookie).await;
    assert!(r.corps.contains("En cours") && r.corps.contains("Actualiser le statut"), "{}", r.corps);

    assert_eq!(poster(&s, &format!("/factures/{id}/peppol/actualiser"), "", &cookie).await.lieu, Some(format!("/factures/{id}#peppol")));
    assert_eq!(statut_envoi(&pool, id).await, vec![("livre".to_string(), String::new())]);
    assert!(!lire(&s, &format!("/factures/{id}"), &cookie).await.corps.contains("Actualiser le statut"));
}

#[sqlx::test]
async fn il_faut_le_numero_de_tva_et_un_proprietaire(options: PgPoolOptions, connexion: PgConnectOptions) {
    faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;

    // Un membre voit la page mais n'inscrit pas l'entreprise.
    connexion::ajouter_utilisateur(&pool, 2, "membre@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    sqlx::query("UPDATE membres SET role = 'membre' WHERE utilisateur_id = (SELECT id FROM utilisateurs WHERE email = 'membre@exemple.be')")
        .execute(&pool)
        .await
        .unwrap();
    let membre = connecter(&s, "membre%40exemple.be").await;
    assert!(lire(&s, "/parametres/peppol", &membre).await.corps.contains("Seul un propriétaire peut inscrire"));
    assert_eq!(poster(&s, "/parametres/peppol/inscrire", "", &membre).await.lieu.as_deref(), Some("/parametres/peppol?erreur=droits"));

    // Sans numéro de TVA belge valide, pas d'inscription (et rien n'est demandé à Recommand).
    let mut db = Base::ouvrir(&pool, 2).await.unwrap();
    sqlx::query("UPDATE societe SET numero_tva = NULL WHERE organisation_id = app_organisation()").execute(&mut *db).await.unwrap();
    let r = poster(&s, "/parametres/peppol/inscrire", "", &cookie).await;
    assert!(r.corps.contains("Il faut le numéro de TVA belge de votre société"), "{}", r.corps);
    let societe: Option<String> = sqlx::query_scalar("SELECT peppol_societe_id FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    assert_eq!(societe, None);
}
