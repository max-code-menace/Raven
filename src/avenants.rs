//! Avenants (étape 6-2) : un avenant est un devis rattaché à un chantier, qui corrige le devis de base
//! (en plus ou en moins) après l'acceptation. Il réutilise l'éditeur, les totaux, le PDF et les statuts du devis.
//! Seuls les avenants acceptés comptent dans le total du chantier.

use axum::{
    Router,
    extract::{Path, State},
    response::Redirect,
    routing::post,
};
use chrono::NaiveDate;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::commun::format_euros;
use crate::devis::statut;
use crate::journal::{self, Genre};
use crate::{AppState, Erreur};

/// Le prochain numéro d'avenant du chantier. Le devis de base compte pour le n° 1 : le premier avenant est le n° 2.
/// La ligne du chantier est verrouillée : deux avenants créés en même temps n'ont jamais le même numéro.
pub async fn prochain_numero(tx: &mut Transaction<'_, Postgres>, chantier_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query("SELECT id FROM chantiers WHERE id = $1 FOR UPDATE").bind(chantier_id).execute(&mut **tx).await?;
    sqlx::query_scalar("SELECT COALESCE(max(numero), 1) + 1 FROM devis WHERE chantier_id = $1")
        .bind(chantier_id)
        .fetch_one(&mut **tx)
        .await
}

/// Crée un avenant en brouillon : il reprend l'en-tête du devis de base (client, facturation, adresse,
/// TVA, conditions…), sans ses lignes. On l'ouvre ensuite dans l'éditeur, comme un devis.
async fn creer(State(state): State<AppState>, Path(chantier_id): Path<i64>) -> Result<Redirect, Erreur> {
    let mut tx = state.db.begin().await?;
    let chantier: Option<(i64, i64)> = sqlx::query_as("SELECT numero, devis_id FROM chantiers WHERE id = $1")
        .bind(chantier_id)
        .fetch_optional(&mut *tx)
        .await?;
    let (numero_chantier, devis_base) = chantier.ok_or_else(Erreur::introuvable)?;
    let numero = prochain_numero(&mut tx, chantier_id).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, chantier_id, date_devis, reference, responsable, client_entreprise_id, client_contact_id,
                taux_tva_id, conditions_paiement_id, facturation_identique, facturation_entreprise_id, facturation_contact_id,
                chantier_identique, chantier_rue, chantier_numero, chantier_code_postal, chantier_ville, chantier_pays,
                architecte_contact_id, gestionnaire_contact_id, marge_activee, frais_generaux_centiemes)
         SELECT $2, $3, (now() AT TIME ZONE 'Europe/Brussels')::date, reference, responsable, client_entreprise_id, client_contact_id,
                taux_tva_id, conditions_paiement_id, facturation_identique, facturation_entreprise_id, facturation_contact_id,
                chantier_identique, chantier_rue, chantier_numero, chantier_code_postal, chantier_ville, chantier_pays,
                architecte_contact_id, gestionnaire_contact_id, marge_activee, frais_generaux_centiemes
         FROM devis WHERE id = $1
         RETURNING id",
    )
    .bind(devis_base)
    .bind(numero)
    .bind(chantier_id)
    .fetch_one(&mut *tx)
    .await?;
    journal::noter(&mut tx, id, Genre::Creation, &format!("Avenant créé pour le chantier n° {numero_chantier}")).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/devis/{id}?ok=avenant")))
}

// ─── Ce que la fiche du chantier affiche ────────────────────────────────────

#[derive(FromRow)]
struct LigneBase {
    id: i64,
    numero: i64,
    date_devis: NaiveDate,
    reference: String,
    jours_ouvrables: i32,
    montant_ht_cents: i64,
    statut: String,
    date_acceptation: Option<NaiveDate>,
}

pub struct Ligne {
    pub id: i64,
    pub numero: i64,
    pub date: String,
    pub reference: String,
    pub jours_ouvrables: i32,
    pub montant_ht: String,
    pub statut: &'static str,
    pub classe_statut: &'static str,
    /// « 05/10/2026 » pour un avenant accepté.
    pub date_acceptation: String,
}

/// Les avenants d'un chantier et ce qu'ils ajoutent au devis de base.
pub struct Recap {
    pub lignes: Vec<Ligne>,
    /// Somme HT des avenants acceptés, en centimes (négative si les avenants retirent des travaux).
    pub acceptes_ht_cents: i64,
    /// Jours ouvrables supplémentaires des avenants acceptés.
    pub jours_acceptes: i64,
}

pub async fn recap(db: &PgPool, chantier_id: i64) -> Result<Recap, sqlx::Error> {
    let lignes: Vec<LigneBase> = sqlx::query_as(
        "SELECT id, numero, date_devis, reference, jours_ouvrables, montant_ht_cents, statut::text AS statut, date_acceptation
         FROM devis WHERE chantier_id = $1 ORDER BY numero",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await?;
    let acceptes = lignes.iter().filter(|l| l.statut == "accepte");
    let acceptes_ht_cents = acceptes.clone().map(|l| l.montant_ht_cents).sum();
    let jours_acceptes = acceptes.map(|l| i64::from(l.jours_ouvrables)).sum();
    Ok(Recap {
        acceptes_ht_cents,
        jours_acceptes,
        lignes: lignes
            .into_iter()
            .map(|l| {
                let (libelle, classe) = statut(&l.statut);
                Ligne {
                    id: l.id,
                    numero: l.numero,
                    date: l.date_devis.format("%d/%m/%Y").to_string(),
                    reference: l.reference,
                    jours_ouvrables: l.jours_ouvrables,
                    montant_ht: format_euros(l.montant_ht_cents),
                    statut: libelle,
                    classe_statut: classe,
                    date_acceptation: l.date_acceptation.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(),
                }
            })
            .collect(),
    })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/chantiers/{id}/avenants", post(creer))
}
