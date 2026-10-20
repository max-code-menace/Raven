//! Export des factures pour le comptable (étape 8-4), comme « Export factures » de Vertuoza.
//!
//! Un dossier d'export couvre une période (date des pièces) : factures et notes de crédit **validées**, au format PDF,
//! UBL 2.1 ou les deux (choix de Mahx : les deux), toutes ou seulement celles qui n'ont encore jamais été exportées.
//! Le dossier est un ZIP (`PDF/`, `UBL/`, `recapitulatif.csv`) gardé en base : on le télécharge ou on l'envoie par
//! e-mail au comptable (modèle « Export pour le comptable »).

use std::io::Write as _;

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query},
    http::{HeaderValue, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use sqlx::{FromRow, PgConnection};

use crate::base::Base;
use crate::commun::{Choix, email_valide};
use crate::emails::{self, Issue, Message};
use crate::{AppState, Erreur, factures};

/// Un e-mail ne transporte pas des fichiers trop gros : au-delà, on télécharge le dossier.
const TAILLE_MAX_EMAIL: usize = 10 * 1024 * 1024;

pub(crate) const FORMATS: [(&str, &str); 3] = [("les_deux", "PDF et UBL 2.1"), ("pdf", "PDF"), ("ubl", "UBL 2.1")];
pub(crate) const CONTENUS: [(&str, &str); 2] = [("toutes", "Toutes les pièces"), ("non_exportees", "Uniquement les non exportées")];

fn libelle(liste: &[(&str, &str)], code: &str) -> String {
    liste.iter().find(|(c, _)| *c == code).map(|(_, l)| (*l).to_string()).unwrap_or_default()
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// Le mois précédent : la période proposée par défaut.
pub(crate) fn mois_precedent(aujourdhui: NaiveDate) -> (NaiveDate, NaiveDate) {
    let premier_du_mois = aujourdhui.with_day(1).expect("le 1er existe toujours");
    let fin = premier_du_mois.pred_opt().expect("une date valable");
    (fin.with_day(1).expect("le 1er existe toujours"), fin)
}

/// « 01/09/2026 au 30/09/2026 ».
fn periode(debut: NaiveDate, fin: NaiveDate) -> String {
    format!("{} au {}", date_belge(debut), date_belge(fin))
}

// ─── Le dossier ZIP ─────────────────────────────────────────────────────────

#[derive(FromRow)]
struct Piece {
    id: i64,
    genre: String,
    numero: i64,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    client: String,
    tva_client: String,
    montant_ht_cents: i64,
    montant_tva_cents: i64,
    montant_ttc_cents: i64,
    paye_cents: i64,
    reste_cents: i64,
    vcs: Option<String>,
    reference: String,
}

async fn pieces(db: &mut PgConnection, debut: NaiveDate, fin: NaiveDate, seulement_nouvelles: bool) -> Result<Vec<Piece>, sqlx::Error> {
    sqlx::query_as(
        "SELECT f.id, f.genre::text AS genre, f.numero, f.date_facture, f.date_echeance,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '') AS client,
                COALESCE(e.numero_tva, c.numero_tva, '') AS tva_client,
                f.montant_ht_cents, f.montant_tva_cents, f.montant_ttc_cents, et.paye_cents, GREATEST(et.reste_cents, 0) AS reste_cents,
                f.vcs, f.reference
         FROM factures f
         JOIN factures_etats et ON et.facture_id = f.id
         LEFT JOIN entreprises e ON e.id = f.client_entreprise_id
         LEFT JOIN contacts c ON c.id = f.client_contact_id
         WHERE f.numero IS NOT NULL AND NOT f.reprise AND f.date_facture BETWEEN $1 AND $2
           AND (NOT $3 OR NOT EXISTS (SELECT 1 FROM export_pieces p WHERE p.facture_id = f.id))
         ORDER BY f.genre = 'note_credit', f.date_facture, f.numero",
    )
    .bind(debut)
    .bind(fin)
    .bind(seulement_nouvelles)
    .fetch_all(&mut *db)
    .await
}

/// 102098 → « 1020,98 » (virgule décimale, comme Excel en Belgique).
fn nombre_csv(centimes: i64) -> String {
    let signe = if centimes < 0 { "-" } else { "" };
    let a = centimes.unsigned_abs();
    format!("{signe}{},{:02}", a / 100, a % 100)
}

fn champ_csv(texte: &str) -> String {
    if texte.contains([';', '"', '\n', '\r']) { format!("\"{}\"", texte.replace('"', "\"\"")) } else { texte.to_string() }
}

/// Le récapitulatif lisible par Excel : point-virgule, virgule décimale, BOM UTF-8. Les notes de crédit en négatif.
fn recapitulatif(pieces: &[Piece]) -> String {
    let mut csv = String::from("\u{feff}Type;Numéro;Date;Échéance;Client;TVA client;Référence;Montant HT;TVA;Montant TTC;Payé;Reste à recevoir;Communication\n");
    for p in pieces {
        let signe = if p.genre == "note_credit" { -1 } else { 1 };
        let colonnes = [
            factures::libelle_genre(&p.genre).to_string(),
            p.numero.to_string(),
            date_belge(p.date_facture),
            p.date_echeance.map(date_belge).unwrap_or_default(),
            p.client.clone(),
            p.tva_client.clone(),
            p.reference.clone(),
            nombre_csv(signe * p.montant_ht_cents),
            nombre_csv(signe * p.montant_tva_cents),
            nombre_csv(signe * p.montant_ttc_cents),
            nombre_csv(p.paye_cents),
            nombre_csv(p.reste_cents),
            p.vcs.clone().unwrap_or_default(),
        ];
        csv.push_str(&colonnes.iter().map(|c| champ_csv(c)).collect::<Vec<_>>().join(";"));
        csv.push('\n');
    }
    csv
}

/// « Facture-125 » ; « Facture-2026-125 » quand le dossier couvre plusieurs années (la numérotation repart à 1
/// chaque année : deux pièces de deux années peuvent avoir le même numéro).
fn nom_piece(genre: &str, numero: i64, annee: Option<i32>) -> String {
    let numero = match annee {
        Some(a) => format!("{a}-{numero}"),
        None => numero.to_string(),
    };
    if genre == "note_credit" { format!("Note-de-credit-{numero}") } else { format!("Facture-{numero}") }
}

/// Construit le ZIP du dossier d'export.
async fn construire_zip(db: &mut PgConnection, pieces: &[Piece], format: &str) -> Result<Vec<u8>, Erreur> {
    let mut fichiers: Vec<(String, Vec<u8>)> = vec![("recapitulatif.csv".to_string(), recapitulatif(pieces).into_bytes())];
    let plusieurs_annees = pieces.iter().any(|p| p.date_facture.year() != pieces[0].date_facture.year());
    for p in pieces {
        let nom = nom_piece(&p.genre, p.numero, plusieurs_annees.then(|| p.date_facture.year()));
        if format != "ubl" {
            let f = factures::charger(&mut *db, p.id).await?.ok_or_else(Erreur::introuvable)?;
            let (_, pdf) = crate::archive::pdf(&mut *db, &f).await?;
            fichiers.push((format!("PDF/{nom}.pdf"), pdf));
        }
        if format != "pdf" {
            let f = factures::charger(&mut *db, p.id).await?.ok_or_else(Erreur::introuvable)?;
            let Some(xml) = crate::archive::ubl(&mut *db, &f).await? else { continue };
            fichiers.push((format!("UBL/{nom}.xml"), xml.into_bytes()));
        }
    }
    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (nom, contenu) in fichiers {
            zip.start_file(nom, options).map_err(|e| e.to_string())?;
            zip.write_all(&contenu).map_err(|e| e.to_string())?;
        }
        Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
    })
    .await
    .map_err(|e| Erreur::pdf(&e.to_string()))?
    .map_err(|e| Erreur::pdf(&e))
}

// ─── L'écran ────────────────────────────────────────────────────────────────

pub struct LigneExport {
    pub id: i64,
    pub date: String,
    pub periode: String,
    pub format: String,
    pub contenu: String,
    pub nombre: i32,
    pub taille: String,
    pub envoye: String,
}

#[derive(Template)]
#[template(path = "exports.html")]
struct Page {
    exports: Vec<LigneExport>,
    debut: String,
    fin: String,
    formats: Vec<Choix>,
    contenus: Vec<Choix>,
    email_comptable: String,
    envoi_actif: bool,
    message_ok: &'static str,
    message_erreur: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

impl Retour {
    fn message_ok(&self) -> &'static str {
        match self.ok.as_str() {
            "cree" => "Le dossier d'export est prêt : télécharge-le ou envoie-le au comptable.",
            "comptable" => "L'adresse du comptable est enregistrée.",
            "envoye" => "Le dossier est envoyé au comptable.",
            "enregistre" => "Mode test : l'e-mail au comptable est enregistré (Réglages › E-mails) mais n'est pas parti, car aucune clé Brevo n'est configurée.",
            "supprime" => "Le dossier d'export est supprimé (les factures, elles, ne changent pas).",
            _ => "",
        }
    }

    fn message_erreur(&self) -> &'static str {
        match self.erreur.as_str() {
            "periode" => "La période n'est pas valable : deux dates, la seconde après la première.",
            "choix" => "Choisissez un format et un contenu.",
            "vide" => "Aucune pièce validée dans cette période (ou toutes déjà exportées) : rien à exporter.",
            "comptable" => "L'adresse du comptable n'est pas valable.",
            "sans_comptable" => "Indiquez d'abord l'adresse e-mail du comptable.",
            "trop_gros" => "Le dossier dépasse 10 Mo : trop gros pour un e-mail. Téléchargez-le et envoyez-le autrement, ou faites des périodes plus courtes.",
            "echec" => "L'e-mail au comptable n'a pas pu partir : le détail est dans Réglages › E-mails.",
            _ => "",
        }
    }
}

fn taille(octets: i64) -> String {
    if octets >= 1024 * 1024 {
        format!("{:.1} Mo", octets as f64 / (1024.0 * 1024.0)).replace('.', ",")
    } else {
        format!("{} ko", (octets + 1023) / 1024)
    }
}

async fn afficher(mut db: Base, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let lignes: Vec<(i64, String, NaiveDate, NaiveDate, String, String, i32, i64, Option<String>, String)> = sqlx::query_as(
        "SELECT id, to_char(created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), date_debut, date_fin, format, contenu,
                nombre_pieces, octet_length(fichier)::bigint, to_char(envoye_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY'), envoye_a
         FROM exports ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(&mut *db)
    .await?;
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await?;
    let (debut, fin) = mois_precedent(aujourdhui);
    let email_comptable: String = sqlx::query_scalar("SELECT email_comptable FROM reglages WHERE organisation_id = app_organisation()").fetch_one(&mut *db).await?;
    let page = Page {
        exports: lignes
            .into_iter()
            .map(|(id, date, debut, fin, format, contenu, nombre, octets, envoye_le, envoye_a)| LigneExport {
                id,
                date,
                periode: periode(debut, fin),
                format: libelle(&FORMATS, &format),
                contenu: libelle(&CONTENUS, &contenu),
                nombre,
                taille: taille(octets),
                envoye: envoye_le.map(|d| format!("le {d} à {envoye_a}")).unwrap_or_default(),
            })
            .collect(),
        debut: debut.to_string(),
        fin: fin.to_string(),
        formats: FORMATS.iter().enumerate().map(|(i, (v, l))| Choix { valeur: (*v).into(), libelle: (*l).into(), choisi: i == 0 }).collect(),
        contenus: CONTENUS.iter().enumerate().map(|(i, (v, l))| Choix { valeur: (*v).into(), libelle: (*l).into(), choisi: i == 1 }).collect(),
        email_comptable,
        envoi_actif: emails::envoi_actif(),
        message_ok: r.message_ok(),
        message_erreur: r.message_erreur(),
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Demande {
    debut: String,
    fin: String,
    format: String,
    contenu: String,
}

async fn creer(mut db: Base, Form(d): Form<Demande>) -> Result<Redirect, Erreur> {
    let lire = |t: &str| NaiveDate::parse_from_str(t.trim(), "%Y-%m-%d").ok();
    let (Some(debut), Some(fin)) = (lire(&d.debut), lire(&d.fin)) else { return Ok(Redirect::to("/exports?erreur=periode")) };
    if fin < debut {
        return Ok(Redirect::to("/exports?erreur=periode"));
    }
    let (Some(format), Some(contenu)) = (
        FORMATS.iter().map(|(c, _)| *c).find(|c| *c == d.format),
        CONTENUS.iter().map(|(c, _)| *c).find(|c| *c == d.contenu),
    ) else {
        return Ok(Redirect::to("/exports?erreur=choix"));
    };
    let pieces = pieces(&mut *db, debut, fin, contenu == "non_exportees").await?;
    if pieces.is_empty() {
        return Ok(Redirect::to("/exports?erreur=vide"));
    }
    let fichier = construire_zip(&mut *db, &pieces, format).await?;
    let nom = format!("Export-factures-{debut}-au-{fin}.zip");
    let mut tx = db.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO exports (date_debut, date_fin, format, contenu, nombre_pieces, nom_fichier, fichier)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(debut)
    .bind(fin)
    .bind(format)
    .bind(contenu)
    .bind(pieces.len() as i32)
    .bind(&nom)
    .bind(&fichier)
    .fetch_one(&mut *tx)
    .await?;
    for p in &pieces {
        sqlx::query("INSERT INTO export_pieces (export_id, facture_id) VALUES ($1, $2)").bind(id).bind(p.id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Redirect::to("/exports?ok=cree"))
}

async fn telecharger(mut db: Base, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let ligne: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT nom_fichier, fichier FROM exports WHERE id = $1").bind(id).fetch_optional(&mut *db).await?;
    let (nom, fichier) = ligne.ok_or_else(Erreur::introuvable)?;
    let mut reponse = fichier.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    // Le nom ne contient que des lettres sans accent, des chiffres et des tirets.
    entetes.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{nom}\"")).map_err(|_| Erreur::pdf("nom de fichier"))?,
    );
    Ok(reponse)
}

async fn supprimer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let fait = sqlx::query("DELETE FROM exports WHERE id = $1").bind(id).execute(&mut *db).await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to("/exports?ok=supprime"))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Comptable {
    email: String,
}

async fn enregistrer_comptable(mut db: Base, Form(c): Form<Comptable>) -> Result<Redirect, Erreur> {
    let email = c.email.trim();
    if !email.is_empty() && !email_valide(email) {
        return Ok(Redirect::to("/exports?erreur=comptable"));
    }
    sqlx::query("UPDATE reglages SET email_comptable = $1, updated_at = now() WHERE organisation_id = app_organisation()").bind(email).execute(&mut *db).await?;
    Ok(Redirect::to("/exports?ok=comptable"))
}

/// Envoie le dossier au comptable avec le modèle « Export pour le comptable ».
async fn envoyer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let ligne: Option<(NaiveDate, NaiveDate, String, Vec<u8>)> =
        sqlx::query_as("SELECT date_debut, date_fin, nom_fichier, fichier FROM exports WHERE id = $1").bind(id).fetch_optional(&mut *db).await?;
    let (debut, fin, nom, fichier) = ligne.ok_or_else(Erreur::introuvable)?;
    let email: String = sqlx::query_scalar("SELECT email_comptable FROM reglages WHERE organisation_id = app_organisation()").fetch_one(&mut *db).await?;
    if email.is_empty() {
        return Ok(Redirect::to("/exports?erreur=sans_comptable"));
    }
    if fichier.len() > TAILLE_MAX_EMAIL {
        return Ok(Redirect::to("/exports?erreur=trop_gros"));
    }
    let (nom_societe, email_societe, _) = emails::societe(&mut *db).await?;
    let variables = vec![
        ("[NOM_SOCIETE]", nom_societe),
        ("[MAIL_GESTIONNAIRE]", email_societe),
        ("[PERIODE]", periode(debut, fin)),
    ];
    let (objet, corps) = emails::modele(&mut *db, "export").await?;
    let (email_id, issue) = emails::envoyer(
        &mut *db,
        Message {
            modele: "export",
            devis_id: None,
            facture_id: None,
            destinataires: vec![email.clone()],
            copie: Vec::new(),
            objet: emails::remplir(&objet, &variables),
            corps: emails::remplir(&corps, &variables),
            pieces: vec![(nom, fichier)],
        },
    )
    .await?;
    sqlx::query("UPDATE emails SET export_id = $1 WHERE id = $2").bind(id).bind(email_id).execute(&mut *db).await?;
    if issue == Issue::Envoye {
        sqlx::query("UPDATE exports SET envoye_le = now(), envoye_a = $1 WHERE id = $2").bind(&email).bind(id).execute(&mut *db).await?;
    }
    Ok(Redirect::to(match issue {
        Issue::Envoye => "/exports?ok=envoye",
        Issue::Enregistre => "/exports?ok=enregistre",
        Issue::Echec => "/exports?erreur=echec",
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/exports", get(afficher).post(creer))
        .route("/exports/comptable", post(enregistrer_comptable))
        .route("/exports/{id}/telecharger", get(telecharger))
        .route("/exports/{id}/envoyer", post(envoyer))
        .route("/exports/{id}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    #[test]
    fn le_mois_precedent() {
        assert_eq!(mois_precedent(jour(2026, 10, 16)), (jour(2026, 9, 1), jour(2026, 9, 30)));
        assert_eq!(mois_precedent(jour(2026, 1, 3)), (jour(2025, 12, 1), jour(2025, 12, 31)));
        assert_eq!(mois_precedent(jour(2028, 3, 1)), (jour(2028, 2, 1), jour(2028, 2, 29)));
    }

    #[test]
    fn le_recapitulatif_pour_excel() {
        let piece = |genre: &str, numero, ht, ttc| Piece {
            id: 1,
            genre: genre.into(),
            numero,
            date_facture: jour(2026, 9, 15),
            date_echeance: None,
            client: "Client; \"SA\"".into(),
            tva_client: String::new(),
            montant_ht_cents: ht,
            montant_tva_cents: ttc - ht,
            montant_ttc_cents: ttc,
            paye_cents: 0,
            reste_cents: 0,
            vcs: None,
            reference: String::new(),
        };
        let csv = recapitulatif(&[piece("simple", 125, 84_998, 102_098), piece("note_credit", 6, 84_998, 102_098)]);
        let lignes: Vec<&str> = csv.lines().collect();
        assert!(lignes[0].starts_with("\u{feff}Type;Numéro;Date"));
        assert_eq!(lignes[1], "Facture simple;125;15/09/2026;;\"Client; \"\"SA\"\"\";;;849,98;171,00;1020,98;0,00;0,00;");
        assert!(lignes[2].starts_with("Note de crédit;6;") && lignes[2].contains(";-849,98;-171,00;-1020,98;"));
    }

    #[test]
    fn tailles_lisibles() {
        assert_eq!(taille(500), "1 ko");
        assert_eq!(taille(2048), "2 ko");
        assert_eq!(taille(3 * 1024 * 1024 + 200 * 1024), "3,2 Mo");
    }
}
