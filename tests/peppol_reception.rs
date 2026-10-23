// Étape 5-3 : réception des factures fournisseurs par Peppol. Les tests parlent à un **faux serveur Recommand** (lancé
// ici, sur la machine) : jamais au vrai.
//
// Le faux serveur répond avec les formes documentées par Recommand (https://docs.recommand.eu, 21/10/2026) : boîte de
// réception (`GET /api/v1/inbox`), document lu (`GET /api/v1/documents/{id}` : `parsed` et `xml`), « lu »
// (`POST …/mark-as-read`), rendu PDF, mise à jour de la société (`isSmpRecipient`), identifiants et clé de migration.

mod commun;

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, application, base::Base, connexion, organisations, peppol};

const SECRET_WEBHOOK: &str = "secret-du-webhook";

// ─── Le faux serveur Recommand ──────────────────────────────────────────────

#[derive(Default)]
struct Faux {
    /// Société → vérifiée.
    societes: HashMap<String, bool>,
    /// (société, corps) reçus par POST /api/v1/companies.
    inscriptions: Vec<(String, Value)>,
    /// (société, corps) reçus par PUT /api/v1/companies/{id}.
    mises_a_jour: Vec<(String, Value)>,
    /// (société, clé) reçues pour une migration.
    migrations: Vec<(String, String)>,
    /// La prochaine inscription en réception est refusée (déjà inscrite ailleurs).
    deja_ailleurs: bool,
    /// Les documents reçus, et ceux encore non lus.
    documents: HashMap<String, Value>,
    non_lus: HashSet<String>,
}

type Etat = Arc<Mutex<Faux>>;

fn refus(code: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "success": false, "errors": { "root": [message] } })))
}

async fn creer_societe(State(e): State<Etat>, Json(corps): Json<Value>) -> Json<Value> {
    let mut f = e.lock().unwrap();
    let id = format!("c_r{}_{}", std::process::id(), f.inscriptions.len() + 1);
    f.societes.insert(id.clone(), false);
    f.inscriptions.push((id.clone(), corps));
    Json(json!({ "success": true, "company": { "id": id, "isVerified": false }, "verificationUrl": format!("https://faux.recommand/verifier/{id}") }))
}

async fn lire_societe(State(e): State<Etat>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    match e.lock().unwrap().societes.get(&id) {
        Some(v) => (StatusCode::OK, Json(json!({ "success": true, "company": { "id": id, "isVerified": v } }))),
        None => refus(StatusCode::NOT_FOUND, "Company not found"),
    }
}

async fn modifier_societe(State(e): State<Etat>, Path(id): Path<String>, Json(corps): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut f = e.lock().unwrap();
    if corps["isSmpRecipient"] == true && f.deja_ailleurs {
        return refus(StatusCode::BAD_REQUEST, "This participant is already registered for receiving at another SMP (smp.autre.example).");
    }
    f.mises_a_jour.push((id.clone(), corps));
    (StatusCode::OK, Json(json!({ "success": true, "company": { "id": id } })))
}

async fn identifiants(Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "success": true, "identifiers": [
        { "id": "ci_9925", "companyId": id, "scheme": "9925", "identifier": "BE0123456749" },
        { "id": "ci_0208", "companyId": id, "scheme": "0208", "identifier": "0123456749" }
    ] }))
}

async fn migrer(State(e): State<Etat>, Path((id, identifiant)): Path<(String, String)>, Json(corps): Json<Value>) -> Json<Value> {
    assert_eq!(identifiant, "ci_0208", "la clé vaut pour le numéro d'entreprise");
    e.lock().unwrap().migrations.push((id.clone(), corps["migrationKey"].as_str().unwrap_or_default().to_string()));
    Json(json!({ "success": true, "migration": { "id": "pm_1", "companyId": id, "scheme": "0208", "status": "pending", "errorMessage": null } }))
}

async fn boite(State(e): State<Etat>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let f = e.lock().unwrap();
    let societe = q.get("companyId").cloned().unwrap_or_default();
    let documents: Vec<Value> = f
        .non_lus
        .iter()
        .filter_map(|id| f.documents.get(id))
        .filter(|d| d["companyId"] == societe.as_str())
        .map(|d| json!({ "id": d["id"], "companyId": d["companyId"], "direction": d["direction"], "type": d["type"], "readAt": null }))
        .collect();
    Json(json!({ "success": true, "documents": documents }))
}

async fn lire_document(State(e): State<Etat>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    match e.lock().unwrap().documents.get(&id) {
        Some(d) => (StatusCode::OK, Json(json!({ "success": true, "document": d }))),
        None => refus(StatusCode::NOT_FOUND, "Document not found"),
    }
}

async fn marquer_lu(State(e): State<Etat>, Path(id): Path<String>) -> Json<Value> {
    e.lock().unwrap().non_lus.remove(&id);
    Json(json!({ "success": true }))
}

async fn rendu_pdf() -> Vec<u8> {
    b"%PDF-1.7 rendu par Recommand".to_vec()
}

fn faux() -> Etat {
    static FAUX: OnceLock<Etat> = OnceLock::new();
    FAUX.get_or_init(|| {
        let etat: Etat = Arc::default();
        let routes = Router::new()
            .route("/api/v1/companies", post(creer_societe))
            .route("/api/v1/companies/{id}", get(lire_societe).put(modifier_societe))
            .route("/api/v1/companies/{id}/identifiers", get(identifiants))
            .route("/api/v1/companies/{id}/identifiers/{identifiant}/migration", post(migrer))
            .route("/api/v1/inbox", get(boite))
            .route("/api/v1/documents/{id}", get(lire_document))
            .route("/api/v1/documents/{id}/mark-as-read", post(marquer_lu))
            .route("/api/v1/documents/{id}/render/pdf", get(rendu_pdf))
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
        peppol::configurer_webhook(Some(SECRET_WEBHOOK.into()));
        etat
    })
    .clone()
}

/// Une facture (ou note de crédit) que « Matériaux SA » envoie à la société, déposée dans sa boîte chez Recommand.
fn deposer(etat: &Etat, id: &str, societe: &str, genre: &str, numero: &str, ht: &str, tva: &str, ttc: &str, avec_pdf: bool) {
    let aujourdhui = chrono::Local::now().date_naive().to_string();
    let mut parsed = json!({
        "issueDate": aujourdhui, "dueDate": null, "note": "Livraison chantier",
        "seller": { "name": "Matériaux SA", "vatNumber": "BE0987654394", "street": "Rue du Dépôt 4", "city": "Wavre", "postalZone": "1300", "country": "BE" },
        "buyer": { "name": "Atelier B", "vatNumber": "BE0123456749", "street": "Rue des Ateliers 3", "city": "Wavre", "postalZone": "1300", "country": "BE" },
        "lines": [{ "name": "Ciment", "description": "Sacs de 25 kg", "quantity": "10.00", "unitCode": "KGM", "netPriceAmount": "12.40", "netAmount": ht, "vat": { "category": "S", "percentage": "21.00" } }],
        "totals": { "taxExclusiveAmount": ht, "taxInclusiveAmount": ttc, "payableAmount": ttc },
        "vat": { "totalVatAmount": tva, "subtotals": [{ "taxableAmount": ht, "vatAmount": tva, "category": "S", "percentage": "21.00" }] },
        "paymentMeans": [{ "paymentMethod": "credit_transfer", "iban": "BE68539007547034", "reference": "123456789002" }],
        "attachments": []
    });
    if genre == "invoice" {
        parsed["invoiceNumber"] = json!(numero);
    } else {
        parsed["creditNoteNumber"] = json!(numero);
    }
    if avec_pdf {
        parsed["attachments"] = json!([{ "id": "1", "filename": "facture.pdf", "mimeCode": "application/pdf",
            "embeddedDocument": base64::engine::general_purpose::STANDARD.encode(format!("%PDF-1.4 facture {numero}")) }]);
    }
    let doc = json!({
        "id": id, "companyId": societe, "direction": "incoming", "type": genre, "senderId": "0208:0987654394", "receiverId": "0208:0123456749",
        "createdAt": "2026-10-21T08:15:00.000Z", "readAt": null, "xml": format!("<?xml version=\"1.0\"?><Invoice><ID>{numero}</ID></Invoice>"),
        "parsed": parsed, "validation": { "result": "valid", "errors": [] }
    });
    let mut f = etat.lock().unwrap();
    f.documents.insert(id.to_string(), doc);
    f.non_lus.insert(id.to_string());
}

/// Un accusé de réception Peppol (réponse à une facture envoyée) : rien à garder, marqué lu.
fn deposer_accuse(etat: &Etat, id: &str, societe: &str) {
    let mut f = etat.lock().unwrap();
    f.documents.insert(id.to_string(), json!({ "id": id, "companyId": societe, "direction": "incoming", "type": "messageLevelResponse", "xml": "<ApplicationResponse/>", "parsed": {} }));
    f.non_lus.insert(id.to_string());
}

// ─── Le site ────────────────────────────────────────────────────────────────

struct Reponse {
    statut: StatusCode,
    lieu: Option<String>,
    octets: Vec<u8>,
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
    let octets = reponse.into_body().collect().await.unwrap().to_bytes().to_vec();
    let corps = String::from_utf8_lossy(&octets).replace("&#39;", "'").replace("&quot;", "\"");
    Reponse { statut, lieu, octets, corps }
}

async fn lire(site: &Router, chemin: &str, cookie: &str) -> Reponse {
    envoyer(site, "GET", chemin, "", cookie).await
}

async fn poster(site: &Router, chemin: &str, corps: &str, cookie: &str) -> Reponse {
    envoyer(site, "POST", chemin, corps, cookie).await
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

/// L'organisation n° 2 (bâtiment, TVA BE0123456749, adresse complète), son propriétaire connecté : le cookie.
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

/// Inscrit l'organisation n° 2 (avec ou sans la réception) et la vérifie : l'id de sa société chez Recommand.
async fn inscrire(pool: &PgPool, site: &Router, etat: &Etat, cookie: &str, recevoir: bool) -> String {
    let corps = if recevoir { "recevoir=on" } else { "" };
    assert_eq!(poster(site, "/parametres/peppol/inscrire", corps, cookie).await.lieu.as_deref(), Some("/parametres/peppol?ok=inscrite"));
    let societe: String = sqlx::query_scalar("SELECT peppol_societe_id FROM organisations WHERE id = 2").fetch_one(pool).await.unwrap();
    etat.lock().unwrap().societes.insert(societe.clone(), true);
    lire(site, "/parametres/peppol", cookie).await;
    societe
}

async fn un<T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin>(pool: &PgPool, requete: &str) -> T {
    let mut db = Base::ouvrir(pool, 2).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(requete.to_string())).fetch_one(&mut *db).await.unwrap()
}

// ─── Les tests ──────────────────────────────────────────────────────────────

#[sqlx::test]
async fn reception_releve_rattachement_et_export(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;

    // Bâtiment : la case « Recevoir aussi mes factures fournisseurs » est cochée d'avance, avec l'avertissement.
    let r = lire(&s, "/parametres/peppol", &cookie).await;
    assert!(r.corps.contains("name=\"recevoir\" value=\"on\" checked") && r.corps.contains("un seul point d'accès"), "{}", r.corps);

    // Inscription avec la réception : `isSmpRecipient: true`, la réception est allumée, le menu montre la boîte.
    let societe = inscrire(&pool, &s, &etat, &cookie, true).await;
    let corps = etat.lock().unwrap().inscriptions.iter().find(|(id, _)| *id == societe).cloned().unwrap().1;
    assert_eq!(corps["isSmpRecipient"], true);
    assert!(un::<bool>(&pool, "SELECT peppol_reception FROM organisations WHERE id = 2").await);
    assert!(lire(&s, "/parametres/peppol", &cookie).await.corps.contains("Allumée"));

    // Deux pièces de Matériaux SA arrivent (une facture avec son PDF, une note de crédit sans PDF) et un accusé.
    deposer(&etat, &format!("doc_{societe}_1"), &societe, "invoice", "F-2026-77", "124.00", "26.04", "150.04", true);
    deposer(&etat, &format!("doc_{societe}_2"), &societe, "creditNote", "NC-3", "20.00", "4.20", "24.20", false);
    deposer_accuse(&etat, &format!("doc_{societe}_3"), &societe);
    // Un document d'une autre société de l'équipe n'est jamais pris.
    deposer(&etat, "doc_autre", "c_autre", "invoice", "X-1", "10.00", "2.10", "12.10", false);

    // Ouvrir la boîte relève : deux pièces enregistrées, tout est marqué lu chez Recommand.
    let r = lire(&s, "/achats/recues", &cookie).await;
    assert_eq!(r.statut, StatusCode::OK);
    assert!(r.corps.contains("Matériaux SA") && r.corps.contains("F-2026-77") && r.corps.contains("NC-3") && r.corps.contains("À traiter"), "{}", r.corps);
    assert!(!r.corps.contains("X-1"));
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 2);
    let non_lus = etat.lock().unwrap().non_lus.clone();
    assert!(!non_lus.iter().any(|d| d.starts_with(&format!("doc_{societe}_"))), "tout est lu : {non_lus:?}");
    assert!(non_lus.contains("doc_autre"), "le document d'une autre société reste non lu");
    // Le fournisseur est créé une fois, retrouvé ensuite par son numéro de TVA.
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM entreprises WHERE numero_tva = 'BE0987654394'").await, 1);
    assert_eq!(un::<i64>(&pool, "SELECT count(DISTINCT fournisseur_id) FROM factures_recues").await, 1);
    // Relever à nouveau : rien de plus (et un document relu par erreur ne fait pas de doublon).
    etat.lock().unwrap().non_lus.insert(format!("doc_{societe}_1"));
    let r = poster(&s, "/achats/recues/relever", "", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/achats/recues?ok=releve&nouvelles=0"));
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 2);
    assert!(lire(&s, "/", &cookie).await.corps.contains("2\u{a0}factures fournisseurs reçues par Peppol à traiter"));

    // La facture : fournisseur, lignes, montants, PDF du fournisseur, XML tel quel.
    let facture: i64 = un(&pool, "SELECT id FROM factures_recues WHERE numero = 'F-2026-77'").await;
    let note: i64 = un(&pool, "SELECT id FROM factures_recues WHERE numero = 'NC-3'").await;
    let r = lire(&s, &format!("/achats/recues/{facture}"), &cookie).await;
    for attendu in ["Matériaux SA", "BE0987654394", "Ciment", "Sacs de 25 kg", "150,04", "26,04", "BE68539007547034", "Conforme", "Reste à répartir : 124,00"] {
        assert!(r.corps.contains(attendu), "« {attendu} » manque : {}", r.corps);
    }
    assert_eq!(lire(&s, &format!("/achats/recues/{facture}/pdf"), &cookie).await.octets, b"%PDF-1.4 facture F-2026-77");
    assert_eq!(lire(&s, &format!("/achats/recues/{note}/pdf"), &cookie).await.octets, b"%PDF-1.7 rendu par Recommand", "sans PDF joint : le rendu");
    let xml = lire(&s, &format!("/achats/recues/{facture}/xml"), &cookie).await;
    assert_eq!(xml.corps, "<?xml version=\"1.0\"?><Invoice><ID>F-2026-77</ID></Invoice>");
    let empreinte: String = un(&pool, &format!("SELECT empreinte FROM factures_recues WHERE id = {facture}")).await;
    assert_eq!(empreinte, hex::encode(Sha256::digest(xml.octets)));

    // L'archive ne se modifie ni ne se supprime.
    let mut db = Base::ouvrir(&pool, 2).await.unwrap();
    assert!(sqlx::query("UPDATE factures_recues SET xml = 'faux' WHERE id = $1").bind(facture).execute(&mut *db).await.is_err());
    assert!(sqlx::query("UPDATE factures_recues SET montant_ht_cents = 1 WHERE id = $1").bind(facture).execute(&mut *db).await.is_err());
    assert!(sqlx::query("DELETE FROM factures_recues WHERE id = $1").bind(facture).execute(&mut *db).await.is_err());

    // Un chantier en cours.
    let devis: i64 = sqlx::query_scalar("INSERT INTO devis (numero, date_devis, reference, statut) VALUES (1, current_date, 'Villa Dupont', 'accepte') RETURNING id")
        .fetch_one(&mut *db)
        .await
        .unwrap();
    let chantier: i64 = sqlx::query_scalar("INSERT INTO chantiers (numero, devis_id, date_debut) VALUES (1, $1, current_date) RETURNING id")
        .bind(devis)
        .fetch_one(&mut *db)
        .await
        .unwrap();

    // Répartie : 100 € sur le chantier, le reste (24 €) sans chantier. Un montant trop grand est refusé.
    let r = poster(&s, &format!("/achats/recues/{facture}/rattacher"), &format!("destination={chantier}&montant=200&categorie=fourniture"), &cookie).await;
    assert_eq!(r.lieu, Some(format!("/achats/recues/{facture}?erreur=montant#repartition")));
    let r = poster(&s, &format!("/achats/recues/{facture}/rattacher"), &format!("destination={chantier}&montant=100&categorie=fourniture"), &cookie).await;
    assert_eq!(r.lieu, Some(format!("/achats/recues/{facture}?ok=rattachee#repartition")));
    let achat: (i64, String, String) = sqlx::query_as("SELECT montant_ht_cents, fournisseur, description FROM chantier_achats WHERE facture_recue_id = $1")
        .bind(facture)
        .fetch_one(&mut *db)
        .await
        .unwrap();
    assert_eq!(achat, (10_000, "Matériaux SA".to_string(), "Facture n° F-2026-77".to_string()));
    assert!(lire(&s, &format!("/achats/recues/{facture}"), &cookie).await.corps.contains("Reste à répartir : 24,00"));
    poster(&s, &format!("/achats/recues/{facture}/rattacher"), "destination=sans&montant=24", &cookie).await;
    let r = lire(&s, &format!("/achats/recues/{facture}"), &cookie).await;
    assert!(r.corps.contains("Toute la base HT est répartie") && r.corps.contains("frais généraux"), "{}", r.corps);
    // Le chantier voit l'achat (budget et rentabilité).
    assert!(lire(&s, &format!("/chantiers/{chantier}/finances"), &cookie).await.corps.contains("Facture n° F-2026-77"));

    // La note de crédit compte en négatif.
    poster(&s, &format!("/achats/recues/{note}/rattacher"), &format!("destination={chantier}&montant=20&categorie=fourniture"), &cookie).await;
    let montant: i64 = sqlx::query_scalar("SELECT montant_ht_cents FROM chantier_achats WHERE facture_recue_id = $1").bind(note).fetch_one(&mut *db).await.unwrap();
    assert_eq!(montant, -2_000);
    let r = lire(&s, "/achats/recues?vue=traitees&ok=filtre", &cookie).await;
    assert!(r.corps.contains("F-2026-77") && r.corps.contains("NC-3"));
    assert!(lire(&s, "/achats/recues?ok=filtre", &cookie).await.corps.contains("Aucune facture à traiter"));

    // Annuler la répartition : l'achat quitte le chantier, la note est de nouveau à traiter.
    poster(&s, &format!("/achats/recues/{note}/annuler"), "", &cookie).await;
    let reste: i64 = sqlx::query_scalar("SELECT count(*) FROM chantier_achats WHERE facture_recue_id = $1").bind(note).fetch_one(&mut *db).await.unwrap();
    assert_eq!(reste, 0);
    assert!(lire(&s, &format!("/achats/recues/{note}"), &cookie).await.corps.contains("Reste à répartir : -20,00"));

    // Export pour le comptable : les factures reçues de la période, à part des ventes.
    let aujourdhui = chrono::Local::now().date_naive().to_string();
    let r = poster(&s, "/exports", &format!("debut={aujourdhui}&fin={aujourdhui}&format=les_deux&contenu=non_exportees"), &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/exports?ok=cree"));
    let export: i64 = sqlx::query_scalar("SELECT id FROM exports").fetch_one(&mut *db).await.unwrap();
    let zip = lire(&s, &format!("/exports/{export}/telecharger"), &cookie).await.octets;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let noms: Vec<String> = archive.file_names().map(str::to_string).collect();
    for attendu in [
        "Achats/achats.csv".to_string(),
        format!("Achats/UBL/Achat-{facture}-F-2026-77.xml"),
        format!("Achats/PDF/Achat-{facture}-F-2026-77.pdf"),
        format!("Achats/UBL/Achat-NC-{note}-NC-3.xml"),
    ] {
        assert!(noms.contains(&attendu), "{attendu} manque : {noms:?}");
    }
    let mut csv = String::new();
    archive.by_name("Achats/achats.csv").unwrap().read_to_string(&mut csv).unwrap();
    assert!(csv.contains("Matériaux SA;BE0987654394;F-2026-77") && csv.contains(";124,00;26,04;150,04;") && csv.contains(";-20,00;-4,20;-24,20;"), "{csv}");
    // Déjà exportées : un nouvel export « uniquement les non exportées » n'a plus rien.
    let r = poster(&s, "/exports", &format!("debut={aujourdhui}&fin={aujourdhui}&format=les_deux&contenu=non_exportees"), &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/exports?erreur=vide"));
}

#[sqlx::test]
async fn allumer_la_reception_apres_coup(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;
    let societe = inscrire(&pool, &s, &etat, &cookie, false).await;
    let corps = etat.lock().unwrap().inscriptions.iter().find(|(id, _)| *id == societe).cloned().unwrap().1;
    assert_eq!(corps["isSmpRecipient"], false, "case décochée : envoi seulement");

    // Réception éteinte : la boîte le dit et ne relève rien.
    deposer(&etat, &format!("doc_{societe}_1"), &societe, "invoice", "F-1", "10.00", "2.10", "12.10", false);
    let r = lire(&s, "/achats/recues", &cookie).await;
    assert!(r.corps.contains("La réception par Peppol n'est pas allumée"), "{}", r.corps);
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 0);

    // Réglages › Peppol : l'avertissement, et rien sans la case cochée.
    let r = lire(&s, "/parametres/peppol", &cookie).await;
    assert!(r.corps.contains("Recevoir mes factures fournisseurs dans Raven") && r.corps.contains("Avant d'allumer"), "{}", r.corps);
    assert_eq!(poster(&s, "/parametres/peppol/reception", "", &cookie).await.lieu.as_deref(), Some("/parametres/peppol?erreur=confirmation"));

    // Un membre ne peut pas l'allumer.
    connexion::ajouter_utilisateur(&pool, 2, "membre@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    sqlx::query("UPDATE membres SET role = 'membre' WHERE utilisateur_id = (SELECT id FROM utilisateurs WHERE email = 'membre@exemple.be')")
        .execute(&pool)
        .await
        .unwrap();
    let membre = connecter(&s, "membre%40exemple.be").await;
    assert_eq!(poster(&s, "/parametres/peppol/reception", "confirme=on", &membre).await.lieu.as_deref(), Some("/parametres/peppol?erreur=droits"));

    // Déjà inscrite ailleurs : Recommand refuse, l'écran explique la clé de migration.
    etat.lock().unwrap().deja_ailleurs = true;
    let r = poster(&s, "/parametres/peppol/reception", "confirme=on", &cookie).await;
    assert!(r.corps.contains("already registered") && r.corps.contains("clé de migration"), "{}", r.corps);
    assert!(!un::<bool>(&pool, "SELECT peppol_reception FROM organisations WHERE id = 2").await);
    etat.lock().unwrap().deja_ailleurs = false;

    // Avec la clé de migration de l'ancien point d'accès : la clé, puis l'inscription en réception.
    let r = poster(&s, "/parametres/peppol/reception", "confirme=on&cle_migration=court", &cookie).await;
    assert!(r.corps.contains("8 à 24 caractères"), "{}", r.corps);
    let r = poster(&s, "/parametres/peppol/reception", "confirme=on&cle_migration=Ab12%24%23xyZ9%21k", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/peppol?ok=reception"));
    assert!(etat.lock().unwrap().migrations.contains(&(societe.clone(), "Ab12$#xyZ9!k".to_string())));
    assert!(etat.lock().unwrap().mises_a_jour.iter().any(|(id, c)| *id == societe && c["isSmpRecipient"] == true));
    assert!(un::<bool>(&pool, "SELECT peppol_reception FROM organisations WHERE id = 2").await);

    // La facture en attente arrive.
    let r = lire(&s, "/achats/recues", &cookie).await;
    assert!(r.corps.contains("F-1"), "{}", r.corps);

    // Arrêter : la société n'est plus inscrite en réception, les factures reçues restent.
    assert_eq!(poster(&s, "/parametres/peppol/reception/arreter", "confirme=on", &cookie).await.lieu.as_deref(), Some("/parametres/peppol?ok=reception_arretee"));
    assert!(etat.lock().unwrap().mises_a_jour.iter().any(|(id, c)| *id == societe && c["isSmpRecipient"] == false));
    assert!(!un::<bool>(&pool, "SELECT peppol_reception FROM organisations WHERE id = 2").await);
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 1);
}

fn signer(corps: &str) -> String {
    let mut k = [0u8; 64];
    k[..SECRET_WEBHOOK.len()].copy_from_slice(SECRET_WEBHOOK.as_bytes());
    let mut interieur = Sha256::new();
    interieur.update(k.map(|b| b ^ 0x36));
    interieur.update(corps.as_bytes());
    let mut exterieur = Sha256::new();
    exterieur.update(k.map(|b| b ^ 0x5c));
    exterieur.update(interieur.finalize());
    format!("sha256={}", hex::encode(exterieur.finalize()))
}

async fn notifier(site: &Router, corps: &str, signature: Option<&str>) -> StatusCode {
    let mut r = Request::builder().method("POST").uri("/peppol/webhook").header(header::CONTENT_TYPE, "application/json");
    if let Some(s) = signature {
        r = r.header("X-Signature", s);
    }
    site.clone().oneshot(r.body(Body::from(corps.to_string())).unwrap()).await.unwrap().status()
}

#[sqlx::test]
async fn le_webhook_signe_enregistre_la_facture(options: PgPoolOptions, connexion: PgConnectOptions) {
    let etat = faux();
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = organisation(&pool, &s).await;
    let societe = inscrire(&pool, &s, &etat, &cookie, true).await;
    let id = format!("doc_{societe}_w");
    deposer(&etat, &id, &societe, "invoice", "W-1", "50.00", "10.50", "60.50", true);

    // Sans signature, ou avec une fausse : refusé, rien n'est enregistré (l'adresse est publique).
    let corps = json!({ "eventType": "document.received", "documentId": id, "teamId": "team_x", "companyId": societe }).to_string();
    assert_eq!(notifier(&s, &corps, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(notifier(&s, &corps, Some("sha256=00")).await, StatusCode::UNAUTHORIZED);
    let autre = corps.replace("team_x", "team_y");
    assert_eq!(notifier(&s, &corps, Some(&signer(&autre))).await, StatusCode::UNAUTHORIZED, "signature d'un autre corps");
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 0);

    // Signée : la facture est enregistrée tout de suite, sans relève.
    assert_eq!(notifier(&s, &corps, Some(&signer(&corps))).await, StatusCode::OK);
    assert_eq!(un::<String>(&pool, "SELECT numero FROM factures_recues").await, "W-1");
    assert!(!etat.lock().unwrap().non_lus.contains(&id));
    // La même notification deux fois : pas de doublon.
    assert_eq!(notifier(&s, &corps, Some(&signer(&corps))).await, StatusCode::OK);
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 1);

    // Une notification qui annonce un document d'une autre société sous notre nom : relu chez Recommand, refusé.
    deposer(&etat, "doc_pas_a_nous", "c_quelquun_d_autre", "invoice", "Z-9", "10.00", "2.10", "12.10", false);
    let piege = json!({ "eventType": "document.received", "documentId": "doc_pas_a_nous", "companyId": societe }).to_string();
    assert_eq!(notifier(&s, &piege, Some(&signer(&piege))).await, StatusCode::OK);
    assert_eq!(un::<i64>(&pool, "SELECT count(*) FROM factures_recues").await, 1);

    // Une société inconnue : ignoré.
    let inconnue = json!({ "eventType": "document.received", "documentId": id, "companyId": "c_inconnue" }).to_string();
    assert_eq!(notifier(&s, &inconnue, Some(&signer(&inconnue))).await, StatusCode::OK);
}
