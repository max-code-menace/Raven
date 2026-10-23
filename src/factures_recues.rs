//! Réception des factures fournisseurs par Peppol (étape 5-3, décision de Mahx du 21/10/2026) : sans cela, Ubique ne
//! peut pas quitter Vertuoza, où elle reçoit ses factures (BigMat…) et les rattache à ses chantiers.
//!
//! - **Arrivée** : Recommand reçoit la facture, la contrôle et la lit (`parsed`). Raven la relève par l'API (bouton
//!   « Relever » et ouverture de la boîte de réception, `GET /api/v1/inbox`) ou, en production, la reçoit par le
//!   **webhook** `document.received` (corps signé HMAC SHA-256 avec `RECOMMAND_WEBHOOK_SECRET`). Une fois enregistrée,
//!   elle est marquée « lue » chez Recommand ; une facture déjà enregistrée ne l'est jamais deux fois.
//! - **Archive** : le XML reçu est gardé tel quel avec son empreinte SHA-256, non modifiable (déclencheur), comme
//!   l'archive des ventes ; le PDF joint par le fournisseur (sinon le rendu de Recommand) l'accompagne.
//! - **Fournisseur** : retrouvé dans les entreprises par son numéro de TVA (ou son adresse Peppol), sinon créé.
//! - **Boîte de réception** (Paiements › À payer › Reçues par Peppol) : à traiter / traitées ; une facture
//!   se **rattache** à un ou plusieurs chantiers (elle devient un achat du chantier : budget et rentabilité à jour) ou
//!   se classe **sans chantier** (frais généraux). Elle est traitée quand toute sa base HT est répartie (statut
//!   calculé, jamais enregistré). Une note de crédit fournisseur compte en négatif.
//! - **Export pour le comptable** : les factures reçues de la période (XML, PDF, récapitulatif), à part des ventes.
//!
//! **Sécurité** : le webhook est la seule page sans session qui écrit des données. Il refuse tout sans secret configuré
//! ou sans signature valable, ne prend de l'appel que l'identifiant du document, relit le document chez Recommand
//! avec la clé de Raven et vérifie qu'il est bien adressé à la société Recommand de l'organisation.

use askama::Template;
use axum::{
    Form, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::Engine;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};

use crate::base::Base;
use crate::commun::{Choix, echapper_like, format_euros};
use crate::fiches::euros_vers_centimes;
use crate::paiements_fournisseurs;
use crate::suivi_chantier::{CATEGORIES, libelle_categorie};
use crate::{AppState, Erreur, peppol};

// ─── Lecture d'un document reçu ─────────────────────────────────────────────

/// Un montant de Recommand (« 1234.5 », « -21.00 », ou un nombre) en centimes, arrondi au centime le plus proche.
fn centimes(v: &Value) -> Option<i64> {
    let texte = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let (signe, chiffres) = match texte.strip_prefix('-') {
        Some(reste) => (-1, reste),
        None => (1, texte.as_str()),
    };
    let (entier, decimales) = chiffres.split_once('.').unwrap_or((chiffres, ""));
    if entier.is_empty() || !entier.chars().all(|c| c.is_ascii_digit()) || !decimales.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut d: String = decimales.chars().chain("000".chars()).take(3).collect();
    let arrondi = d.pop().map(|c| c >= '5').unwrap_or(false);
    let valeur = entier.parse::<i64>().ok()?.checked_mul(100)?.checked_add(d.parse::<i64>().ok()?)? + i64::from(arrondi);
    Some(signe * valeur)
}

/// Ce que Raven garde d'un document reçu.
#[derive(Debug)]
struct Lu {
    genre: &'static str,
    expediteur: String,
    fournisseur_nom: String,
    fournisseur_tva: String,
    rue: String,
    code_postal: String,
    ville: String,
    pays: String,
    email: String,
    telephone: String,
    numero: String,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    ht: i64,
    tva: i64,
    ttc: i64,
    a_payer: i64,
    iban: String,
    communication: String,
    validation: String,
    donnees: Value,
    xml: String,
    pdf: Option<Vec<u8>>,
    recue_le: String,
}

fn texte(v: &Value) -> String {
    v.as_str().map(|s| s.trim().to_string()).unwrap_or_default()
}

fn date(v: &Value) -> Option<NaiveDate> {
    v.as_str().and_then(|s| NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok())
}

/// Lit un document de Recommand (`GET /api/v1/documents/{id}`, champ `document`) : une facture ou une note de crédit.
fn lire_document(doc: &Value) -> Result<Lu, String> {
    let genre = match doc["type"].as_str() {
        Some("invoice") => "facture",
        Some("creditNote") => "note_credit",
        autre => return Err(format!("Document de type « {} » : Raven ne lit que les factures et les notes de crédit.", autre.unwrap_or("?"))),
    };
    let p = &doc["parsed"];
    let xml = doc["xml"].as_str().filter(|x| !x.trim().is_empty()).ok_or("Document sans XML.")?.to_string();
    let vendeur = &p["seller"];
    let numero = texte(if genre == "facture" { &p["invoiceNumber"] } else { &p["creditNoteNumber"] });
    let recue_le = texte(&doc["createdAt"]);
    let date_facture = date(&p["issueDate"]).or_else(|| date(&doc["createdAt"])).ok_or("Document sans date.")?;

    // Les totaux du document ; à défaut, recalculés depuis le détail de la TVA ou les lignes.
    let sous_totaux: Vec<&Value> = p["vat"]["subtotals"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let lignes_ht: i64 = p["lines"].as_array().into_iter().flatten().filter_map(|l| centimes(&l["netAmount"])).sum();
    let ht = centimes(&p["totals"]["taxExclusiveAmount"])
        .or_else(|| (!sous_totaux.is_empty()).then(|| sous_totaux.iter().filter_map(|s| centimes(&s["taxableAmount"])).sum()))
        .unwrap_or(lignes_ht);
    let tva = centimes(&p["vat"]["totalVatAmount"])
        .or_else(|| centimes(&p["totals"]["taxInclusiveAmount"]).map(|ttc| ttc - ht))
        .unwrap_or_else(|| sous_totaux.iter().filter_map(|s| centimes(&s["vatAmount"])).sum());
    let ttc = centimes(&p["totals"]["taxInclusiveAmount"]).unwrap_or(ht + tva);
    let a_payer = centimes(&p["totals"]["payableAmount"]).unwrap_or(ttc);

    let paiement = p["paymentMeans"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
    // Le PDF joint par le fournisseur (le premier) ; le contenu des pièces jointes ne reste pas dans `donnees`.
    let mut donnees = p.clone();
    let mut pdf = None;
    if let Some(pieces) = donnees["attachments"].as_array_mut() {
        for piece in pieces {
            let est_pdf = piece["mimeCode"].as_str().is_none_or(|m| m.contains("pdf"));
            if let Some(contenu) = piece["embeddedDocument"].as_str() {
                if est_pdf && pdf.is_none() {
                    pdf = base64::engine::general_purpose::STANDARD.decode(contenu.trim()).ok().filter(|o| o.starts_with(b"%PDF"));
                }
                piece["embeddedDocument"] = Value::Null;
            }
        }
    }

    Ok(Lu {
        genre,
        expediteur: texte(&doc["senderId"]),
        fournisseur_nom: Some(texte(&vendeur["name"])).filter(|n| !n.is_empty()).unwrap_or_else(|| "Fournisseur sans nom".into()),
        fournisseur_tva: texte(&vendeur["vatNumber"]),
        rue: [texte(&vendeur["street"]), texte(&vendeur["street2"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
        code_postal: texte(&vendeur["postalZone"]),
        ville: texte(&vendeur["city"]),
        pays: Some(texte(&vendeur["country"])).filter(|c| c.len() == 2).unwrap_or_else(|| "BE".into()),
        email: texte(&vendeur["email"]),
        telephone: texte(&vendeur["phone"]),
        numero,
        date_facture,
        date_echeance: date(&p["dueDate"]),
        ht,
        tva,
        ttc,
        a_payer,
        iban: texte(&paiement["iban"]),
        communication: texte(&paiement["reference"]),
        validation: texte(&doc["validation"]["result"]),
        donnees,
        xml,
        pdf,
        recue_le,
    })
}

/// Le numéro de TVA écrit sans espace ni point, en majuscules (« BE 0123.456.749 » → « BE0123456749 »).
fn tva_compacte(tva: &str) -> String {
    tva.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}

// ─── Références du document : le chantier proposé d'office ──────────────────

/// Les références qu'un fournisseur recopie sur sa facture (UBL) : c'est là qu'il écrit d'habitude la référence du
/// chantier que son client lui a donnée. (élément, élément lu dedans, nom en clair)
const REFERENCES: [(&str, Option<&str>, &str); 4] = [
    ("BuyerReference", None, "Référence de l'acheteur"),
    ("OrderReference", Some("ID"), "Bon de commande"),
    ("ProjectReference", Some("ID"), "Projet"),
    ("ContractDocumentReference", Some("ID"), "Contrat"),
];

pub struct Reference {
    pub libelle: &'static str,
    pub valeur: String,
}

/// Le contenu du premier élément `nom` d'un morceau de XML, quel que soit son préfixe (« cbc:ID » ou « ID »), sans le
/// décoder. Raven n'a pas besoin d'un vrai lecteur de XML pour ces quelques éléments simples.
fn element<'a>(xml: &'a str, nom: &str) -> Option<&'a str> {
    let mut reste = xml;
    while let Some(debut) = reste.find('<') {
        reste = &reste[debut + 1..];
        let fin_nom = reste.find(|c: char| c == '>' || c == '/' || c.is_whitespace())?;
        let balise = &reste[..fin_nom];
        if balise.is_empty() || balise.rsplit(':').next() != Some(nom) {
            continue;
        }
        let ouverture = reste.find('>')?;
        if reste[..ouverture].ends_with('/') {
            return Some("");
        }
        let contenu = &reste[ouverture + 1..];
        return contenu.find(&format!("</{balise}>")).map(|fin| &contenu[..fin]);
    }
    None
}

/// Le texte d'un élément XML : entités remplacées, espaces resserrés.
fn texte_xml(brut: &str) -> String {
    let t = brut.trim();
    let t = match t.strip_prefix("<![CDATA[").and_then(|x| x.strip_suffix("]]>")) {
        Some(cdata) => cdata.to_string(),
        None => t.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"),
    };
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Les références trouvées dans le XML reçu, dans l'ordre de `REFERENCES`.
fn references(xml: &str) -> Vec<Reference> {
    REFERENCES
        .iter()
        .filter_map(|(nom, dedans, libelle)| {
            let bloc = element(xml, nom)?;
            let brut = match dedans {
                Some(d) => element(bloc, d)?,
                None => bloc,
            };
            let valeur = texte_xml(brut);
            (!valeur.is_empty()).then_some(Reference { libelle, valeur })
        })
        .collect()
}

/// Un texte réduit à ses lettres et chiffres, en majuscules (« Maison Dupont » → « MAISONDUPONT »).
fn cle(texte: &str) -> String {
    texte.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_uppercase).collect()
}

/// Les codes d'un texte : les mots qui mêlent lettres et chiffres (« PL46 », « PL-46 » → « PL46 »), en majuscules. Un
/// nombre seul (une année, une quantité) n'en est pas un.
fn codes(texte: &str) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for mot in texte.split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '.' | '_'))) {
        for morceau in std::iter::once(mot).chain(mot.split(['-', '.', '_'])) {
            let code = cle(morceau);
            let est_code = code.chars().count() >= 3 && code.chars().any(|c| c.is_ascii_digit()) && code.chars().any(char::is_alphabetic);
            if est_code && !codes.contains(&code) {
                codes.push(code);
            }
        }
    }
    codes
}

/// Le chantier proposé d'office : celui dont la référence entière, ou un code de sa référence (« PL46 » dans
/// « PL46 Salle de bain »), se retrouve dans une référence de la facture. Les chantiers en cours d'abord ; rien n'est
/// proposé quand plusieurs correspondent. Renvoie le chantier, la référence de la facture et ce qui a été reconnu.
fn chantier_propose<'a>(references: &'a [Reference], chantiers: &[(i64, String, bool)]) -> Option<(i64, &'a Reference, String)> {
    let reconnu = |reference_chantier: &str| {
        let entiere = cle(reference_chantier);
        let codes_chantier = codes(reference_chantier);
        references.iter().find_map(|r| {
            if entiere.chars().count() >= 3 && cle(&r.valeur) == entiere {
                return Some((r, r.valeur.clone()));
            }
            let codes_facture = codes(&r.valeur);
            codes_chantier.iter().find(|c| codes_facture.contains(c)).map(|c| (r, c.clone()))
        })
    };
    for en_cours in [true, false] {
        let mut trouves = chantiers
            .iter()
            .filter(|(_, _, e)| *e == en_cours)
            .filter_map(|(id, reference, _)| reconnu(reference).map(|(r, code)| (*id, r, code)));
        match (trouves.next(), trouves.next()) {
            (Some(seul), None) => return Some(seul),
            (Some(_), Some(_)) => return None,
            _ => {}
        }
    }
    None
}

/// La fiche du fournisseur : retrouvée par son numéro de TVA ou son adresse Peppol, sinon créée (profil
/// « fournisseur (autres) », modifiable ensuite).
async fn fournisseur(db: &mut PgConnection, lu: &Lu) -> Result<i64, sqlx::Error> {
    let tva = tva_compacte(&lu.fournisseur_tva);
    let trouve: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM entreprises
         WHERE ($1 <> '' AND regexp_replace(upper(coalesce(numero_tva, '')), '[^A-Z0-9]', '', 'g') = $1) OR ($2 <> '' AND peppol_id = $2)
         ORDER BY id LIMIT 1",
    )
    .bind(&tva)
    .bind(&lu.expediteur)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(id) = trouve {
        return Ok(id);
    }
    // Sans numéro de TVA, l'adresse Peppol de l'expéditeur identifie le fournisseur.
    let peppol_id = if tva.is_empty() { peppol::lire(&lu.expediteur).ok().map(|i| i.to_string()) } else { None };
    let vide = |s: &str| (!s.is_empty()).then(|| s.to_string());
    sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, numero_tva, rue, code_postal, ville, pays, email, telephone, iban, peppol_id)
         VALUES ($1, '{fournisseur_autres}', $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(&lu.fournisseur_nom)
    .bind(vide(&lu.fournisseur_tva))
    .bind(&lu.rue)
    .bind(&lu.code_postal)
    .bind(&lu.ville)
    .bind(&lu.pays)
    .bind(vide(&lu.email))
    .bind(vide(&lu.telephone))
    .bind(vide(&lu.iban))
    .bind(peppol_id)
    .fetch_one(&mut *db)
    .await
}

async fn marquer_lu(document_id: &str) {
    if let Err(e) = peppol::appel("POST", format!("/api/v1/documents/{document_id}/mark-as-read"), Some(json!({ "read": true }))).await {
        eprintln!("Document Peppol {document_id} pas marqué comme lu : {e}");
    }
}

/// Enregistre un document reçu par la société Recommand de l'organisation. `Ok(true)` : nouvelle facture enregistrée ;
/// `Ok(false)` : déjà là, ou document qui n'est pas une facture ; `Err` : la raison lisible.
async fn importer(db: &mut PgConnection, societe_id: &str, document_id: &str) -> Result<Result<bool, String>, sqlx::Error> {
    let deja: Option<i64> = sqlx::query_scalar("SELECT id FROM factures_recues WHERE document_id = $1").bind(document_id).fetch_optional(&mut *db).await?;
    if deja.is_some() {
        marquer_lu(document_id).await;
        return Ok(Ok(false));
    }
    let v = match peppol::appel("GET", format!("/api/v1/documents/{document_id}"), None).await {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let doc = &v["document"];
    if doc["companyId"].as_str() != Some(societe_id) || doc["direction"].as_str() != Some("incoming") {
        eprintln!("Document Peppol {document_id} refusé : il n'est pas adressé à la société {societe_id}.");
        return Ok(Err("Ce document n'est pas adressé à votre entreprise.".into()));
    }
    match doc["type"].as_str() {
        Some("invoice" | "creditNote") => {}
        // L'accusé de réception d'une facture envoyée : rien à garder.
        Some("messageLevelResponse") => {
            marquer_lu(document_id).await;
            return Ok(Ok(false));
        }
        // Autre chose (autofacturation, document illisible) : laissé non lu chez Recommand, rien n'est perdu.
        _ => return Ok(Ok(false)),
    }
    let mut lu = match lire_document(doc) {
        Ok(lu) => lu,
        Err(e) => return Ok(Err(e)),
    };
    if lu.pdf.is_none() {
        lu.pdf = peppol::appel_octets(format!("/api/v1/documents/{document_id}/render/pdf")).await.ok().filter(|o| o.starts_with(b"%PDF"));
    }

    let mut tx = sqlx::Connection::begin(&mut *db).await?;
    let fournisseur_id = fournisseur(&mut tx, &lu).await?;
    sqlx::query(
        "INSERT INTO factures_recues (document_id, genre, expediteur, fournisseur_id, fournisseur_nom, fournisseur_tva, numero,
             date_facture, date_echeance, montant_ht_cents, montant_tva_cents, montant_ttc_cents, a_payer_cents, iban, communication,
             validation, donnees, xml, empreinte, pdf, recue_le)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, COALESCE($21::text::timestamptz, now()))
         ON CONFLICT (organisation_id, document_id) DO NOTHING",
    )
    .bind(document_id)
    .bind(lu.genre)
    .bind(&lu.expediteur)
    .bind(fournisseur_id)
    .bind(&lu.fournisseur_nom)
    .bind(&lu.fournisseur_tva)
    .bind(&lu.numero)
    .bind(lu.date_facture)
    .bind(lu.date_echeance)
    .bind(lu.ht)
    .bind(lu.tva)
    .bind(lu.ttc)
    .bind(lu.a_payer)
    .bind(&lu.iban)
    .bind(&lu.communication)
    .bind(&lu.validation)
    .bind(&lu.donnees)
    .bind(&lu.xml)
    .bind(hex::encode(Sha256::digest(lu.xml.as_bytes())))
    .bind(&lu.pdf)
    .bind(Some(lu.recue_le.as_str()).filter(|t| !t.is_empty()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    marquer_lu(document_id).await;
    Ok(Ok(true))
}

/// Le résultat d'une relève.
#[derive(Default)]
pub struct Releve {
    pub nouvelles: usize,
    pub erreurs: Vec<String>,
}

/// Relève la boîte de réception de l'organisation chez Recommand (si la réception est allumée).
pub(crate) async fn relever(db: &mut PgConnection) -> Result<Releve, sqlx::Error> {
    let mut releve = Releve::default();
    let i = peppol::inscription(&mut *db).await?;
    let (Some(societe_id), true, true) = (i.societe_id, i.reception, peppol::envoi_actif()) else { return Ok(releve) };
    match peppol::appel("GET", format!("/api/v1/inbox?companyId={societe_id}"), None).await {
        Ok(v) => {
            for d in v["documents"].as_array().into_iter().flatten() {
                let Some(id) = d["id"].as_str() else { continue };
                if d["direction"].as_str() != Some("incoming") || d["companyId"].as_str() != Some(&societe_id) {
                    continue;
                }
                match importer(&mut *db, &societe_id, id).await? {
                    Ok(true) => releve.nouvelles += 1,
                    Ok(false) => {}
                    Err(e) => releve.erreurs.push(e),
                }
            }
        }
        Err(e) => releve.erreurs.push(e),
    }
    sqlx::query("UPDATE organisations SET peppol_releve_le = now() WHERE id = app_organisation()").execute(&mut *db).await?;
    Ok(releve)
}

// ─── Le webhook de Recommand ────────────────────────────────────────────────

/// HMAC SHA-256 (RFC 2104), sans bibliothèque de plus : la clé est complétée à 64 octets.
fn hmac_sha256(cle: &[u8], message: &[u8]) -> Vec<u8> {
    let mut k = [0u8; 64];
    if cle.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(cle));
    } else {
        k[..cle.len()].copy_from_slice(cle);
    }
    let mut interieur = Sha256::new();
    interieur.update(k.map(|b| b ^ 0x36));
    interieur.update(message);
    let mut exterieur = Sha256::new();
    exterieur.update(k.map(|b| b ^ 0x5c));
    exterieur.update(interieur.finalize());
    exterieur.finalize().to_vec()
}

/// L'en-tête `X-Signature: sha256=<hex>` correspond au corps brut (comparaison en temps constant).
fn signature_valide(secret: &str, corps: &[u8], signature: &str) -> bool {
    let attendue = format!("sha256={}", hex::encode(hmac_sha256(secret.as_bytes(), corps)));
    let recue = signature.trim().as_bytes();
    recue.len() == attendue.len() && recue.iter().zip(attendue.as_bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// `POST /peppol/webhook` : une notification de Recommand. Une facture arrivée est enregistrée tout de suite ; une
/// vérification d'identité terminée met à jour Réglages › Peppol. Une erreur de traitement ne fait pas perdre la
/// facture : elle reste non lue chez Recommand et arrive à la prochaine relève.
async fn webhook(State(state): State<AppState>, entetes: HeaderMap, corps: Bytes) -> StatusCode {
    let Some(secret) = peppol::secret_webhook() else { return StatusCode::NOT_FOUND };
    let signature = entetes.get("x-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !signature_valide(secret, &corps, signature) {
        eprintln!("Webhook Peppol refusé : signature absente ou fausse.");
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(evenement) = serde_json::from_slice::<Value>(&corps) else { return StatusCode::BAD_REQUEST };
    let Some(societe_id) = evenement["companyId"].as_str() else { return StatusCode::OK };
    // `organisations` est une table commune : on y cherche l'organisation de la société, puis tout se fait dans elle.
    let organisation: Result<Option<(i64, bool)>, sqlx::Error> =
        sqlx::query_as("SELECT id, peppol_reception FROM organisations WHERE peppol_societe_id = $1").bind(societe_id).fetch_optional(&state.db).await;
    let (organisation, reception) = match organisation {
        Ok(Some(o)) => o,
        Ok(None) => return StatusCode::OK,
        Err(e) => {
            eprintln!("Webhook Peppol : {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    let resultat = async {
        let mut db = Base::ouvrir(&state.db, organisation).await?;
        match evenement["eventType"].as_str() {
            Some("document.received") if reception => {
                if let Some(document_id) = evenement["documentId"].as_str()
                    && let Err(e) = importer(&mut db, societe_id, document_id).await?
                {
                    eprintln!("Webhook Peppol : document {document_id} pas enregistré : {e}");
                }
            }
            Some("company.verification") => {
                if let Some(e) = peppol::actualiser_organisation(&mut db).await? {
                    eprintln!("Webhook Peppol : vérification pas relue : {e}");
                }
            }
            _ => {}
        }
        Ok::<(), sqlx::Error>(())
    }
    .await;
    match resultat {
        Ok(()) => StatusCode::OK,
        Err(e) => {
            eprintln!("Webhook Peppol : {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

// ─── La boîte de réception ──────────────────────────────────────────────────

/// Le signe d'une pièce : une note de crédit fournisseur compte en négatif.
fn signe(genre: &str) -> i64 {
    if genre == "note_credit" { -1 } else { 1 }
}

fn libelle_genre(genre: &str) -> &'static str {
    if genre == "note_credit" { "Note de crédit" } else { "Facture" }
}

/// Le reste à répartir d'une facture reçue : sa base HT (avec son signe), moins ce qui est rattaché aux chantiers et
/// classé sans chantier. Zéro : la facture est traitée.
const RESTE: &str = "(CASE WHEN fr.genre = 'note_credit' THEN -fr.montant_ht_cents ELSE fr.montant_ht_cents END) - fr.frais_generaux_cents
     - COALESCE((SELECT sum(a.montant_ht_cents) FROM chantier_achats a WHERE a.facture_recue_id = fr.id), 0)::bigint";

pub struct LigneBoite {
    pub id: i64,
    pub recue: String,
    pub fournisseur: String,
    pub numero: String,
    pub genre: &'static str,
    pub date: String,
    pub echeance: String,
    pub ht: String,
    pub ttc: String,
    pub a_traiter: bool,
    pub reste: String,
    /// La pastille de paiement (vide pour une note de crédit) et son détail (« échue le … · 12 jours »).
    pub paiement: &'static str,
    pub paiement_classe: &'static str,
    pub paiement_detail: String,
}

#[derive(FromRow)]
struct LigneSql {
    id: i64,
    recue: String,
    fournisseur_nom: String,
    numero: String,
    genre: String,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    montant_ht_cents: i64,
    montant_ttc_cents: i64,
    reste: i64,
    #[sqlx(flatten)]
    solde: paiements_fournisseurs::Solde,
}

#[derive(Template)]
#[template(path = "factures_recues.html")]
struct PageBoite {
    vue: String,
    q: String,
    lignes: Vec<LigneBoite>,
    a_traiter: i64,
    traitees: i64,
    a_payer: i64,
    payees: i64,
    toutes: i64,
    /// Ce qui reste à payer aux fournisseurs (toutes les factures reçues), et la part en retard.
    total_a_payer: String,
    total_en_retard: String,
    nombre_en_retard: i64,
    /// La réception est allumée et Peppol configuré : la boîte se relève.
    reception: bool,
    releve_le: String,
    message_ok: String,
    message_erreur: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RechercheBoite {
    vue: String,
    q: String,
    ok: String,
    nouvelles: usize,
}

async fn page_boite(db: &mut Base, r: RechercheBoite, message_erreur: String) -> Result<Html<String>, Erreur> {
    let vue = match r.vue.as_str() {
        "traitees" | "a_payer" | "payees" | "toutes" => r.vue.clone(),
        _ => "a_traiter".to_string(),
    };
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    let (a_traiter, traitees, a_payer, payees): (i64, i64, i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FILTER (WHERE ({RESTE}) <> 0), count(*) FILTER (WHERE ({RESTE}) = 0),
                count(*) FILTER (WHERE e.reste_cents > 0), count(*) FILTER (WHERE e.etat = 'payee')
         FROM factures_recues fr JOIN factures_recues_etats e ON e.facture_recue_id = fr.id"
    )))
    .fetch_one(&mut **db)
    .await?;
    // « À payer » : les plus urgentes d'abord (échéance la plus proche) ; ailleurs, les plus récemment reçues.
    let lignes: Vec<LigneSql> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT fr.id, to_char(fr.recue_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY') AS recue, fr.fournisseur_nom, fr.numero, fr.genre,
                fr.date_facture, fr.date_echeance, fr.montant_ht_cents, fr.montant_ttc_cents, {RESTE} AS reste,
                e.paye_cents, e.reste_cents, e.dernier_paiement, e.etat, e.jours_retard
         FROM factures_recues fr JOIN factures_recues_etats e ON e.facture_recue_id = fr.id
         WHERE CASE $1 WHEN 'toutes' THEN true
                       WHEN 'a_traiter' THEN ({RESTE}) <> 0
                       WHEN 'traitees' THEN ({RESTE}) = 0
                       WHEN 'a_payer' THEN e.reste_cents > 0
                       ELSE e.etat = 'payee' END
           AND ($2 = '' OR fr.fournisseur_nom ILIKE $3 OR fr.numero ILIKE $3)
         ORDER BY CASE WHEN $1 = 'a_payer' THEN fr.date_echeance END ASC NULLS LAST, fr.recue_le DESC, fr.id DESC LIMIT 200"
    )))
    .bind(&vue)
    .bind(&q)
    .bind(&motif)
    .fetch_all(&mut **db)
    .await?;
    let totaux = paiements_fournisseurs::totaux(&mut *db).await?;
    let i = peppol::inscription(&mut *db).await?;
    let releve_le: Option<String> = sqlx::query_scalar(
        "SELECT to_char(peppol_releve_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI') FROM organisations WHERE id = app_organisation()",
    )
    .fetch_one(&mut **db)
    .await?;
    let page = PageBoite {
        vue,
        q,
        lignes: lignes
            .into_iter()
            .map(|l| {
                let (paiement, paiement_classe) = paiements_fournisseurs::etat(&l.solde.etat);
                LigneBoite {
                    id: l.id,
                    recue: l.recue,
                    fournisseur: l.fournisseur_nom,
                    numero: l.numero,
                    genre: libelle_genre(&l.genre),
                    date: l.date_facture.format("%d/%m/%Y").to_string(),
                    echeance: l.date_echeance.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(),
                    ht: format_euros(signe(&l.genre) * l.montant_ht_cents),
                    ttc: format_euros(signe(&l.genre) * l.montant_ttc_cents),
                    a_traiter: l.reste != 0,
                    reste: format_euros(l.reste),
                    paiement,
                    paiement_classe,
                    paiement_detail: l.solde.detail(l.date_echeance),
                }
            })
            .collect(),
        a_traiter,
        traitees,
        a_payer,
        payees,
        toutes: a_traiter + traitees,
        total_a_payer: format_euros(totaux.a_payer_cents),
        total_en_retard: format_euros(totaux.en_retard_cents),
        nombre_en_retard: totaux.nombre_en_retard,
        reception: i.reception && i.societe_id.is_some() && peppol::envoi_actif(),
        releve_le: releve_le.unwrap_or_default(),
        message_ok: match (r.ok.as_str(), r.nouvelles) {
            ("releve", 0) => "Relève faite : aucune nouvelle facture.".into(),
            ("releve", 1) => "Relève faite : 1 nouvelle facture.".into(),
            ("releve", n) => format!("Relève faite : {n} nouvelles factures."),
            _ => String::new(),
        },
        message_erreur,
    };
    Ok(Html(page.render()?))
}

fn erreurs_releve(r: &Releve) -> String {
    if r.erreurs.is_empty() { String::new() } else { format!("La relève chez Recommand n'a pas tout pris : {}", r.erreurs.join(" ; ")) }
}

/// La boîte de réception. L'ouvrir relève d'abord les nouvelles factures chez Recommand.
async fn boite(mut db: Base, Query(r): Query<RechercheBoite>) -> Result<Html<String>, Erreur> {
    let erreur = if r.ok.is_empty() { erreurs_releve(&relever(&mut db).await?) } else { String::new() };
    page_boite(&mut db, r, erreur).await
}

/// Le bouton « Relever ».
async fn relever_page(mut db: Base) -> Result<Response, Erreur> {
    let releve = relever(&mut db).await?;
    if releve.erreurs.is_empty() {
        return Ok(Redirect::to(&format!("/achats/recues?ok=releve&nouvelles={}", releve.nouvelles)).into_response());
    }
    let erreur = erreurs_releve(&releve);
    Ok(page_boite(&mut db, RechercheBoite { ok: "releve".into(), nouvelles: releve.nouvelles, ..Default::default() }, erreur).await?.into_response())
}

// ─── Une facture reçue ──────────────────────────────────────────────────────

#[derive(FromRow)]
struct Recue {
    id: i64,
    genre: String,
    expediteur: String,
    fournisseur_id: Option<i64>,
    fournisseur_nom: String,
    fournisseur_tva: String,
    numero: String,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    montant_ht_cents: i64,
    montant_tva_cents: i64,
    montant_ttc_cents: i64,
    a_payer_cents: i64,
    iban: String,
    communication: String,
    validation: String,
    donnees: Value,
    empreinte: String,
    a_pdf: bool,
    recue: String,
    frais_generaux_cents: i64,
    reste: i64,
}

async fn charger(db: &mut PgConnection, id: i64) -> Result<Option<Recue>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT fr.id, fr.genre, fr.expediteur, fr.fournisseur_id, fr.fournisseur_nom, fr.fournisseur_tva, fr.numero, fr.date_facture,
                fr.date_echeance, fr.montant_ht_cents, fr.montant_tva_cents, fr.montant_ttc_cents, fr.a_payer_cents, fr.iban,
                fr.communication, fr.validation, fr.donnees, fr.empreinte, fr.pdf IS NOT NULL AS a_pdf,
                to_char(fr.recue_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI') AS recue, fr.frais_generaux_cents,
                ({RESTE}) AS reste
         FROM factures_recues fr WHERE fr.id = $1"
    )))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Un montant de `donnees` (texte de Recommand) affiché à la belge ; vide s'il manque.
fn montant(v: &Value) -> String {
    centimes(v).map(format_euros).unwrap_or_default()
}

/// Une quantité ou un prix unitaire tel que le fournisseur l'a écrit (« 2.5 » → « 2,5 »).
fn decimal(v: &Value) -> String {
    let t = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return String::new(),
    };
    let t = match t.split_once('.') {
        Some((e, d)) => {
            let d = d.trim_end_matches('0');
            if d.is_empty() { e.to_string() } else { format!("{e},{d}") }
        }
        None => t,
    };
    t
}

/// L'unité d'une ligne (codes UN/ECE de Peppol) en mots courants.
fn unite(code: &str) -> String {
    match code {
        "C62" | "H87" | "EA" | "XPP" => "pièce".into(),
        "HUR" => "h".into(),
        "DAY" => "jour".into(),
        "MTR" => "m".into(),
        "MTK" => "m²".into(),
        "MTQ" => "m³".into(),
        "KGM" => "kg".into(),
        "LTR" => "l".into(),
        "TNE" => "t".into(),
        "SET" => "ensemble".into(),
        "LS" => "forfait".into(),
        autre => autre.to_string(),
    }
}

pub struct LigneDocument {
    pub description: String,
    pub detail: String,
    pub quantite: String,
    pub unite: String,
    pub prix: String,
    pub total: String,
    pub tva: String,
}

pub struct TauxDocument {
    pub taux: String,
    pub base: String,
    pub tva: String,
}

pub struct Part {
    pub achat_id: Option<i64>,
    pub chantier_id: Option<i64>,
    pub destination: String,
    pub categorie: &'static str,
    pub description: String,
    pub montant: String,
}

#[derive(Template)]
#[template(path = "facture_recue.html")]
struct PageRecue {
    id: i64,
    genre: &'static str,
    note_credit: bool,
    expediteur: String,
    fournisseur_id: Option<i64>,
    fournisseur: String,
    fournisseur_tva: String,
    numero: String,
    date: String,
    echeance: String,
    ht: String,
    tva: String,
    ttc: String,
    a_payer: String,
    iban: String,
    communication: String,
    validation: &'static str,
    validation_ok: bool,
    remarque: String,
    lignes: Vec<LigneDocument>,
    taux: Vec<TauxDocument>,
    empreinte: String,
    a_pdf: bool,
    recue: String,
    parts: Vec<Part>,
    traitee: bool,
    reste: String,
    reste_saisie: String,
    chantiers: Vec<Choix>,
    categories: Vec<Choix>,
    /// Les références recopiées par le fournisseur (référence de l'acheteur, bon de commande…).
    references: Vec<Reference>,
    /// « Proposé d'office : … » quand une référence de la facture désigne un seul chantier ; vide sinon.
    proposition: String,
    /// Le paiement (pas pour une note de crédit) : pastille, montants, échéance, paiements déjà faits.
    paiement: &'static str,
    paiement_classe: &'static str,
    paye: String,
    reste_a_payer: String,
    reste_a_payer_saisie: String,
    a_payer_encore: bool,
    en_retard: bool,
    jours_retard: i32,
    paiements: Vec<crate::paiements::LignePaiement>,
    methodes: Vec<Choix>,
    aujourdhui: String,
    payer_ouvert: bool,
    message_ok: &'static str,
    message_erreur: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
    payer: String,
}

async fn afficher(mut db: Base, Path(id): Path<i64>, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let f = charger(&mut db, id).await?.ok_or_else(Erreur::introuvable)?;
    let d = &f.donnees;
    let lignes = d["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let nom = texte(&l["name"]);
            let description = texte(&l["description"]);
            let (titre, detail) = if nom.is_empty() { (description, String::new()) } else { (nom, description) };
            LigneDocument {
                description: titre,
                detail,
                quantite: decimal(&l["quantity"]),
                unite: unite(&texte(&l["unitCode"])),
                prix: decimal(&l["netPriceAmount"]),
                total: montant(&l["netAmount"]),
                tva: Some(decimal(&l["vat"]["percentage"])).filter(|t| !t.is_empty()).map(|t| format!("{t} %")).unwrap_or_default(),
            }
        })
        .collect();
    let taux = d["vat"]["subtotals"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let pourcent = decimal(&s["percentage"]);
            let categorie = texte(&s["category"]);
            TauxDocument {
                taux: match categorie.as_str() {
                    "AE" => format!("{pourcent} % (autoliquidation)"),
                    "E" => "Exonéré".into(),
                    "Z" | "" | "S" => format!("{pourcent} %"),
                    autre => format!("{pourcent} % ({autre})"),
                },
                base: montant(&s["taxableAmount"]),
                tva: montant(&s["vatAmount"]),
            }
        })
        .collect();

    let achats: Vec<(i64, i64, String, String, String, i64)> = sqlx::query_as(
        "SELECT a.id, a.chantier_id, CASE WHEN dv.reference <> '' THEN dv.reference || ' (n° ' || ch.numero || ')' ELSE 'n° ' || ch.numero END,
                a.categorie::text, a.description, a.montant_ht_cents
         FROM chantier_achats a JOIN chantiers ch ON ch.id = a.chantier_id JOIN devis dv ON dv.id = ch.devis_id
         WHERE a.facture_recue_id = $1 ORDER BY a.id",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?;
    let mot = crate::profil::mot("Chantier");
    let mut parts: Vec<Part> = achats
        .into_iter()
        .map(|(achat_id, chantier_id, chantier, categorie, description, montant)| Part {
            achat_id: Some(achat_id),
            chantier_id: Some(chantier_id),
            destination: format!("{mot} {chantier}"),
            categorie: libelle_categorie(&categorie),
            description,
            montant: format_euros(montant),
        })
        .collect();
    if f.frais_generaux_cents != 0 {
        parts.push(Part {
            achat_id: None,
            chantier_id: None,
            destination: format!("Sans {} (frais généraux)", crate::profil::mot("chantier")),
            categorie: "",
            description: String::new(),
            montant: format_euros(f.frais_generaux_cents),
        });
    }
    let chantiers: Vec<(i64, i64, String, bool)> = sqlx::query_as(
        "SELECT ch.id, ch.numero, d.reference, ch.statut = 'en_cours' FROM chantiers ch JOIN devis d ON d.id = ch.devis_id
         ORDER BY ch.statut = 'en_cours' DESC, ch.numero DESC",
    )
    .fetch_all(&mut *db)
    .await?;
    let libelle_chantier = |numero: i64, reference: &str| {
        if reference.is_empty() { format!("{mot} n° {numero}") } else { format!("{reference} (n° {numero})") }
    };

    // Le chantier proposé d'office : une référence de la facture désigne un seul chantier (seulement avec le module
    // des achats, qui permet de rattacher).
    let xml: String = sqlx::query_scalar("SELECT xml FROM factures_recues WHERE id = $1").bind(id).fetch_one(&mut *db).await?;
    let references = references(&xml);
    let propose = if crate::profil::actuel().achats && f.reste != 0 {
        let candidats: Vec<(i64, String, bool)> = chantiers.iter().map(|(id, _, reference, en_cours)| (*id, reference.clone(), *en_cours)).collect();
        chantier_propose(&references, &candidats)
    } else {
        None
    };
    let proposition = match &propose {
        Some((chantier, r, code)) => chantiers
            .iter()
            .find(|(c, _, _, _)| c == chantier)
            .map(|(_, numero, reference, _)| {
                format!(
                    "Proposé d'office : {mot} {}, car la facture porte la référence « {code} » ({}).",
                    libelle_chantier(*numero, reference),
                    r.libelle.to_lowercase()
                )
            })
            .unwrap_or_default(),
        None => String::new(),
    };

    let solde = paiements_fournisseurs::solde(&mut *db, id).await?;
    let (paiement, paiement_classe) = paiements_fournisseurs::etat(&solde.etat);
    let paiements = paiements_fournisseurs::lignes(&mut *db, id).await?;
    let methodes = crate::paiements::methodes(&mut *db).await?;
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await?;
    let (validation, validation_ok) = match f.validation.as_str() {
        "valid" => ("Conforme (contrôlée par Recommand : EN 16931 et Peppol BIS 3.0)", true),
        "invalid" => ("Non conforme aux règles Peppol : vérifiez-la avec le fournisseur avant de la payer", false),
        "" => ("", true),
        _ => ("Pas contrôlée par Recommand", true),
    };
    let page = PageRecue {
        id: f.id,
        genre: libelle_genre(&f.genre),
        note_credit: f.genre == "note_credit",
        expediteur: f.expediteur,
        fournisseur_id: f.fournisseur_id,
        fournisseur: f.fournisseur_nom,
        fournisseur_tva: f.fournisseur_tva,
        numero: f.numero,
        date: f.date_facture.format("%d/%m/%Y").to_string(),
        echeance: f.date_echeance.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(),
        ht: format_euros(f.montant_ht_cents),
        tva: format_euros(f.montant_tva_cents),
        ttc: format_euros(f.montant_ttc_cents),
        a_payer: format_euros(f.a_payer_cents),
        iban: f.iban,
        communication: f.communication,
        validation,
        validation_ok,
        remarque: texte(&d["note"]),
        lignes,
        taux,
        empreinte: f.empreinte,
        a_pdf: f.a_pdf,
        recue: f.recue,
        parts,
        traitee: f.reste == 0,
        reste: format_euros(f.reste),
        reste_saisie: format_euros(f.reste.abs()).trim_end_matches("\u{a0}€").replace('\u{a0}', ""),
        chantiers: chantiers
            .iter()
            .map(|(id, numero, reference, _)| Choix {
                valeur: id.to_string(),
                libelle: libelle_chantier(*numero, reference),
                choisi: propose.as_ref().is_some_and(|(c, _, _)| c == id),
            })
            .collect(),
        categories: CATEGORIES
            .iter()
            .enumerate()
            .map(|(i, (v, l))| Choix { valeur: (*v).into(), libelle: crate::profil::categorie(l).into(), choisi: i == 0 })
            .collect(),
        references,
        proposition,
        paiement,
        paiement_classe,
        paye: format_euros(solde.paye_cents),
        reste_a_payer: format_euros(solde.reste_cents),
        reste_a_payer_saisie: crate::avancement_calcul::euros_a_saisir(solde.reste_cents),
        a_payer_encore: solde.reste_cents > 0,
        en_retard: solde.etat == "en_retard",
        jours_retard: solde.jours_retard,
        paiements,
        methodes: methodes.iter().enumerate().map(|(i, (v, l))| Choix { valeur: v.to_string(), libelle: l.clone(), choisi: i == 0 }).collect(),
        aujourdhui: aujourdhui.format("%Y-%m-%d").to_string(),
        payer_ouvert: (r.payer == "1" || r.erreur.starts_with("paiement_")) && solde.reste_cents > 0,
        message_ok: match r.ok.as_str() {
            "rattachee" => "La part est enregistrée.",
            "annulee" => "La répartition est annulée : la facture est de nouveau à traiter.",
            "paiement" => "Le paiement est enregistré.",
            "payee" => "Le paiement est enregistré : la facture est payée.",
            "paiement_retire" => "Le paiement est retiré.",
            _ => "",
        },
        message_erreur: match r.erreur.as_str() {
            "montant" => "La part n'est pas enregistrée : indiquez un montant HT positif, au plus le reste à répartir.",
            "destination" => "Choisissez où va cette part.",
            "paiement_date" => "Le paiement n'est pas enregistré : la date est vide ou dans le futur.",
            "paiement_montant" => "Le paiement n'est pas enregistré : le montant doit être positif et ne pas dépasser le reste à payer.",
            "paiement_methode" => "Le paiement n'est pas enregistré : choisissez une méthode de paiement.",
            "paiement_remarque" => "Le paiement n'est pas enregistré : la remarque est trop longue (500 caractères au maximum).",
            _ => "",
        },
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DemandePart {
    /// L'id du chantier, ou « sans » (frais généraux).
    destination: String,
    montant: String,
    categorie: String,
    description: String,
}

/// Rattache une part de la facture (toute la base HT par défaut) à un chantier, ou la classe sans chantier.
async fn rattacher(mut db: Base, Path(id): Path<i64>, Form(d): Form<DemandePart>) -> Result<Redirect, Erreur> {
    let f = charger(&mut db, id).await?.ok_or_else(Erreur::introuvable)?;
    let retour = |erreur: &str| Redirect::to(&format!("/achats/recues/{id}?erreur={erreur}#repartition"));
    let Some(montant) = euros_vers_centimes(&d.montant).filter(|m| *m > 0 && *m <= f.reste.abs()) else { return Ok(retour("montant")) };
    // La part prend le signe du reste (une note de crédit compte en négatif).
    let part = if f.reste < 0 { -montant } else { montant };
    let destination = d.destination.trim();
    if destination == "sans" {
        sqlx::query("UPDATE factures_recues SET frais_generaux_cents = frais_generaux_cents + $2 WHERE id = $1").bind(id).bind(part).execute(&mut *db).await?;
    } else {
        // Rattacher à un chantier, c'est en faire un achat : seulement avec le module des achats.
        let chantier: Option<i64> = match destination.parse::<i64>() {
            Ok(c) if crate::profil::actuel().achats => sqlx::query_scalar("SELECT id FROM chantiers WHERE id = $1").bind(c).fetch_optional(&mut *db).await?,
            _ => None,
        };
        let Some(chantier) = chantier else { return Ok(retour("destination")) };
        let categorie = CATEGORIES.iter().map(|(c, _)| *c).find(|c| *c == d.categorie.trim()).unwrap_or("fourniture");
        let description: String = match d.description.trim() {
            "" if f.numero.is_empty() => libelle_genre(&f.genre).to_string(),
            "" => format!("{} n° {}", libelle_genre(&f.genre), f.numero),
            t => t.chars().take(300).collect(),
        };
        let fournisseur: String = f.fournisseur_nom.chars().take(300).collect();
        sqlx::query(
            "INSERT INTO chantier_achats (chantier_id, date_achat, categorie, fournisseur, description, montant_ht_cents, facture_recue_id)
             VALUES ($1, $2, $3::text::categorie_achat, $4, $5, $6, $7)",
        )
        .bind(chantier)
        .bind(f.date_facture)
        .bind(categorie)
        .bind(&fournisseur)
        .bind(&description)
        .bind(part)
        .bind(id)
        .execute(&mut *db)
        .await?;
    }
    Ok(Redirect::to(&format!("/achats/recues/{id}?ok=rattachee#repartition")))
}

/// Annule toute la répartition : les achats créés depuis la facture sont retirés de leurs chantiers.
async fn annuler(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    charger(&mut db, id).await?.ok_or_else(Erreur::introuvable)?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM chantier_achats WHERE facture_recue_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE factures_recues SET frais_generaux_cents = 0 WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/achats/recues/{id}?ok=annulee#repartition")))
}

fn fichier(contenu: Vec<u8>, type_mime: &'static str, nom: String, telecharger: bool) -> Result<Response, Erreur> {
    let mut reponse = contenu.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static(type_mime));
    let disposition = format!("{}; filename=\"{nom}\"", if telecharger { "attachment" } else { "inline" });
    entetes.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disposition).map_err(|_| Erreur::interne())?);
    Ok(reponse)
}

async fn pdf(mut db: Base, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let pdf: Option<Option<Vec<u8>>> = sqlx::query_scalar("SELECT pdf FROM factures_recues WHERE id = $1").bind(id).fetch_optional(&mut *db).await?;
    fichier(pdf.flatten().ok_or_else(Erreur::introuvable)?, "application/pdf", format!("Facture-fournisseur-{id}.pdf"), false)
}

async fn xml(mut db: Base, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let xml: Option<String> = sqlx::query_scalar("SELECT xml FROM factures_recues WHERE id = $1").bind(id).fetch_optional(&mut *db).await?;
    fichier(xml.ok_or_else(Erreur::introuvable)?.into_bytes(), "application/xml", format!("Facture-fournisseur-{id}.xml"), true)
}

// ─── Pour les autres écrans ─────────────────────────────────────────────────

/// Le nombre de factures reçues à traiter (tableau de bord, liste des achats).
pub(crate) async fn nombre_a_traiter(db: &mut PgConnection) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM factures_recues fr WHERE ({RESTE}) <> 0"))).fetch_one(db).await
}

/// Une facture reçue dans un export pour le comptable.
#[derive(FromRow)]
pub(crate) struct PieceRecue {
    pub id: i64,
    genre: String,
    fournisseur_nom: String,
    fournisseur_tva: String,
    numero: String,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    montant_ht_cents: i64,
    montant_tva_cents: i64,
    montant_ttc_cents: i64,
    iban: String,
    communication: String,
    xml: String,
    pdf: Option<Vec<u8>>,
}

/// Les factures reçues datées dans la période (toutes, ou celles jamais exportées).
pub(crate) async fn pour_export(db: &mut PgConnection, debut: NaiveDate, fin: NaiveDate, seulement_nouvelles: bool) -> Result<Vec<PieceRecue>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, genre, fournisseur_nom, fournisseur_tva, numero, date_facture, date_echeance, montant_ht_cents, montant_tva_cents,
                montant_ttc_cents, iban, communication, xml, pdf
         FROM factures_recues fr
         WHERE date_facture BETWEEN $1 AND $2
           AND (NOT $3 OR NOT EXISTS (SELECT 1 FROM export_factures_recues e WHERE e.facture_recue_id = fr.id))
         ORDER BY date_facture, id",
    )
    .bind(debut)
    .bind(fin)
    .bind(seulement_nouvelles)
    .fetch_all(db)
    .await
}

/// « Achat-12-BIG-2026-1234 » : le numéro de la pièce dans Raven (unique) et celui du fournisseur, en caractères sûrs.
fn nom_export(p: &PieceRecue) -> String {
    let numero: String = p.numero.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).take(40).collect();
    let prefixe = if p.genre == "note_credit" { "Achat-NC" } else { "Achat" };
    if numero.is_empty() { format!("{prefixe}-{}", p.id) } else { format!("{prefixe}-{}-{numero}", p.id) }
}

fn nombre_csv(centimes: i64) -> String {
    let signe = if centimes < 0 { "-" } else { "" };
    let a = centimes.unsigned_abs();
    format!("{signe}{},{:02}", a / 100, a % 100)
}

fn champ_csv(texte: &str) -> String {
    if texte.contains([';', '"', '\n', '\r']) { format!("\"{}\"", texte.replace('"', "\"\"")) } else { texte.to_string() }
}

/// Les fichiers des achats dans le dossier ZIP : `Achats/UBL/…xml` (le XML reçu, tel quel), `Achats/PDF/…pdf` et
/// `Achats/achats.csv` (Excel : point-virgule, virgule décimale, notes de crédit en négatif).
pub(crate) fn fichiers_export(pieces: &[PieceRecue], format: &str) -> Vec<(String, Vec<u8>)> {
    if pieces.is_empty() {
        return Vec::new();
    }
    let mut csv = String::from("\u{feff}Type;Fichier;Fournisseur;TVA fournisseur;Numéro;Date;Échéance;Montant HT;TVA;Montant TTC;IBAN;Communication\n");
    let mut fichiers = Vec::new();
    for p in pieces {
        let nom = nom_export(p);
        let s = signe(&p.genre);
        let colonnes = [
            libelle_genre(&p.genre).to_string(),
            nom.clone(),
            p.fournisseur_nom.clone(),
            p.fournisseur_tva.clone(),
            p.numero.clone(),
            p.date_facture.format("%d/%m/%Y").to_string(),
            p.date_echeance.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(),
            nombre_csv(s * p.montant_ht_cents),
            nombre_csv(s * p.montant_tva_cents),
            nombre_csv(s * p.montant_ttc_cents),
            p.iban.clone(),
            p.communication.clone(),
        ];
        csv.push_str(&colonnes.iter().map(|c| champ_csv(c)).collect::<Vec<_>>().join(";"));
        csv.push('\n');
        if format != "pdf" {
            fichiers.push((format!("Achats/UBL/{nom}.xml"), p.xml.clone().into_bytes()));
        }
        if format != "ubl"
            && let Some(pdf) = &p.pdf
        {
            fichiers.push((format!("Achats/PDF/{nom}.pdf"), pdf.clone()));
        }
    }
    fichiers.insert(0, ("Achats/achats.csv".to_string(), csv.into_bytes()));
    fichiers
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/peppol/webhook", post(webhook))
        .route("/achats/recues", get(boite))
        .route("/achats/recues/relever", post(relever_page))
        .route("/achats/recues/{id}", get(afficher))
        .route("/achats/recues/{id}/rattacher", post(rattacher))
        .route("/achats/recues/{id}/annuler", post(annuler))
        .route("/achats/recues/{id}/pdf", get(pdf))
        .route("/achats/recues/{id}/xml", get(xml))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn montants_de_recommand() {
        assert_eq!(centimes(&json!("21.00")), Some(2_100));
        assert_eq!(centimes(&json!("1234.5")), Some(123_450));
        assert_eq!(centimes(&json!("-3.10")), Some(-310));
        assert_eq!(centimes(&json!("0.125")), Some(13), "arrondi au centime");
        assert_eq!(centimes(&json!("7")), Some(700));
        assert_eq!(centimes(&json!(12.5)), Some(1_250));
        assert_eq!(centimes(&json!("abc")), None);
        assert_eq!(centimes(&Value::Null), None);
    }

    #[test]
    fn signature_hmac() {
        // RFC 4231, cas n° 2.
        assert_eq!(hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let corps = br#"{"eventType":"document.received"}"#;
        let bonne = format!("sha256={}", hex::encode(hmac_sha256(b"secret", corps)));
        assert!(signature_valide("secret", corps, &bonne));
        assert!(!signature_valide("autre", corps, &bonne));
        assert!(!signature_valide("secret", b"{}", &bonne));
        assert!(!signature_valide("secret", corps, ""));
    }

    #[test]
    fn lecture_d_une_facture() {
        let pdf = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4 essai");
        let doc = json!({
            "type": "invoice", "senderId": "0208:0123456749", "createdAt": "2026-10-21T09:30:00.000Z", "xml": "<Invoice/>",
            "validation": { "result": "valid", "errors": [] },
            "parsed": {
                "invoiceNumber": "F-2026-77", "issueDate": "2026-10-20", "dueDate": "2026-11-19",
                "seller": { "name": "Matériaux SA", "vatNumber": "BE0123456749", "street": "Rue du Dépôt 4", "city": "Wavre", "postalZone": "1300", "country": "BE" },
                "lines": [{ "name": "Ciment", "quantity": "10.00", "unitCode": "KGM", "netPriceAmount": "12.40", "netAmount": "124.00", "vat": { "percentage": "21.00" } }],
                "totals": { "taxExclusiveAmount": "124.00", "taxInclusiveAmount": "150.04", "payableAmount": "150.04" },
                "vat": { "totalVatAmount": "26.04", "subtotals": [{ "taxableAmount": "124.00", "vatAmount": "26.04", "category": "S", "percentage": "21.00" }] },
                "paymentMeans": [{ "iban": "BE68539007547034", "reference": "+++123/4567/89002+++" }],
                "attachments": [{ "id": "1", "filename": "f.pdf", "mimeCode": "application/pdf", "embeddedDocument": pdf }]
            }
        });
        let lu = lire_document(&doc).unwrap();
        assert_eq!((lu.genre, lu.numero.as_str(), lu.fournisseur_nom.as_str()), ("facture", "F-2026-77", "Matériaux SA"));
        assert_eq!((lu.ht, lu.tva, lu.ttc, lu.a_payer), (12_400, 2_604, 15_004, 15_004));
        assert_eq!(lu.date_facture, NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());
        assert_eq!(lu.pdf.as_deref(), Some(&b"%PDF-1.4 essai"[..]));
        assert!(lu.donnees["attachments"][0]["embeddedDocument"].is_null(), "le contenu du PDF ne reste pas dans les données");
        assert_eq!((lu.iban.as_str(), lu.validation.as_str()), ("BE68539007547034", "valid"));

        // Sans totaux : recalculés depuis la TVA. Une note de crédit garde des montants positifs.
        let mut nc = doc.clone();
        nc["type"] = json!("creditNote");
        nc["parsed"]["creditNoteNumber"] = json!("NC-3");
        nc["parsed"]["totals"] = Value::Null;
        let lu = lire_document(&nc).unwrap();
        assert_eq!((lu.genre, lu.numero.as_str(), lu.ht, lu.tva, lu.ttc), ("note_credit", "NC-3", 12_400, 2_604, 15_004));

        let mut autre = doc;
        autre["type"] = json!("selfBillingInvoice");
        assert!(lire_document(&autre).is_err());
    }

    #[test]
    fn references_du_xml() {
        let xml = r#"<?xml version="1.0"?><Invoice xmlns:cbc="urn:cbc" xmlns:cac="urn:cac"><cbc:ID>F-77</cbc:ID>
            <cbc:BuyerReference> Chantier PL46 &amp; Claes </cbc:BuyerReference>
            <cac:OrderReference><cbc:ID>BC-2026-0412</cbc:ID></cac:OrderReference>
            <cac:ProjectReference><cbc:ID/></cac:ProjectReference>
            <cac:InvoiceLine><cac:OrderLineReference><cbc:LineID>1</cbc:LineID></cac:OrderLineReference></cac:InvoiceLine></Invoice>"#;
        let r = references(xml);
        let lus: Vec<(&str, &str)> = r.iter().map(|r| (r.libelle, r.valeur.as_str())).collect();
        assert_eq!(lus, [("Référence de l'acheteur", "Chantier PL46 & Claes"), ("Bon de commande", "BC-2026-0412")]);
        // Sans préfixe d'espace de noms, et sans référence du tout.
        assert_eq!(references("<Invoice><BuyerReference>L63</BuyerReference></Invoice>")[0].valeur, "L63");
        assert!(references("<Invoice><ID>F-1</ID></Invoice>").is_empty());
    }

    #[test]
    fn codes_d_une_reference() {
        assert_eq!(codes("PL46 Salle de bain"), ["PL46"]);
        assert_eq!(codes("Chantier PL-46 / Claes"), ["PL46"]);
        assert_eq!(codes("PL46-Claes"), ["PL46CLAES", "PL46"]);
        assert!(codes("Rénovation 2026, 12 m²").is_empty(), "un nombre seul n'est pas un code");
        assert_eq!(cle("Maison  Dupont"), "MAISONDUPONT");
    }

    #[test]
    fn chantier_propose_d_office() {
        let refs = |texte: &str| vec![Reference { libelle: "Référence de l'acheteur", valeur: texte.into() }];
        let chantiers = vec![
            (1, "L63 Toiture".to_string(), true),
            (2, "PL46 Salle de bain".to_string(), true),
            (3, "Maison Dupont".to_string(), false),
            (4, "PL46 Ancien".to_string(), false),
        ];
        let r = refs("Chantier pl46 - Claes");
        assert_eq!(chantier_propose(&r, &chantiers).map(|(c, _, code)| (c, code)), Some((2, "PL46".into())), "le chantier en cours passe avant le terminé");
        let r = refs("MAISON DUPONT");
        assert_eq!(chantier_propose(&r, &chantiers).map(|(c, _, _)| c), Some(3), "la référence entière, même terminée");
        let r = refs("L63 et PL46");
        assert_eq!(chantier_propose(&r, &chantiers).map(|(c, _, _)| c), None, "deux chantiers en cours : rien n'est proposé");
        let r = refs("Commande 2026-118");
        assert_eq!(chantier_propose(&r, &chantiers).map(|(c, _, _)| c), None);
        assert!(chantier_propose(&[], &chantiers).is_none());
    }

    #[test]
    fn affichage() {
        assert_eq!(decimal(&json!("2.50")), "2,5");
        assert_eq!(decimal(&json!("10.00")), "10");
        assert_eq!(unite("MTK"), "m²");
        assert_eq!(tva_compacte("be 0123.456.749"), "BE0123456749");
    }
}
