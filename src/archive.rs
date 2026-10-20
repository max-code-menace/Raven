//! Archive légale des factures et des notes de crédit (conservation de 10 ans, art. 60 du Code TVA).
//!
//! À la validation, le PDF et l'UBL de la pièce sont fabriqués une fois pour toutes et rangés dans `facture_documents`
//! avec leur empreinte SHA-256. Ensuite, tout ce qui sort de l'application (téléchargement, e-mail, page client, export
//! pour le comptable) sert **cette** version, jamais un document regénéré : le modèle du PDF, la société ou le client
//! peuvent changer, la facture envoyée reste celle qui a été émise. La base refuse de modifier ou de supprimer un document
//! archivé (déclencheur de la migration 0032).
//!
//! Un brouillon n'a pas d'archive (son PDF est fabriqué à la demande, avec son filigrane). Une facture reprise de Vertuoza
//! non plus : son PDF d'origine est dans Vertuoza.

use axum::{
    Router,
    extract::Path,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

use crate::factures::{self, Facture};
use crate::base::Base;
use crate::{AppState, Erreur};

/// L'empreinte SHA-256 d'un contenu, en hexadécimal.
pub fn empreinte(octets: &[u8]) -> String {
    hex::encode(Sha256::digest(octets))
}

/// Une pièce validée émise ici : elle a une archive.
pub(crate) fn archivable(f: &Facture) -> bool {
    f.numero.is_some() && !f.reprise
}

#[derive(sqlx::FromRow)]
struct Rangee {
    nom: String,
    contenu: Vec<u8>,
}

async fn lire(db: &mut PgConnection, facture_id: i64, genre: &str) -> Result<Option<Rangee>, sqlx::Error> {
    sqlx::query_as("SELECT nom, contenu FROM facture_documents WHERE facture_id = $1 AND genre = $2").bind(facture_id).bind(genre).fetch_optional(&mut *db).await
}

/// Range le document s'il n'y est pas encore. Deux fabrications en même temps : la première gagne, l'autre est jetée.
async fn ranger(db: &mut PgConnection, facture_id: i64, genre: &str, nom: &str, contenu: &[u8]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO facture_documents (facture_id, genre, nom, contenu, empreinte) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (facture_id, genre) DO NOTHING",
    )
    .bind(facture_id)
    .bind(genre)
    .bind(nom)
    .bind(contenu)
    .bind(empreinte(contenu))
    .execute(&mut *db)
    .await?;
    Ok(())
}

/// S'assure que le PDF et l'UBL de la pièce sont archivés (sans effet s'ils le sont déjà ou si la pièce n'est pas archivable).
pub(crate) async fn garantir(db: &mut PgConnection, f: &Facture) -> Result<(), Erreur> {
    if !archivable(f) {
        return Ok(());
    }
    if lire(&mut *db, f.id, "pdf").await?.is_none() {
        let (nom, octets) = crate::facture_pdf::pdf_de_la_facture(&mut *db, f).await?;
        ranger(&mut *db, f.id, "pdf", &nom, &octets).await?;
    }
    if lire(&mut *db, f.id, "ubl").await?.is_none()
        && let Some(xml) = crate::ubl::de_la_facture(&mut *db, f.id).await?
    {
        let nom = nom_de_fichier(f, "xml");
        ranger(&mut *db, f.id, "ubl", &nom, xml.as_bytes()).await?;
    }
    Ok(())
}

fn nom_de_fichier(f: &Facture, extension: &str) -> String {
    match (f.genre.as_str(), f.numero) {
        ("note_credit", Some(n)) => format!("Note-de-credit-{n}.{extension}"),
        (_, Some(n)) => format!("Facture-{n}.{extension}"),
        (_, None) => format!("Facture-brouillon-{}.{extension}", f.id),
    }
}

/// Le PDF à servir (nom de fichier, octets) : l'archive d'une pièce validée, sinon un PDF fabriqué à la demande.
pub(crate) async fn pdf(db: &mut PgConnection, f: &Facture) -> Result<(String, Vec<u8>), Erreur> {
    if archivable(f) {
        garantir(&mut *db, f).await?;
        if let Some(r) = lire(&mut *db, f.id, "pdf").await? {
            return Ok((r.nom, r.contenu));
        }
    }
    crate::facture_pdf::pdf_de_la_facture(&mut *db, f).await
}

/// L'UBL à servir : l'archive d'une pièce validée, sinon fabriqué à la demande (`None` pour un brouillon).
pub(crate) async fn ubl(db: &mut PgConnection, f: &Facture) -> Result<Option<String>, Erreur> {
    if archivable(f) {
        garantir(&mut *db, f).await?;
        if let Some(r) = lire(&mut *db, f.id, "ubl").await? {
            return Ok(Some(String::from_utf8_lossy(&r.contenu).to_string()));
        }
    }
    Ok(crate::ubl::de_la_facture(&mut *db, f.id).await?)
}

/// Une ligne du bloc « Archive légale » de la fiche.
pub struct Ligne {
    pub genre: String,
    pub libelle: &'static str,
    pub nom: String,
    pub taille: String,
    pub empreinte: String,
    pub quand: String,
    pub adresse: String,
}

fn taille_lisible(octets: i64) -> String {
    if octets >= 1_000_000 { format!("{:.1} Mo", octets as f64 / 1_000_000.0).replace('.', ",") } else { format!("{} Ko", (octets + 999) / 1000) }
}

/// Les documents archivés d'une pièce, pour la fiche (les archive d'abord si la pièce validée n'en a pas encore).
pub(crate) async fn lignes(db: &mut PgConnection, f: &Facture) -> Result<Vec<Ligne>, Erreur> {
    if archivable(f) && garantir(&mut *db, f).await.is_err() {
        eprintln!("Archive de la facture {} : impossible pour l'instant.", f.id);
    }
    let rangees: Vec<(String, String, i64, String, String)> = sqlx::query_as(
        "SELECT genre, nom, length(contenu)::bigint, empreinte, to_char(cree_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI')
         FROM facture_documents WHERE facture_id = $1 ORDER BY genre DESC",
    )
    .bind(f.id)
    .fetch_all(&mut *db)
    .await?;
    Ok(rangees
        .into_iter()
        .map(|(genre, nom, taille, empreinte, quand)| Ligne {
            libelle: if genre == "pdf" { "PDF" } else { "UBL (Peppol BIS 3.0)" },
            adresse: format!("/factures/{}/archive/{genre}", f.id),
            genre,
            nom,
            taille: taille_lisible(taille),
            empreinte,
            quand,
        })
        .collect())
}

/// Vérifie que chaque document archivé a toujours son empreinte : (genre, intact).
pub async fn verifier(db: &mut PgConnection, facture_id: i64) -> Result<Vec<(String, bool)>, sqlx::Error> {
    let rangees: Vec<(String, Vec<u8>, String)> =
        sqlx::query_as("SELECT genre, contenu, empreinte FROM facture_documents WHERE facture_id = $1 ORDER BY genre").bind(facture_id).fetch_all(&mut *db).await?;
    Ok(rangees.into_iter().map(|(genre, contenu, attendue)| (genre, empreinte(&contenu) == attendue)).collect())
}

/// Télécharge un document archivé : `/factures/{id}/archive/pdf` ou `/archive/ubl`.
async fn telecharger(mut db: Base, Path((id, genre)): Path<(i64, String)>) -> Result<Response, Erreur> {
    let f = factures::charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    if genre != "pdf" && genre != "ubl" {
        return Err(Erreur::introuvable());
    }
    garantir(&mut *db, &f).await?;
    let r = lire(&mut *db, id, &genre).await?.ok_or_else(Erreur::introuvable)?;
    let type_mime = if genre == "pdf" { "application/pdf" } else { "application/xml" };
    let mut reponse = r.contenu.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static(type_mime));
    // Le nom ne contient que des lettres sans accent, des chiffres et des tirets : il est sûr dans un en-tête.
    entetes.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("attachment; filename=\"{}\"", r.nom)).map_err(|_| Erreur::pdf("nom de fichier"))?);
    entetes.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(reponse)
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/factures/{id}/archive/{genre}", get(telecharger))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_empreinte_est_un_sha256_en_hexadecimal() {
        assert_eq!(empreinte(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(empreinte(b"").len(), 64);
    }

    #[test]
    fn les_tailles_se_lisent() {
        assert_eq!(taille_lisible(1), "1 Ko");
        assert_eq!(taille_lisible(52_300), "53 Ko");
        assert_eq!(taille_lisible(1_260_000), "1,3 Mo");
    }
}
