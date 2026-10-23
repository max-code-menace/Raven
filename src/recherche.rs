//! Recherche partout (audit du design du 23/10/2026) : le champ en haut du menu cherche à la fois dans les devis, les
//! factures, les chantiers (ou projets) et les contacts, par numéro (« 2026-59 », « 59 »), référence ou code de chantier
//! (« PL46 »), nom du client, e-mail ou numéro de TVA. Dix résultats au plus par groupe ; chaque groupe mène à sa liste
//! filtrée pour voir la suite. Tout passe par la connexion de l'organisation (`Base`) : la sécurité au niveau des lignes
//! limite les résultats à l'organisation de la session.

use askama::Template;
use axum::{Router, extract::Query, response::Html, routing::get};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::PgConnection;

use crate::{
    AppState, Erreur,
    base::Base,
    commun::{echapper_like, encoder_url, format_euros},
};

/// Le nombre de résultats montrés par groupe.
const PAR_GROUPE: i64 = 10;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Saisie {
    q: String,
}

pub struct Resultat {
    pub lien: String,
    pub titre: String,
    pub detail: String,
    pub montant: String,
}

pub struct Groupe {
    pub titre: String,
    /// La liste du groupe, filtrée sur la même recherche (pour voir plus de dix résultats).
    pub tout_voir: String,
    pub resultats: Vec<Resultat>,
}

#[derive(Template)]
#[template(path = "recherche.html")]
struct Page {
    q: String,
    groupes: Vec<Groupe>,
    total: usize,
}

/// « Client · référence » sans séparateur en trop quand l'un des deux manque.
fn detail(morceaux: &[&str]) -> String {
    morceaux.iter().map(|m| m.trim()).filter(|m| !m.is_empty()).collect::<Vec<_>>().join(" · ")
}

const CLIENT: &str = "COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '')";

async fn devis(db: &mut PgConnection, q: &str, motif: &str) -> Result<Vec<Resultat>, sqlx::Error> {
    let lignes: Vec<(i64, i32, i64, bool, String, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT d.id, extract(year FROM d.date_devis)::int, d.numero, d.chantier_id IS NOT NULL, d.reference, {CLIENT}, d.montant_ht_cents
         FROM devis d
         LEFT JOIN entreprises e ON e.id = d.client_entreprise_id
         LEFT JOIN contacts c ON c.id = d.client_contact_id
         WHERE d.numero::text = $1
            OR (extract(year FROM d.date_devis)::int || '-' || d.numero) ILIKE $2
            OR d.reference ILIKE $2
            OR {CLIENT} ILIKE $2
         ORDER BY d.date_devis DESC, d.id DESC LIMIT $3"
    )))
    .bind(q)
    .bind(motif)
    .bind(PAR_GROUPE)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(id, annee, numero, avenant, reference, client, ht)| Resultat {
            lien: format!("/devis/{id}"),
            titre: format!(
                "{} n° {}",
                if avenant { "Avenant" } else { "Devis" },
                crate::devis_lignes::numero_affiche(annee, numero, avenant)
            ),
            detail: detail(&[&client, &reference]),
            montant: format!("{} HT", format_euros(ht)),
        })
        .collect())
}

async fn factures(db: &mut PgConnection, q: &str, motif: &str) -> Result<Vec<Resultat>, sqlx::Error> {
    let lignes: Vec<(i64, String, Option<i64>, NaiveDate, String, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.id, f.genre::text, f.numero, f.date_facture, f.reference, {CLIENT}, f.montant_ttc_cents
         FROM factures f
         LEFT JOIN entreprises e ON e.id = f.client_entreprise_id
         LEFT JOIN contacts c ON c.id = f.client_contact_id
         WHERE f.numero::text = $1
            OR (extract(year FROM f.date_facture)::int || '-' || f.numero) ILIKE $2
            OR f.reference ILIKE $2
            OR {CLIENT} ILIKE $2
         ORDER BY f.date_facture DESC, f.id DESC LIMIT $3"
    )))
    .bind(q)
    .bind(motif)
    .bind(PAR_GROUPE)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(id, genre, numero, date, reference, client, ttc)| Resultat {
            lien: format!("/factures/{id}"),
            titre: crate::factures::titre(&genre, numero, date),
            detail: detail(&[&client, &reference]),
            montant: format!("{} TTC", format_euros(ttc)),
        })
        .collect())
}

async fn chantiers(db: &mut PgConnection, q: &str, motif: &str) -> Result<Vec<Resultat>, sqlx::Error> {
    let lignes: Vec<(i64, i64, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT ch.id, ch.numero, d.reference, {CLIENT}
         FROM chantiers ch
         JOIN devis d ON d.id = ch.devis_id
         LEFT JOIN entreprises e ON e.id = d.client_entreprise_id
         LEFT JOIN contacts c ON c.id = d.client_contact_id
         WHERE ch.numero::text = $1 OR d.reference ILIKE $2 OR {CLIENT} ILIKE $2
         ORDER BY ch.numero DESC LIMIT $3"
    )))
    .bind(q)
    .bind(motif)
    .bind(PAR_GROUPE)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(id, numero, reference, client)| Resultat {
            lien: format!("/chantiers/{id}"),
            titre: format!("{} n° {numero}", crate::profil::mot("Chantier")),
            detail: detail(&[&reference, &client]),
            montant: String::new(),
        })
        .collect())
}

async fn contacts(db: &mut PgConnection, motif: &str) -> Result<Vec<Resultat>, sqlx::Error> {
    let lignes: Vec<(String, i64, String, String)> = sqlx::query_as(
        "(SELECT 'entreprises', id, nom, concat_ws(' · ', NULLIF(email, ''), NULLIF(numero_tva, '')) FROM entreprises
          WHERE nom ILIKE $1 OR COALESCE(email, '') ILIKE $1 OR COALESCE(numero_tva, '') ILIKE $1 ORDER BY nom LIMIT $2)
         UNION ALL
         (SELECT 'contacts', c.id, trim(concat_ws(' ', c.prenom, c.nom)), concat_ws(' · ', NULLIF(e.nom, ''), NULLIF(c.email, ''))
          FROM contacts c LEFT JOIN entreprises e ON e.id = c.entreprise_id
          WHERE concat_ws(' ', c.prenom, c.nom) ILIKE $1 OR COALESCE(c.email, '') ILIKE $1 ORDER BY c.nom LIMIT $2)",
    )
    .bind(motif)
    .bind(PAR_GROUPE)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .take(PAR_GROUPE as usize)
        .map(|(genre, id, nom, detail)| Resultat { lien: format!("/{genre}/{id}/modifier"), titre: nom, detail, montant: String::new() })
        .collect())
}

async fn page(mut db: Base, Query(s): Query<Saisie>) -> Result<Html<String>, Erreur> {
    let q = s.q.trim().to_string();
    let mut groupes = Vec::new();
    if !q.is_empty() {
        let motif = format!("%{}%", echapper_like(&q));
        let filtre = encoder_url(&q);
        groupes.push(Groupe { titre: "Devis".to_string(), tout_voir: format!("/devis?q={filtre}"), resultats: devis(&mut db, &q, &motif).await? });
        groupes.push(Groupe { titre: "Factures".to_string(), tout_voir: format!("/factures?q={filtre}"), resultats: factures(&mut db, &q, &motif).await? });
        groupes.push(Groupe {
            titre: crate::profil::mot("Chantiers").to_string(),
            tout_voir: format!("/chantiers?statut=tous&q={filtre}"),
            resultats: chantiers(&mut db, &q, &motif).await?,
        });
        groupes.push(Groupe { titre: "Contacts".to_string(), tout_voir: format!("/entreprises?q={filtre}"), resultats: contacts(&mut db, &motif).await? });
    }
    let total = groupes.iter().map(|g| g.resultats.len()).sum();
    Ok(Html(Page { q, groupes, total }.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/recherche", get(page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_detail_saute_ce_qui_manque() {
        assert_eq!(detail(&["Marc Lambert", "L63 Toiture"]), "Marc Lambert · L63 Toiture");
        assert_eq!(detail(&["", "L63 Toiture"]), "L63 Toiture");
        assert_eq!(detail(&["Marc Lambert", " "]), "Marc Lambert");
    }
}
