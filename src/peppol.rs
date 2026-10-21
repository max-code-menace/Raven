//! Envoi Peppol (étape 5) : l'adresse Peppol des parties, les contrôles avant l'envoi, puis l'envoi par l'API du point
//! d'accès Recommand (choix de Mahx du 20/10/2026, voir `docs/peppol-points-acces.md`).
//!
//! **Sécurité** : rien ne part tant que `configurer` n'a pas reçu la clé et le secret de Recommand. Seul `main.rs`
//! l'appelle, avec `RECOMMAND_CLE_API` et `RECOMMAND_SECRET` ; les tests passent par un faux serveur Recommand.
//!
//! Une adresse Peppol suit la norme ISO 6523 : un schéma sur 4 chiffres et un numéro (« 0208:0123456789 »). Une
//! entreprise belge est inscrite sous son **numéro d'entreprise** (schéma 0208), le numéro de TVA sans « BE » : on le
//! déduit du numéro de TVA. Pour un client étranger ou inscrit autrement, l'identifiant se saisit sur sa fiche.

use std::sync::OnceLock;

use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgConnection;

use crate::base::{Base, Utilisateur};
use crate::factures::Facture;
use crate::{AppState, Erreur};

/// Le schéma du numéro d'entreprise belge (Banque-Carrefour des Entreprises).
pub const SCHEMA_BCE: &str = "0208";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifiant {
    pub schema: String,
    pub numero: String,
}

impl std::fmt::Display for Identifiant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.schema, self.numero)
    }
}

/// Le numéro d'entreprise belge (10 chiffres) d'un numéro de TVA belge, quelle que soit son écriture
/// (« BE 0123.456.789 », « be0123456789 »). Un ancien numéro à 9 chiffres prend un 0 devant.
fn numero_bce(tva: &str) -> Option<String> {
    let compact: String = tva.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    let chiffres = compact.strip_prefix("BE")?;
    if !chiffres.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match chiffres.len() {
        10 => Some(chiffres.to_string()),
        9 => Some(format!("0{chiffres}")),
        _ => None,
    }
}

/// Les deux derniers chiffres d'un numéro d'entreprise valent 97 moins le reste de la division des huit premiers
/// par 97.
pub fn bce_valide(numero: &str) -> bool {
    if numero.len() != 10 || !numero.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let (base, controle) = numero.split_at(8);
    let base: u64 = base.parse().unwrap_or(0);
    let controle: u64 = controle.parse().unwrap_or(0);
    97 - base % 97 == controle
}

/// L'adresse Peppol déduite d'un numéro de TVA : seulement pour un numéro belge.
pub fn depuis_tva(tva: &str) -> Option<Identifiant> {
    numero_bce(tva).map(|numero| Identifiant { schema: SCHEMA_BCE.into(), numero })
}

/// Lit un identifiant saisi (« 0208:0123.456.789 », « 0088 : 5400000000001 ») et le remet au propre.
pub fn lire(texte: &str) -> Result<Identifiant, String> {
    let erreur = || "L'identifiant Peppol s'écrit avec le schéma, deux-points et le numéro (exemple : 0208:0123456789).".to_string();
    let (schema, numero) = texte.split_once(':').ok_or_else(erreur)?;
    let schema = schema.trim();
    if schema.len() != 4 || !schema.chars().all(|c| c.is_ascii_digit()) {
        return Err(erreur());
    }
    let mut numero: String = numero.trim().chars().filter(|c| !c.is_whitespace()).collect();
    if schema == SCHEMA_BCE {
        numero.retain(|c| c.is_ascii_digit());
        if !bce_valide(&numero) {
            return Err("Ce numéro d'entreprise belge n'est pas valide (10 chiffres, les deux derniers de contrôle).".into());
        }
    }
    if numero.is_empty() || numero.len() > 50 || !numero.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(erreur());
    }
    Ok(Identifiant { schema: schema.into(), numero })
}

/// L'adresse Peppol d'un client : celle de sa fiche, sinon celle de son numéro de TVA belge.
pub fn du_client(peppol_id: Option<&str>, tva: &str) -> Option<Identifiant> {
    match peppol_id.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => lire(p).ok(),
        None => depuis_tva(tva),
    }
}

/// Le bloc « Facturation électronique » de la fiche d'une facture validée.
pub struct Bloc {
    /// L'adresse Peppol du client (« 0208:0123456789 »), vide s'il n'en a pas.
    pub client: String,
    /// Un client sans numéro de TVA ni identifiant : un particulier, qui reçoit la facture par e-mail.
    pub particulier: bool,
    /// Facture reprise d'un autre logiciel : elle n'est pas renvoyée.
    pub reprise: bool,
    /// Ce qui empêche l'envoi (vide : la facture peut partir par Peppol).
    pub problemes: Vec<String>,
    /// L'envoi par Peppol est configuré sur ce serveur (clé Recommand).
    pub envoi_actif: bool,
    /// L'organisation est inscrite et vérifiée chez Recommand.
    pub organisation_verifiee: bool,
    /// Les envois déjà faits, du plus récent au plus ancien.
    pub envois: Vec<LigneEnvoi>,
}

impl Bloc {
    pub fn prete(&self) -> bool {
        !self.particulier && !self.reprise && self.problemes.is_empty()
    }

    /// Le bouton « Envoyer par Peppol » : prête, serveur configuré, organisation vérifiée, pas déjà partie.
    pub fn peut_envoyer(&self) -> bool {
        self.prete() && self.envoi_actif && self.organisation_verifiee && !self.envois.iter().any(|e| e.statut != "echec")
    }

    /// Un envoi attend encore la confirmation de livraison.
    pub fn en_attente(&self) -> bool {
        self.envois.iter().any(|e| e.statut == "en_cours")
    }
}

/// Un envoi par Peppol, pour la fiche de la facture.
pub struct LigneEnvoi {
    pub statut: String,
    pub libelle: &'static str,
    pub classe: &'static str,
    pub quand: String,
    pub destinataire: String,
    pub erreur: String,
}

fn libelle_statut(statut: &str) -> (&'static str, &'static str) {
    match statut {
        "livre" => ("Livrée", "s-accepte"),
        "echec" => ("Échec", "s-echec"),
        _ => ("En cours", "s-envoye"),
    }
}

async fn envois(db: &mut PgConnection, facture_id: i64) -> Result<Vec<LigneEnvoi>, sqlx::Error> {
    let lignes: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT statut, destinataire, erreur, to_char(envoye_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI')
         FROM envois_peppol WHERE facture_id = $1 ORDER BY envoye_le DESC, id DESC",
    )
    .bind(facture_id)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(statut, destinataire, erreur, quand)| {
            let (libelle, classe) = libelle_statut(&statut);
            LigneEnvoi { statut, libelle, classe, quand, destinataire, erreur }
        })
        .collect())
}

/// Les contrôles d'une facture (ou note de crédit) validée avant son envoi par Peppol. `None` pour un brouillon.
pub(crate) async fn controler(db: &mut PgConnection, f: &Facture) -> Result<Option<Bloc>, sqlx::Error> {
    if f.numero.is_none() {
        return Ok(None);
    }
    let client: Option<(Option<String>, Option<String>, String)> = match (f.client_entreprise_id, f.client_contact_id) {
        (Some(e), _) => sqlx::query_as("SELECT peppol_id, numero_tva, pays FROM entreprises WHERE id = $1").bind(e).fetch_optional(&mut *db).await?,
        (_, Some(c)) => sqlx::query_as("SELECT peppol_id, numero_tva, pays FROM contacts WHERE id = $1").bind(c).fetch_optional(&mut *db).await?,
        _ => None,
    };
    let (peppol_id, tva, pays) = client.unwrap_or((None, None, String::new()));
    let tva = tva.unwrap_or_default();
    let identifiant = du_client(peppol_id.as_deref(), &tva);
    let particulier = identifiant.is_none() && tva.trim().is_empty() && peppol_id.is_none();

    let mut problemes = Vec::new();
    match &identifiant {
        Some(i) if i.schema == SCHEMA_BCE && !bce_valide(&i.numero) => problemes.push(format!(
            "Le numéro de TVA du client ({}) n'est pas valide : vérifiez-le sur sa fiche.",
            tva.trim()
        )),
        Some(_) => {}
        None if !particulier => problemes.push(if pays.trim().is_empty() || pays == "BE" {
            "Le numéro de TVA du client n'est pas un numéro belge valide : corrigez-le, ou saisissez son identifiant Peppol sur sa fiche.".to_string()
        } else {
            "Client hors de Belgique : saisissez son identifiant Peppol sur sa fiche (il vous le communique).".to_string()
        }),
        None => {}
    }
    let tva_societe: Option<Option<String>> =
        sqlx::query_scalar("SELECT numero_tva FROM societe WHERE organisation_id = app_organisation()").fetch_optional(&mut *db).await?;
    match depuis_tva(tva_societe.flatten().as_deref().unwrap_or("")) {
        Some(i) if bce_valide(&i.numero) => {}
        Some(_) => problemes.push("Le numéro de TVA de votre société n'est pas valide (Réglages › Société).".into()),
        None => problemes.push("Le numéro de TVA belge de votre société manque (Réglages › Société).".into()),
    }

    let organisation_verifiee: Option<bool> =
        sqlx::query_scalar("SELECT peppol_verifiee FROM organisations WHERE id = app_organisation()").fetch_optional(&mut *db).await?;

    Ok(Some(Bloc {
        client: identifiant.map(|i| i.to_string()).unwrap_or_default(),
        particulier,
        reprise: f.reprise,
        problemes,
        envoi_actif: envoi_actif(),
        organisation_verifiee: organisation_verifiee.unwrap_or(false),
        envois: envois(&mut *db, f.id).await?,
    }))
}

// ─── L'API de Recommand ─────────────────────────────────────────────────────

const ADRESSE_RECOMMAND: &str = "https://app.recommand.eu";

struct Config {
    cle: String,
    secret: String,
    adresse: String,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Appelé une seule fois au lancement (`main.rs`) : sans clé ni secret, rien ne part par Peppol. `adresse` sert aux
/// tests (faux serveur) ; par défaut, https://app.recommand.eu (la clé dit s'il s'agit du bac à sable ou du vrai réseau).
pub fn configurer(cle: Option<String>, secret: Option<String>, adresse: Option<String>) {
    let propre = |t: Option<String>| t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let (Some(cle), Some(secret)) = (propre(cle), propre(secret)) {
        let adresse = propre(adresse).map(|a| a.trim_end_matches('/').to_string()).unwrap_or_else(|| ADRESSE_RECOMMAND.into());
        let _ = CONFIG.set(Config { cle, secret, adresse });
    }
}

/// L'envoi par Peppol est configuré sur ce serveur.
pub fn envoi_actif() -> bool {
    CONFIG.get().is_some()
}

/// Le message lisible d'une réponse en erreur : `{"success": false, "errors": {"chemin": ["message", …]}}`.
fn message_erreur(code: u16, v: &Value) -> String {
    let messages: Vec<String> = v["errors"]
        .as_object()
        .map(|o| o.values().flat_map(|m| m.as_array().cloned().unwrap_or_default()).filter_map(|m| m.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if messages.is_empty() { format!("Recommand a répondu {code}.") } else { messages.join(" ; ") }
}

/// Un appel à l'API de Recommand (authentification « basique » : clé et secret). Rend la réponse, ou l'erreur lisible.
async fn appel(methode: &'static str, chemin: String, corps: Option<Value>) -> Result<Value, String> {
    let Some(c) = CONFIG.get() else { return Err("L'envoi par Peppol n'est pas configuré sur ce serveur.".into()) };
    let (adresse, autorisation) =
        (format!("{}{chemin}", c.adresse), format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", c.cle, c.secret))));
    tokio::task::spawn_blocking(move || {
        let requete = ureq::request(methode, &adresse)
            .set("Authorization", &autorisation)
            .set("accept", "application/json")
            .timeout(std::time::Duration::from_secs(60));
        let reponse = match corps {
            Some(corps) => requete.send_json(corps),
            None => requete.call(),
        };
        match reponse {
            Ok(r) => {
                let v: Value = r.into_json().map_err(|e| format!("Réponse illisible de Recommand : {e}"))?;
                if v["success"] == Value::Bool(false) { Err(message_erreur(200, &v)) } else { Ok(v) }
            }
            Err(ureq::Error::Status(code, r)) => Err(message_erreur(code, &r.into_json().unwrap_or(Value::Null))),
            Err(e) => Err(format!("Recommand injoignable : {e}")),
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
}

/// L'inscription de l'organisation chez Recommand.
pub struct Inscription {
    pub societe_id: Option<String>,
    pub verifiee: bool,
    pub lien: String,
}

async fn inscription(db: &mut PgConnection) -> Result<Inscription, sqlx::Error> {
    let (societe_id, verifiee, lien): (Option<String>, bool, Option<String>) = sqlx::query_as(
        "SELECT peppol_societe_id, peppol_verifiee, peppol_lien_verification FROM organisations WHERE id = app_organisation()",
    )
    .fetch_one(&mut *db)
    .await?;
    Ok(Inscription { societe_id, verifiee, lien: lien.unwrap_or_default() })
}

/// Inscrit l'organisation chez Recommand, en envoi seulement, avec son numéro d'entreprise (Recommand en déduit ses
/// adresses Peppol 0208 et 9925). Rend le message d'erreur à montrer, s'il y en a un.
async fn inscrire(db: &mut PgConnection) -> Result<Option<String>, sqlx::Error> {
    if inscription(&mut *db).await?.societe_id.is_some() {
        return Ok(None);
    }
    let societe: Option<(String, String, String, String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT nom, rue, numero, code_postal, ville, pays, numero_tva, email, telephone FROM societe WHERE organisation_id = app_organisation()",
    )
    .fetch_optional(&mut *db)
    .await?;
    let Some((nom, rue, numero, code_postal, ville, pays, tva, email, telephone)) = societe else {
        return Ok(Some("Remplissez d'abord Réglages › Société.".into()));
    };
    let Some(bce) = depuis_tva(tva.as_deref().unwrap_or("")).filter(|i| bce_valide(&i.numero)) else {
        return Ok(Some("Il faut le numéro de TVA belge de votre société (Réglages › Société) : Peppol vous identifie par votre numéro d'entreprise.".into()));
    };
    if rue.trim().is_empty() || code_postal.trim().is_empty() || ville.trim().is_empty() {
        return Ok(Some("Il faut l'adresse complète de votre société (Réglages › Société).".into()));
    }
    let mut corps = json!({
        "name": nom.trim(),
        "address": format!("{} {}", rue.trim(), numero.trim()).trim(),
        "postalCode": code_postal.trim(),
        "city": ville.trim(),
        "country": if pays.trim().is_empty() { "BE" } else { pays.trim() },
        "enterpriseNumber": bce.numero,
        "vatNumber": format!("BE{}", bce.numero),
        // Envoi seulement : la réception reste là où l'entreprise la reçoit aujourd'hui (étape 5-3).
        "isSmpRecipient": false,
    });
    if crate::commun::email_valide(email.trim()) {
        corps["email"] = json!(email.trim());
    }
    if !telephone.trim().is_empty() {
        corps["phone"] = json!(telephone.trim());
    }
    match appel("POST", "/api/v1/companies".into(), Some(corps)).await {
        Ok(v) => {
            let Some(id) = v["company"]["id"].as_str() else { return Ok(Some("Réponse inattendue de Recommand.".into())) };
            sqlx::query(
                "UPDATE organisations SET peppol_societe_id = $1, peppol_verifiee = $2, peppol_lien_verification = $3 WHERE id = app_organisation()",
            )
            .bind(id)
            .bind(v["company"]["isVerified"].as_bool().unwrap_or(false))
            .bind(v["verificationUrl"].as_str())
            .execute(&mut *db)
            .await?;
            Ok(None)
        }
        Err(e) => {
            eprintln!("Inscription Peppol refusée : {e}");
            Ok(Some(format!("Recommand a refusé l'inscription : {e}")))
        }
    }
}

/// Relit chez Recommand si l'organisation est vérifiée ; demande un nouveau lien de vérification s'il manque.
async fn actualiser_organisation(db: &mut PgConnection) -> Result<Option<String>, sqlx::Error> {
    let i = inscription(&mut *db).await?;
    let Some(id) = i.societe_id else { return Ok(None) };
    let verifiee = match appel("GET", format!("/api/v1/companies/{id}"), None).await {
        Ok(v) => v["company"]["isVerified"].as_bool().unwrap_or(false),
        Err(e) => return Ok(Some(e)),
    };
    let mut lien = i.lien;
    if !verifiee && lien.is_empty() {
        match appel("POST", format!("/api/v1/companies/{id}/verify"), Some(json!({}))).await {
            Ok(v) => lien = v["verificationUrl"].as_str().unwrap_or_default().to_string(),
            Err(e) => return Ok(Some(e)),
        }
    }
    sqlx::query("UPDATE organisations SET peppol_verifiee = $1, peppol_lien_verification = $2 WHERE id = app_organisation()")
        .bind(verifiee)
        .bind(if verifiee { None } else { Some(lien) }.filter(|l| !l.is_empty()))
        .execute(&mut *db)
        .await?;
    Ok(None)
}

/// Le statut d'un document chez Recommand : livré, en échec (avec la raison) ou en cours.
fn statut_document(v: &Value) -> (&'static str, String) {
    let raison = v["deliveries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["failure"]["message"].as_str())
        .collect::<Vec<_>>()
        .join(" ; ");
    match v["deliveryStatus"].as_str() {
        Some("delivered") => ("livre", String::new()),
        Some("failed") => ("echec", if raison.is_empty() { "Le destinataire n'a pas pu recevoir la facture.".into() } else { raison }),
        _ => ("en_cours", String::new()),
    }
}

/// Envoie une facture validée par Peppol : l'UBL tel qu'il a été émis (archive légale), adressé au client.
async fn envoyer_facture(db: &mut PgConnection, f: &Facture) -> Result<bool, Erreur> {
    let bloc = controler(&mut *db, f).await?.ok_or(Erreur::conflit("Seule une facture validée part par Peppol."))?;
    if bloc.reprise {
        return Err(Erreur::conflit("Une facture reprise d'un autre logiciel n'est pas renvoyée par Peppol."));
    }
    if bloc.particulier {
        return Err(Erreur::conflit("Un particulier ne reçoit pas de facture par Peppol : envoyez-la par e-mail."));
    }
    if !bloc.prete() {
        return Err(Erreur::conflit("La facture n'est pas prête pour Peppol : voyez ce qu'il faut corriger sur sa fiche."));
    }
    if !bloc.envoi_actif {
        return Err(Erreur::conflit("L'envoi par Peppol n'est pas configuré sur ce serveur."));
    }
    let Some(societe_id) = inscription(&mut *db).await?.societe_id.filter(|_| bloc.organisation_verifiee) else {
        return Err(Erreur::conflit("Votre entreprise n'est pas encore inscrite et vérifiée pour Peppol (Réglages › Peppol)."));
    };
    if !bloc.peut_envoyer() {
        return Err(Erreur::conflit("Cette facture est déjà partie par Peppol."));
    }
    let xml = crate::archive::ubl(&mut *db, f).await?.ok_or(Erreur::conflit("Seule une facture validée part par Peppol."))?;
    let reponse = appel(
        "POST",
        format!("/api/v1/{societe_id}/send"),
        Some(json!({ "recipient": bloc.client, "documentType": "xml", "document": xml })),
    )
    .await;
    let (statut, document_id, erreur) = match &reponse {
        Ok(v) => {
            let (statut, raison) = statut_document(v);
            (statut, v["id"].as_str().unwrap_or_default().to_string(), raison)
        }
        Err(e) => {
            eprintln!("Envoi Peppol refusé : {e}");
            ("echec", String::new(), e.clone())
        }
    };

    let mut tx = sqlx::Connection::begin(&mut *db).await?;
    sqlx::query("INSERT INTO envois_peppol (facture_id, destinataire, document_id, statut, erreur) VALUES ($1, $2, $3, $4, $5)")
        .bind(f.id)
        .bind(&bloc.client)
        .bind(&document_id)
        .bind(statut)
        .bind(&erreur)
        .execute(&mut *tx)
        .await?;
    if statut == "echec" {
        crate::paiements::noter(&mut tx, f.id, "peppol", &format!("Échec de l'envoi par Peppol : {erreur}")).await?;
    } else {
        let detail = format!("Par Peppol, à {}", bloc.client);
        if !crate::factures::marquer_envoyee_dans(&mut tx, f.id, &detail).await? {
            crate::paiements::noter(&mut tx, f.id, "peppol", &detail).await?;
        }
    }
    tx.commit().await?;
    Ok(statut != "echec")
}

/// Relit chez Recommand le statut des envois de la facture qui attendent une confirmation.
async fn actualiser_envois(db: &mut PgConnection, facture_id: i64) -> Result<(), sqlx::Error> {
    let en_cours: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, document_id FROM envois_peppol WHERE facture_id = $1 AND statut = 'en_cours' AND document_id <> ''")
            .bind(facture_id)
            .fetch_all(&mut *db)
            .await?;
    for (id, document_id) in en_cours {
        let Ok(v) = appel("GET", format!("/api/v1/documents/{document_id}"), None).await else { continue };
        let (statut, erreur) = statut_document(&v["document"]);
        if statut != "en_cours" {
            sqlx::query("UPDATE envois_peppol SET statut = $2, erreur = $3, maj_le = now() WHERE id = $1").bind(id).bind(statut).bind(&erreur).execute(&mut *db).await?;
            if statut == "echec" {
                let mut tx = sqlx::Connection::begin(&mut *db).await?;
                crate::paiements::noter(&mut tx, facture_id, "peppol", &format!("Échec de la livraison par Peppol : {erreur}")).await?;
                tx.commit().await?;
            }
        }
    }
    Ok(())
}

// ─── Écrans ─────────────────────────────────────────────────────────────────

async fn envoyer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let f = crate::factures::charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    let parti = envoyer_facture(&mut db, &f).await?;
    Ok(Redirect::to(&format!("/factures/{id}?{}#peppol", if parti { "ok=peppol" } else { "erreur=peppol" })))
}

async fn actualiser(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    crate::factures::charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    actualiser_envois(&mut db, id).await?;
    Ok(Redirect::to(&format!("/factures/{id}#peppol")))
}

#[derive(Template)]
#[template(path = "reglages_peppol.html")]
struct Reglages {
    envoi_actif: bool,
    inscription: Inscription,
    proprietaire: bool,
    numero_entreprise: String,
    message_erreur: String,
    message_ok: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

async fn page_reglages(db: &mut Base, moi: i64, message_erreur: String, message_ok: &'static str) -> Result<Html<String>, Erreur> {
    let organisation = db.organisation;
    let tva: Option<Option<String>> =
        sqlx::query_scalar("SELECT numero_tva FROM societe WHERE organisation_id = app_organisation()").fetch_optional(&mut **db).await?;
    let page = Reglages {
        envoi_actif: envoi_actif(),
        inscription: inscription(&mut *db).await?,
        proprietaire: crate::equipe::role(&mut *db, moi, organisation).await?.as_deref() == Some("proprietaire"),
        numero_entreprise: depuis_tva(tva.flatten().as_deref().unwrap_or("")).map(|i| i.to_string()).unwrap_or_default(),
        message_erreur,
        message_ok,
    };
    Ok(Html(page.render()?))
}

async fn reglages(mut db: Base, Utilisateur(moi): Utilisateur, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    // Ouvrir la page relit l'état de la vérification chez Recommand.
    let erreur = if envoi_actif() && !inscription(&mut db).await?.verifiee { actualiser_organisation(&mut db).await? } else { None };
    let message_ok = match r.ok.as_str() {
        "inscrite" => "Votre entreprise est inscrite chez Recommand. Il reste à vérifier votre identité.",
        _ => "",
    };
    let erreur = erreur.unwrap_or_else(|| if r.erreur == "droits" { "Seul un propriétaire peut inscrire l'entreprise.".into() } else { String::new() });
    page_reglages(&mut db, moi, erreur, message_ok).await
}

async fn inscrire_page(mut db: Base, Utilisateur(moi): Utilisateur) -> Result<Response, Erreur> {
    let organisation = db.organisation;
    if crate::equipe::role(&mut db, moi, organisation).await?.as_deref() != Some("proprietaire") {
        return Ok(Redirect::to("/parametres/peppol?erreur=droits").into_response());
    }
    if !envoi_actif() {
        return Err(Erreur::conflit("L'envoi par Peppol n'est pas configuré sur ce serveur."));
    }
    match inscrire(&mut db).await? {
        None => Ok(Redirect::to("/parametres/peppol?ok=inscrite").into_response()),
        Some(e) => Ok(page_reglages(&mut db, moi, e, "").await?.into_response()),
    }
}

/// Le formulaire de la page de réglages n'a pas de champ : ce type vide accepte la requête.
#[derive(Deserialize)]
struct Vide {}

async fn actualiser_page(mut db: Base, Utilisateur(moi): Utilisateur, Form(_): Form<Vide>) -> Result<Response, Erreur> {
    match actualiser_organisation(&mut db).await? {
        None => Ok(Redirect::to("/parametres/peppol").into_response()),
        Some(e) => Ok(page_reglages(&mut db, moi, e, "").await?.into_response()),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/parametres/peppol", get(reglages))
        .route("/parametres/peppol/inscrire", post(inscrire_page))
        .route("/parametres/peppol/actualiser", post(actualiser_page))
        .route("/factures/{id}/peppol", post(envoyer))
        .route("/factures/{id}/peppol/actualiser", post(actualiser))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_numero_d_entreprise_vient_du_numero_de_tva() {
        let i = depuis_tva("BE 0123.456.749").unwrap();
        assert_eq!(i.to_string(), "0208:0123456749");
        assert_eq!(depuis_tva("be123456749").unwrap().numero, "0123456749", "ancien numéro à 9 chiffres");
        assert_eq!(depuis_tva("FR12345678901"), None, "pas de déduction hors de Belgique");
        assert_eq!(depuis_tva(""), None);
        assert_eq!(depuis_tva("BE12"), None);
    }

    #[test]
    fn chiffres_de_controle() {
        assert!(bce_valide("0123456749"));
        assert!(!bce_valide("0123456748"));
        assert!(!bce_valide("0123456789"));
        assert!(!bce_valide("012345674"));
    }

    #[test]
    fn identifiant_saisi() {
        assert_eq!(lire(" 0208 : 0123.456.749 ").unwrap().to_string(), "0208:0123456749");
        assert_eq!(lire("0088:5400000000001").unwrap().to_string(), "0088:5400000000001");
        assert_eq!(lire("9925:BE0123456749").unwrap().numero, "BE0123456749");
        assert!(lire("0208:0123456789").is_err(), "chiffres de contrôle faux");
        assert!(lire("0123456749").is_err(), "sans schéma");
        assert!(lire("208:0123456749").is_err());
        assert!(lire("0088:").is_err());
        assert!(lire("0088:54 00<>").is_err());
    }

    #[test]
    fn adresse_du_client() {
        assert_eq!(du_client(None, "BE0123456749").unwrap().to_string(), "0208:0123456749");
        assert_eq!(du_client(Some("0088:5400000000001"), "BE0123456749").unwrap().to_string(), "0088:5400000000001", "la fiche l'emporte");
        assert_eq!(du_client(Some(""), "BE0123456749").unwrap().schema, "0208");
        assert_eq!(du_client(None, ""), None);
    }
}
