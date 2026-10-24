//! Page client en ligne (étape 9-1) : le client ouvre le lien secret reçu par e-mail
//! (`/client/devis/{jeton}` ou `/client/facture/{jeton}`), sans compte ni mot de passe.
//!
//! - **Devis** : il le consulte (PDF), le **signe** (nom + case « bon pour accord ») ; le devis passe à « Signé »
//!   et l'entreprise l'accepte ensuite (comme dans Vertuoza).
//! - **Facture** : il la consulte, voit ce qui reste à payer et la **paie par QR code** (format EPC « SEPA », lu par
//!   les applications bancaires belges) avec la communication structurée.
//!
//! Le lien est un jeton de 144 bits impossible à deviner. Un brouillon n'est jamais visible.
//! Chaque page travaille dans l'organisation de la pièce du lien (`base_du_jeton`) : un lien ne montre jamais rien
//! d'une autre organisation, et le logo est celui de cette organisation (`/client/{devis|facture}/{jeton}/logo`). La page ne montre que ce
//! que le client reçoit déjà par e-mail (jamais les prix d'achat ni la marge).

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{Connection as _, PgConnection};

use crate::base::Base;
use crate::commun::{encoder_url, format_euros};
use crate::emails::{self, Message};
use crate::journal::{self, Genre};
use crate::securite::Ip;
use crate::signature::{self, Preuve, Verification};
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
    /// L'adresse du logo, propre au lien de la page.
    pub logo: String,
    /// L'entreprise a déposé des conditions générales (jointes au PDF du devis).
    pub conditions_generales: bool,
}

/// La pièce d'un lien secret.
#[derive(Clone, Copy)]
enum Lien {
    Devis,
    Facture,
}

impl Lien {
    fn chemin(self) -> &'static str {
        match self {
            Lien::Devis => "devis",
            Lien::Facture => "facture",
        }
    }
}

/// La connexion d'une page client : liée à l'organisation de la pièce du lien secret (lien inconnu : 404).
/// Le réglage `app.jeton` n'est posé que pour cette lecture (transaction), et la règle `lien_secret` de la base ne
/// laisse lire que la pièce de ce lien.
async fn base_du_jeton(state: &AppState, lien: Lien, jeton: &str) -> Result<Base, Erreur> {
    let organisation: Option<i64> = {
        let mut conn = state.db.acquire().await?;
        let mut tx = conn.begin().await?;
        sqlx::query("SELECT set_config('app.jeton', $1, true)").bind(jeton).execute(&mut *tx).await?;
        let requete = match lien {
            Lien::Devis => "SELECT organisation_id FROM devis WHERE jeton = $1",
            Lien::Facture => "SELECT organisation_id FROM factures WHERE jeton = $1",
        };
        let organisation = sqlx::query_scalar(requete).bind(jeton).fetch_optional(&mut *tx).await?;
        tx.rollback().await?;
        organisation
    };
    Ok(Base::ouvrir(&state.db, organisation.ok_or_else(Erreur::introuvable)?).await?)
}

async fn entreprise(db: &mut PgConnection, lien: Lien, jeton: &str) -> Result<Entreprise, sqlx::Error> {
    let ligne: Option<(String, String, String)> =
        sqlx::query_as("SELECT nom, email, telephone FROM societe WHERE organisation_id = app_organisation()").fetch_optional(&mut *db).await?;
    let a_un_logo: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = 'logo')").fetch_one(&mut *db).await?;
    let conditions_generales: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = 'conditions_generales')",
    )
    .fetch_one(&mut *db)
    .await?;
    let (nom, email, telephone) = ligne.unwrap_or_default();
    Ok(Entreprise { nom, email, telephone, a_un_logo, logo: format!("/client/{}/{jeton}/logo", lien.chemin()), conditions_generales })
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

/// Le logo de la société de la pièce, pour l'en-tête des pages client.
async fn logo(State(state): State<AppState>, Path((piece, jeton)): Path<(String, String)>) -> Result<Response, Erreur> {
    let lien = match piece.as_str() {
        "devis" => Lien::Devis,
        "facture" => Lien::Facture,
        _ => return Err(Erreur::introuvable()),
    };
    let mut db = base_du_jeton(&state, lien, &jeton).await?;
    let ligne: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT type_mime, contenu FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = 'logo'").fetch_optional(&mut *db).await?;
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
    /// L'adresse e-mail du client sur le devis (vide : aucune).
    email: String,
    ht: i64,
    ttc: i64,
    signe_le: Option<String>,
    signe_par: String,
}

/// Le devis d'un jeton, s'il est visible (pas un brouillon).
async fn devis_du_jeton(db: &mut PgConnection, jeton: &str) -> Result<DevisClient, Erreur> {
    let ligne: Option<(i64, String, i64, i32, Option<i64>, NaiveDate, String, String, i64, i64, Option<String>, String, String)> = sqlx::query_as(
        "SELECT d.id, d.statut::text, d.numero, extract(year FROM d.date_devis)::int, d.chantier_id, d.date_devis, d.reference,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), ''), d.montant_ht_cents, d.montant_ttc_cents,
                to_char(d.signe_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), d.signe_par,
                COALESCE(NULLIF(e.email, ''), NULLIF(c.email, ''), '')
         FROM devis d LEFT JOIN entreprises e ON e.id = d.client_entreprise_id LEFT JOIN contacts c ON c.id = d.client_contact_id
         WHERE d.jeton = $1",
    )
    .bind(jeton)
    .fetch_optional(&mut *db)
    .await?;
    let (id, statut, numero, annee, chantier, date, reference, client, ht, ttc, signe_le, signe_par, email) = ligne.ok_or_else(Erreur::introuvable)?;
    if statut == "brouillon" {
        return Err(Erreur::introuvable());
    }
    let avenant = chantier.is_some();
    let titre = format!("{} n° {}", if avenant { "Avenant" } else { "Devis" }, crate::devis_lignes::numero_affiche(annee, numero, avenant));
    Ok(DevisClient { id, statut, titre, date, reference, client, email, ht, ttc, signe_le, signe_par })
}

/// « Devis n° 2026-61 » → « le devis n° 2026-61 » ; « Avenant n° 2 » → « l'avenant n° 2 » (pour la phrase d'acceptation).
fn designation(titre: &str) -> String {
    if let Some(reste) = titre.strip_prefix("Avenant") {
        format!("l'avenant{reste}")
    } else if let Some(reste) = titre.strip_prefix("Devis") {
        format!("le devis{reste}")
    } else {
        titre.to_string()
    }
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
    /// « code » : un code reçu par e-mail est exigé ; « simple » : nom et case suffisent (mode test, pas d'adresse).
    mode: &'static str,
    code_envoye: bool,
    email_masque: String,
    nom_saisi: String,
    /// La phrase d'acceptation : celle qui est affichée est aussi celle qui est gardée dans le dossier de preuve.
    texte_accord: String,
    /// Un dossier de preuve existe : l'exemplaire signé peut être téléchargé.
    copie_signee: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
    code: String,
    nom: String,
}

async fn page_devis(State(state): State<AppState>, Path(jeton): Path<String>, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let mut db = base_du_jeton(&state, Lien::Devis, &jeton).await?;
    let d = devis_du_jeton(&mut *db, &jeton).await?;
    // La première consultation est notée dans l'historique du devis.
    // Chaque ouverture est notée (tableau de bord : « Ouvertures par vos clients »).
    sqlx::query("INSERT INTO consultations_client (devis_id) VALUES ($1)").bind(d.id).execute(&mut *db).await?;
    let premiere = sqlx::query("UPDATE devis SET consulte_le = now() WHERE id = $1 AND consulte_le IS NULL").bind(d.id).execute(&mut *db).await?;
    if premiere.rows_affected() == 1 {
        let mut tx = db.begin().await?;
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
    let entreprise = entreprise(&mut *db, Lien::Devis, &jeton).await?;
    let verification = Verification::pour(emails::envoi_actif(), &d.email);
    let texte_accord = signature::texte_accord(&designation(&d.titre), &date_belge(d.date), &format_euros(d.ttc), &entreprise.nom, entreprise.conditions_generales, verification);
    let copie_signee: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devis_signatures WHERE devis_id = $1)").bind(d.id).fetch_one(&mut *db).await?;
    let page = PageDevis {
        entreprise,
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
            "code" => "Le code est incorrect ou a expiré. Demandez-en un nouveau.",
            "envoi" => "Le code n'a pas pu être envoyé. Réessayez dans quelques minutes.",
            "trop" => "Trop de codes ont été demandés : patientez quelques minutes avant d'en demander un autre.",
            _ => "",
        },
        merci: r.ok == "signe",
        mode: if verification == Verification::CodeParEmail { "code" } else { "simple" },
        code_envoye: r.code == "envoye",
        email_masque: signature::masquer_email(&d.email),
        nom_saisi: r.nom.chars().filter(|c| !c.is_control()).take(LONGUEUR_MAX_NOM).collect(),
        texte_accord,
        copie_signee,
    };
    Ok(Html(page.render()?))
}

async fn pdf_devis(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    let mut db = base_du_jeton(&state, Lien::Devis, &jeton).await?;
    let d = devis_du_jeton(&mut *db, &jeton).await?;
    let (nom, octets) = crate::devis_pdf::pdf_du_devis(&mut *db, d.id).await?;
    pdf(nom, octets)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Signature {
    nom: String,
    accord: String,
    /// Le code à usage unique reçu par e-mail.
    code: String,
}

/// Le nom et la case cochée, ou le message d'erreur à afficher.
fn verifier_saisie(s: &Signature) -> Result<String, &'static str> {
    let nom = s.nom.trim();
    if nom.is_empty() || nom.chars().count() > LONGUEUR_MAX_NOM {
        return Err("nom");
    }
    if s.accord != "oui" {
        return Err("accord");
    }
    Ok(nom.to_string())
}

/// « Recevoir le code par e-mail » : envoie à l'adresse du client un code à usage unique (valable 10 minutes).
async fn envoyer_code(State(state): State<AppState>, Path(jeton): Path<String>, Form(s): Form<Signature>) -> Result<Redirect, Erreur> {
    let mut db = base_du_jeton(&state, Lien::Devis, &jeton).await?;
    let d = devis_du_jeton(&mut *db, &jeton).await?;
    let nom = match verifier_saisie(&s) {
        Ok(n) => n,
        Err(erreur) => return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur={erreur}#signer"))),
    };
    if !signable(&d.statut) || Verification::pour(emails::envoi_actif(), &d.email) != Verification::CodeParEmail {
        return Ok(Redirect::to(&format!("/client/devis/{jeton}")));
    }
    let Some(code) = signature::emettre_code(&mut *db, d.id, &d.email).await? else {
        return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur=trop&code=envoye&nom={}#signer", encoder_url(&nom))));
    };
    let entreprise = entreprise(&mut *db, Lien::Devis, &jeton).await?;
    let corps = format!(
        "Bonjour,\n\nVotre code pour signer {} : {code}\n\nIl est valable 10 minutes. Si vous n'êtes pas à l'origine de cette demande, ignorez simplement ce message : rien ne sera signé.\n\n{}",
        designation(&d.titre),
        entreprise.nom
    );
    let (id, issue) = emails::envoyer(
        &mut *db,
        Message {
            modele: "code_signature",
            devis_id: Some(d.id),
            facture_id: None,
            destinataires: vec![d.email.clone()],
            copie: Vec::new(),
            objet: format!("Votre code de signature : {}", d.titre),
            corps,
            pieces: Vec::new(),
        },
    )
    .await?;
    // Le code ne reste dans aucun journal : l'entreprise ne doit pas pouvoir le lire et signer à la place du client.
    sqlx::query("UPDATE emails SET corps = replace(corps, $1, '••••••') WHERE id = $2").bind(&code).bind(id).execute(&mut *db).await?;
    if issue == emails::Issue::Echec {
        return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur=envoi&nom={}#signer", encoder_url(&nom))));
    }
    let mut tx = db.begin().await?;
    journal::noter(&mut tx, d.id, Genre::Client, &format!("Code de signature envoyé à {}", signature::masquer_email(&d.email))).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/client/devis/{jeton}?code=envoye&nom={}#signer", encoder_url(&nom))))
}

async fn signer(
    State(state): State<AppState>,
    Path(jeton): Path<String>,
    ip: Ip,
    entetes: HeaderMap,
    Form(s): Form<Signature>,
) -> Result<Redirect, Erreur> {
    let mut db = base_du_jeton(&state, Lien::Devis, &jeton).await?;
    let d = devis_du_jeton(&mut *db, &jeton).await?;
    let nom = match verifier_saisie(&s) {
        Ok(n) => n,
        Err(erreur) => return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur={erreur}#signer"))),
    };
    let verification = Verification::pour(emails::envoi_actif(), &d.email);
    if verification == Verification::CodeParEmail && !signature::verifier_code(&mut *db, d.id, &d.email, &s.code).await? {
        return Ok(Redirect::to(&format!("/client/devis/{jeton}?erreur=code&code=envoye&nom={}#signer", encoder_url(&nom))));
    }
    // Le PDF que le client signe : sa copie et son empreinte entrent dans le dossier de preuve.
    let (pdf_nom, pdf) = crate::devis_pdf::pdf_du_devis(&mut *db, d.id).await?;
    let entreprise = entreprise(&mut *db, Lien::Devis, &jeton).await?;
    let preuve = Preuve {
        devis_id: d.id,
        nom: nom.clone(),
        email: d.email.clone(),
        verification,
        ip: ip.0,
        navigateur: signature::navigateur(&entetes),
        texte_accepte: signature::texte_accord(&designation(&d.titre), &date_belge(d.date), &format_euros(d.ttc), &entreprise.nom, entreprise.conditions_generales, verification),
        montant_ttc_cents: d.ttc,
        pdf_nom: pdf_nom.clone(),
        pdf: pdf.clone(),
    };
    let mut tx = db.begin().await?;
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1 FOR UPDATE").bind(d.id).fetch_one(&mut *tx).await?;
    if !signable(&statut) {
        // Déjà signé, accepté ou refusé : rien ne change.
        return Ok(Redirect::to(&format!("/client/devis/{jeton}")));
    }
    sqlx::query("UPDATE devis SET statut = 'signe', statut_date = now(), signe_le = now(), signe_par = $1, updated_at = now() WHERE id = $2")
        .bind(&nom)
        .bind(d.id)
        .execute(&mut *tx)
        .await?;
    let empreinte = signature::enregistrer(&mut tx, &preuve).await?;
    journal::noter(
        &mut tx,
        d.id,
        Genre::Client,
        &format!("Signé en ligne par {nom} (bon pour accord) · {} · empreinte du PDF {}…", Verification::libelle(verification.code()), &empreinte[..16]),
    )
    .await?;
    let detail = format!("{} → Signé (en ligne)", crate::devis::statut(&statut).0);
    journal::noter(&mut tx, d.id, Genre::Statut, &detail).await?;
    tx.commit().await?;

    // Un exemplaire signé est envoyé au client (enregistré seulement, sans clé d'envoi). Un échec n'annule pas la signature.
    if verification != Verification::SansEmail {
        let quand: String = sqlx::query_scalar("SELECT to_char(signe_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY à HH24:MI') FROM devis WHERE id = $1")
            .bind(d.id)
            .fetch_one(&mut *db)
            .await?;
        let corps = format!(
            "Bonjour {nom},\n\nVous avez signé {} le {quand}. Vous trouverez en pièce jointe l'exemplaire signé.\n\nEmpreinte numérique (SHA-256) du document : {empreinte}\n\n{}",
            designation(&d.titre),
            entreprise.nom
        );
        emails::envoyer(
            &mut *db,
            Message {
                modele: "devis_signe",
                devis_id: Some(d.id),
                facture_id: None,
                destinataires: vec![d.email.clone()],
                copie: Vec::new(),
                objet: format!("Votre exemplaire signé : {}", d.titre),
                corps,
                pieces: vec![(pdf_nom, pdf)],
            },
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/client/devis/{jeton}?ok=signe")))
}

/// L'exemplaire signé, tel que le client l'a signé.
async fn pdf_signe(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Response, Erreur> {
    let mut db = base_du_jeton(&state, Lien::Devis, &jeton).await?;
    let d = devis_du_jeton(&mut *db, &jeton).await?;
    let (nom, octets) = signature::pdf_signe(&mut *db, d.id).await?.ok_or_else(Erreur::introuvable)?;
    signature::reponse_pdf(&nom, octets, "inline")
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

async fn facture_du_jeton(db: &mut PgConnection, jeton: &str) -> Result<factures::Facture, Erreur> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM factures WHERE jeton = $1 AND numero IS NOT NULL").bind(jeton).fetch_optional(&mut *db).await?;
    let id = id.ok_or_else(Erreur::introuvable)?;
    factures::charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)
}

async fn page_facture(State(state): State<AppState>, Path(jeton): Path<String>) -> Result<Html<String>, Erreur> {
    let mut db = base_du_jeton(&state, Lien::Facture, &jeton).await?;
    let f = facture_du_jeton(&mut *db, &jeton).await?;
    sqlx::query("INSERT INTO consultations_client (facture_id) VALUES ($1)").bind(f.id).execute(&mut *db).await?;
    let premiere = sqlx::query("UPDATE factures SET consulte_le = now() WHERE id = $1 AND consulte_le IS NULL").bind(f.id).execute(&mut *db).await?;
    if premiere.rows_affected() == 1 {
        let mut tx = db.begin().await?;
        paiements::noter(&mut tx, f.id, "client", "Consultée en ligne par le client").await?;
        tx.commit().await?;
    }
    let (ht, ttc): (i64, i64) =
        sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM factures WHERE id = $1").bind(f.id).fetch_one(&mut *db).await?;
    let solde = paiements::solde(&mut *db, f.id).await?;
    let compte: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT iban, bic FROM societe_comptes_bancaires WHERE organisation_id = app_organisation() AND par_defaut").fetch_optional(&mut *db).await?;
    let (iban, bic) = compte.map(|(i, b)| (i, b.unwrap_or_default())).unwrap_or_default();
    let entreprise = entreprise(&mut *db, Lien::Facture, &jeton).await?;
    let note_credit = f.genre == "note_credit";
    let vcs = f.vcs.clone().unwrap_or_default();
    let reste = if note_credit { 0 } else { solde.reste_cents.max(0) };
    let qr = texte_epc(&entreprise.nom, &iban, &bic, reste, &vcs).and_then(|t| qr_svg(&t)).unwrap_or_default();
    let client = factures::nom_client(&mut *db, f.client_entreprise_id, f.client_contact_id).await?;
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
    let mut db = base_du_jeton(&state, Lien::Facture, &jeton).await?;
    let f = facture_du_jeton(&mut *db, &jeton).await?;
    let (nom, octets) = crate::archive::pdf(&mut *db, &f).await?;
    pdf(nom, octets)
}

/// L'adresse de la page client d'une pièce (le jeton est créé au premier besoin).
pub(crate) async fn lien(db: &mut PgConnection, piece: crate::emails::Piece, id: i64) -> Result<String, sqlx::Error> {
    let chemin = match piece {
        crate::emails::Piece::Devis => "devis",
        crate::emails::Piece::Facture => "facture",
    };
    Ok(format!("{}/client/{chemin}/{}", crate::emails::adresse_publique(), crate::emails::jeton(&mut *db, piece, id).await?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/client/{piece}/{jeton}/logo", get(logo))
        .route("/client/devis/{jeton}", get(page_devis).post(signer))
        .route("/client/devis/{jeton}/code", post(envoyer_code))
        .route("/client/devis/{jeton}/signe", get(pdf_signe))
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

    fn page_de_test(mode: &'static str, code_envoye: bool, erreur: &'static str) -> PageDevis {
        PageDevis {
            entreprise: Entreprise { nom: "Ubique SRL".into(), email: "info@ubique.be".into(), telephone: String::new(), a_un_logo: false, logo: String::new(), conditions_generales: false },
            jeton: "jeton".into(),
            titre: "Devis n° 2026-61".into(),
            date: "19/10/2026".into(),
            reference: String::new(),
            client: "Anne Claes".into(),
            ht: "1 000,00 €".into(),
            tva: "60,00 €".into(),
            ttc: "1 060,00 €".into(),
            etat: "signable",
            signe_le: String::new(),
            signe_par: String::new(),
            erreur,
            merci: false,
            mode,
            code_envoye,
            email_masque: "a***@claes.be".into(),
            nom_saisi: "Anne Claes".into(),
            texte_accord: "Texte d'acceptation de test.".into(),
            copie_signee: false,
        }
    }

    #[test]
    fn l_ecran_de_signature_avec_code_a_deux_etapes() {
        // Étape 1 : on demande le code, on ne peut pas encore signer.
        let html = page_de_test("code", false, "").render().unwrap();
        assert!(html.contains("Recevoir le code par e-mail") && html.contains("a***@claes.be") && html.contains("/client/devis/jeton/code"));
        assert!(!html.contains("name=\"code\"") && !html.contains(">Signer le devis</button>"), "pas de signature avant le code");
        // Étape 2 : le code est saisi, la signature devient possible, le nom reste rempli.
        let html = page_de_test("code", true, "").render().unwrap();
        assert!(html.contains("name=\"code\"") && html.contains(">Signer le devis</button>") && html.contains("Recevoir un nouveau code") && html.contains("value=\"Anne Claes\""));
        assert!(html.contains("Un code à 6 chiffres vient d'être envoyé à a***@claes.be") || html.contains("Un code à 6 chiffres vient d&#39;être envoyé à a***@claes.be"));
        // Un code refusé : le message d'erreur remplace la confirmation d'envoi.
        let html = page_de_test("code", true, "Le code est incorrect ou a expiré. Demandez-en un nouveau.").render().unwrap();
        assert!(html.contains("Le code est incorrect") && !html.contains("vient d&#39;être envoyé") && !html.contains("vient d'être envoyé"));
        // Mode simple (test ou pas d'adresse) : nom, case, bouton ; ni code ni envoi.
        let html = page_de_test("simple", false, "").render().unwrap();
        assert!(html.contains(">Signer le devis</button>") && !html.contains("name=\"code\"") && !html.contains("Recevoir le code"));
        assert!(html.contains("Texte d&#39;acceptation de test.") || html.contains("Texte d'acceptation de test."), "la phrase d'acceptation est affichée");
    }
}
