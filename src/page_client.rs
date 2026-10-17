//! Page client en ligne (étape 9-1) : le client ouvre le lien secret reçu par e-mail
//! (`/client/devis/{jeton}` ou `/client/facture/{jeton}`), sans compte ni mot de passe.
//!
//! - **Devis** : il le consulte (PDF), le **signe** (nom + case « bon pour accord ») ; le devis passe à « Signé »
//!   et l'entreprise l'accepte ensuite (comme dans Vertuoza).
//! - **Facture** : il la consulte, voit ce qui reste à payer et la **paie par QR code** (format EPC « SEPA », lu par
//!   les applications bancaires belges) avec la communication structurée.
//!
//! Le lien est un jeton de 144 bits impossible à deviner. Un brouillon n'est jamais visible. La page ne montre que ce
//! que le client reçoit déjà par e-mail (jamais les prix d'achat ni la marge).

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::{HeaderValue, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::PgPool;

use crate::commun::format_euros;
use crate::journal::{self, Genre};
use crate::{AppState, Erreur, factures, paiements};

const LONGUEUR_MAX_NOM: usize = 120;

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// Les statuts d'un devis que le client peut signer.
pub(crate) fn signable(statut: &str) -> bool {
    matches!(statut, "a_envoyer" | "envoi_en_cours" | "envoye" | "echec_envoi")
}

// ─── QR code de paiement (EPC069-12, « SEPA Credit Transfer ») ──────────────

/// Le texte du QR code : bénéficiaire, IBAN, montant et communication. `None` si rien à payer ou pas d'IBAN.
pub(crate) fn texte_epc(beneficiaire: &str, iban: &str, bic: &str, centimes: i64, communication: &str) -> Option<String> {
    let iban: String = iban.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    if iban.is_empty() || !(1..=99_999_999_999).contains(&centimes) {
        return None;
    }
    let nom: String = beneficiaire.chars().filter(|c| !c.is_control()).take(70).collect();
    let bic: String = bic.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let montant = format!("EUR{}.{:02}", centimes / 100, centimes % 100);
    // La communication structurée belge (+++123/4567/89012+++) est reconnue par les banques dans ce champ.
    let communication: String = communication.chars().filter(|c| !c.is_control()).take(140).collect();
    Some(["BCD", "002", "1", "SCT", &bic, &nom, &iban, &montant, "", "", &communication].join("\n"))
}

/// Le QR code en SVG (dessiné par le serveur : pas de JavaScript).
pub(crate) fn qr_svg(texte: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(texte.as_bytes(), qrcode::EcLevel::M).ok()?;
    let svg = code.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build();
    // Sans l'en-tête XML : le dessin est placé tel quel dans la page.
    svg.find("<svg").map(|debut| svg[debut..].to_string())
}

// ─── Ce que toutes les pages client partagent ───────────────────────────────

pub struct Entreprise {
    pub nom: String,
    pub email: String,
    pub telephone: String,
    pub a_un_logo: bool,
}

async fn entreprise(db: &PgPool) -> Result<Entreprise, sqlx::Error> {
    let ligne: Option<(String, String, String)> =
        sqlx::query_as("SELECT nom, email, telephone FROM societe WHERE id = 1").fetch_optional(db).await?;
    let a_un_logo: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM societe_fichiers WHERE usage = 'logo')").fetch_one(db).await?;
    let (nom, email, telephone) = ligne.unwrap_or_default();
    Ok(Entreprise { nom, email, telephone, a_un_logo })
}

fn pdf(nom: String, octets: Vec<u8>) -> Result<Response, Erreur> {
    let mut reponse = octets.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    entetes.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("inline; filename=\"{nom}\"")).map_err(|_| Erreur::pdf("nom de fichier"))?,
    );
    entetes.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(reponse)
}

/// Le logo de la société, pour l'en-tête des pages client.
async fn logo(State(state): State<AppState>) -> Result<Response, Erreur> {
    let ligne: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT type_mime, contenu FROM societe_fichiers WHERE usage = 'logo'").fetch_optional(&state.db).await?;
    let (mime, contenu) = ligne.ok_or_else(Erreur::introuvable)?;
    let mut reponse = contenu.into_response();
    reponse.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).map_err(|_| Erreur::introuvable())?);
    Ok(reponse)
}

// ─── Devis ──────────────────────────────────────────────────────────────────

struct DevisClient {
    id: i64,
    statut: String,
    titre: String,
    date: NaiveDate,
    reference: String,
    client: String,
    ht: i64,
    ttc: i64,
    signe_le: Option<String>,
    signe_par: String,
}

/// Le devis d'un jeton, s'il est visible (pas un brouillon).
async fn devis_du_jeton(db: &PgPool, jeton: &str) -> Result<DevisClient, Erreur> {
    let ligne: Option<(i64, String, i64, i32, Option<i64>, NaiveDate, String, String, i64, i64, Option<String>, String)> = sqlx::query_as(
        "SELECT d.id, d.statut::text, d.numero, extract(year FROM d.date_devis)::int, d.chantier_id, d.date_devis, d.reference,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), ''), d.montant_ht_cents, d.montant_ttc_cents,
                to_char(d.signe_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), d.signe_par
         FROM devis d LEFT JOIN entreprises e ON e.id = d.client_entreprise_id LEFT JOIN contacts c ON c.id = d.client_contact_id
         WHERE d.jeton = $1",
    )
    .bind(jeton)
    .fetch_optional(db)
    .await?;
    let (id, statut, numero, annee, chantier, date, reference, client, ht, ttc, signe_le, signe_par) = ligne.ok_or_else(Erreur::introuvable)?;
    if statut == "brouillon" {
        return Err(Erreur::introuvable());
    }
    let avenant = chantier.is_some();
    let titre = format!("{} n° {}", if avenant { "Avenant" } else { "Devis" }, crate::devis_lignes::numero_affiche(annee, numero, avenant));
    Ok(DevisClient { id, statut, titre, date, reference, client, ht, ttc, signe_le, signe_par })
}

#[derive(Template)]
#[template(path = "client_devis.html")]
struct PageDevis {
    entreprise: Entreprise,
    jeton: String,
    titre: String,
    date: String,
    reference: String,
    client: String,
    ht: String,
    tva: String,
    ttc: String,
    /// « signable », « signe », « accepte », « refuse ».
    etat: &'static str,
    signe_le: String,
    signe_par: String,
    erreur: &'static str,
    merci: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

async fn page_devis(State(state): State<AppState>, Path(jeton): Path<String>, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let d = devis_du_jeton(&state.db, &jeton).await?;
    // La première consultation est notée dans l'historique du devis.
    // Chaque ouverture est notée (tableau de bord : « Ouvertures par vos clients »).
    sqlx::query("INSERT INTO consultations_client (devis_id) VALUES ($1)").bind(d.id).execute(&state.db).await?;
    let premiere = sqlx::query("UPDATE devis SET consulte_le = now() WHERE id = $1 AND consulte_le IS NULL").bind(d.id).execute(&state.db).await?;
    if premiere.rows_affected() == 1 {
        let mut tx = state.db.begin().await?;
        journal::noter(&mut tx, d.id, Genre::Client, "Consulté en ligne par le client").await?;
        tx.commit().await?;
    }
    let etat = match d.statut.as_str() {
        "signe" => "signe",
        "accepte" => "accepte",
        "refuse" => "refuse",
        s if signable(s) => "signable",
        _ => "refuse",
    };
    let page = PageDevis {
        entreprise: entreprise(&state.db).await?,
        jeton,
        titre: d.titre,
        date: date_belge(d.date),
        reference: d.reference,
        client: d.client,
        ht: format_euros(d.ht),
        tva: format_euros(d.ttc - d.ht),
        ttc: format_euros(d.ttc),
        etat,
        signe_le: d.signe_le.unwrap_or_default(),
        signe_par: d.signe_par,
        erreur: match r.erreur.as_str() {
            "nom" => "Indiquez votre nom complet (120 caractères au maximum).",
            "accord" => "Cochez la case « Bon pour accord » pour signer.",
            _ => "",
        },
        merci: r.ok == "signe",
    };
    Ok(Html(page.render()?))
}

async fn pdf_devis(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    let d = devis_du_jeton(&state.db, &jeton).await?;
    let (nom, octets) = crate::devis_pdf::pdf_du_devis(&state.db, d.id).await?;
    pdf(nom, octets)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Signature {
    nom: String,
    accord: String,
}

async fn signer(State(state): State<AppState>, Path(jeton): Path<String>, Form(s): Form<Signature>) -> Result<Redirect, Erreur> {
    let d = devis_du_jeton(&state.db, &jeton).await?;
    let nom = s.nom.trim();
    if nom.is_empty() || nom.chars().count() > LONGUEUR_MAX_NOM {
        return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur=nom#signer")));
    }
    if s.accord != "oui" {
        return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur=accord#signer")));
    }
    let mut tx = state.db.begin().await?;
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1 FOR UPDATE").bind(d.id).fetch_one(&mut *tx).await?;
    if !signable(&statut) {
        // Déjà signé, accepté ou refusé : rien ne change.
        return Ok(Redirect::to(&format!("/client/devis/{jeton}")));
    }
    sqlx::query("UPDATE devis SET statut = 'signe', statut_date = now(), signe_le = now(), signe_par = $1, updated_at = now() WHERE id = $2")
        .bind(nom)
        .bind(d.id)
        .execute(&mut *tx)
        .await?;
    journal::noter(&mut tx, d.id, Genre::Client, &format!("Signé en ligne par {nom} (bon pour accord)")).await?;
    let detail = format!("{} → Signé (en ligne)", crate::devis::statut(&statut).0);
    journal::noter(&mut tx, d.id, Genre::Statut, &detail).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/client/devis/{jeton}?ok=signe")))
}

// ─── Facture ────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "client_facture.html")]
struct PageFacture {
    entreprise: Entreprise,
    jeton: String,
    titre: String,
    note_credit: bool,
    date: String,
    echeance: String,
    reference: String,
    client: String,
    ht: String,
    tva: String,
    ttc: String,
    paye: String,
    reste: String,
    a_payer: bool,
    en_retard: bool,
    payee: bool,
    iban: String,
    bic: String,
    vcs: String,
    qr: String,
}

async fn facture_du_jeton(db: &PgPool, jeton: &str) -> Result<factures::Facture, Erreur> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM factures WHERE jeton = $1 AND numero IS NOT NULL").bind(jeton).fetch_optional(db).await?;
    let id = id.ok_or_else(Erreur::introuvable)?;
    factures::charger(db, id).await?.ok_or_else(Erreur::introuvable)
}

async fn page_facture(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Html<String>, Erreur> {
    let f = facture_du_jeton(&state.db, &jeton).await?;
    sqlx::query("INSERT INTO consultations_client (facture_id) VALUES ($1)").bind(f.id).execute(&state.db).await?;
    let premiere = sqlx::query("UPDATE factures SET consulte_le = now() WHERE id = $1 AND consulte_le IS NULL").bind(f.id).execute(&state.db).await?;
    if premiere.rows_affected() == 1 {
        let mut tx = state.db.begin().await?;
        paiements::noter(&mut tx, f.id, "client", "Consultée en ligne par le client").await?;
        tx.commit().await?;
    }
    let (ht, ttc): (i64, i64) =
        sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM factures WHERE id = $1").bind(f.id).fetch_one(&state.db).await?;
    let solde = paiements::solde(&state.db, f.id).await?;
    let compte: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT iban, bic FROM societe_comptes_bancaires WHERE par_defaut").fetch_optional(&state.db).await?;
    let (iban, bic) = compte.map(|(i, b)| (i, b.unwrap_or_default())).unwrap_or_default();
    let entreprise = entreprise(&state.db).await?;
    let note_credit = f.genre == "note_credit";
    let vcs = f.vcs.clone().unwrap_or_default();
    let reste = if note_credit { 0 } else { solde.reste_cents.max(0) };
    let qr = texte_epc(&entreprise.nom, &iban, &bic, reste, &vcs).and_then(|t| qr_svg(&t)).unwrap_or_default();
    let client = factures::nom_client(&state.db, f.client_entreprise_id, f.client_contact_id).await?;
    let page = PageFacture {
        titre: factures::titre(&f.genre, f.numero, f.date_facture),
        note_credit,
        date: date_belge(f.date_facture),
        echeance: f.date_echeance.map(date_belge).unwrap_or_default(),
        reference: f.reference.clone(),
        client,
        ht: format_euros(ht),
        tva: format_euros(ttc - ht),
        ttc: format_euros(ttc),
        paye: format_euros(solde.paye_cents),
        reste: format_euros(reste),
        a_payer: reste > 0,
        en_retard: solde.etat == "en_retard",
        payee: solde.etat == "payee",
        iban,
        bic,
        vcs,
        qr,
        entreprise,
        jeton,
    };
    Ok(Html(page.render()?))
}

async fn pdf_facture(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    let f = facture_du_jeton(&state.db, &jeton).await?;
    let (nom, octets) = crate::facture_pdf::pdf_de_la_facture(&state.db, &f).await?;
    pdf(nom, octets)
}

/// L'adresse de la page client d'une pièce (le jeton est créé au premier besoin).
pub(crate) async fn lien(db: &PgPool, piece: crate::emails::Piece, id: i64) -> Result<String, sqlx::Error> {
    let chemin = match piece {
        crate::emails::Piece::Devis => "devis",
        crate::emails::Piece::Facture => "facture",
    };
    Ok(format!("{}/client/{chemin}/{}", crate::emails::adresse_publique(), crate::emails::jeton(db, piece, id).await?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/client/logo", get(logo))
        .route("/client/devis/{jeton}", get(page_devis).post(signer))
        .route("/client/devis/{jeton}/pdf", get(pdf_devis))
        .route("/client/facture/{jeton}", get(page_facture))
        .route("/client/facture/{jeton}/pdf", get(pdf_facture))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_texte_du_qr_code_epc() {
        let t = texte_epc("Ubique SRL", "BE68 5390 0754 7034", "GKCCBEBB", 385_254, "+++000/0000/12528+++").unwrap();
        assert_eq!(t, "BCD\n002\n1\nSCT\nGKCCBEBB\nUbique SRL\nBE68539007547034\nEUR3852.54\n\n\n+++000/0000/12528+++");
        assert_eq!(texte_epc("Ubique", "BE68539007547034", "", 5, "").unwrap().lines().nth(7), Some("EUR0.05"));
        // Rien à payer ou pas d'IBAN : pas de QR code.
        assert_eq!(texte_epc("Ubique", "BE68539007547034", "", 0, ""), None);
        assert_eq!(texte_epc("Ubique", "", "", 100, ""), None);
    }

    #[test]
    fn le_qr_code_est_un_svg() {
        let svg = qr_svg("BCD\n002\n1\nSCT\n\nUbique\nBE68539007547034\nEUR10.00\n\n\n").unwrap();
        assert!(svg.contains("<svg") && svg.contains("</svg>"));
    }

    #[test]
    fn statuts_signables() {
        assert!(signable("envoye") && signable("a_envoyer") && signable("echec_envoi"));
        assert!(!signable("brouillon") && !signable("signe") && !signable("accepte") && !signable("refuse"));
    }
}
