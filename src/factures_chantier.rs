//! Factures d'un chantier (étape 7-2) : acompte, avancement et finale. Leurs lignes sont calculées par le serveur
//! et ne se saisissent pas ; l'en-tête (client de facturation, conditions de paiement, référence) vient du devis.
//!
//! - **Acompte** : un pourcentage ou un montant HT du total devis/avenants, réparti entre les taux de TVA du chantier
//!   au prorata de leurs bases.
//! - **Avancement** : reprend un avancement validé, poste par poste (montant de la période).
//! - **Finale** : crée l'avancement « Solde » qui porte chaque poste à 100 %, le facture, puis déduit les acomptes
//!   déjà facturés (moins leurs notes de crédit). Total facturé = total devis/avenants.

use axum::{
    Router,
    extract::{Form, Path, State},
    response::Redirect,
    routing::post,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::avancement_calcul as ac;
use crate::avancements;
use crate::calcul;
use crate::commun::format_euros;
use crate::facture_calcul;
use crate::factures::{self, NouvelleLigne};
use crate::{AppState, Erreur};

// ─── Le récapitulatif dans l'onglet Finances ────────────────────────────────

pub struct LigneFacture {
    pub id: i64,
    pub titre: String,
    pub genre: &'static str,
    pub date: String,
    pub montant_ht: String,
    pub montant_ttc: String,
    pub statut: &'static str,
    pub classe_statut: &'static str,
}

pub struct Recap {
    pub factures: Vec<LigneFacture>,
    /// Total HT des factures validées, notes de crédit déduites.
    pub facture_ht: String,
    pub facture_ttc: String,
    /// Total devis/avenants HT moins le facturé HT.
    pub reste_a_facturer: String,
}

pub async fn recap(db: &PgPool, chantier_id: i64, total_cents: i64) -> Result<Recap, sqlx::Error> {
    let lignes: Vec<(i64, String, Option<i64>, NaiveDate, i64, i64, String)> = sqlx::query_as(
        "SELECT f.id, f.genre::text, f.numero, f.date_facture,
                CASE WHEN f.genre = 'note_credit' THEN -f.montant_ht_cents ELSE f.montant_ht_cents END,
                CASE WHEN f.genre = 'note_credit' THEN -f.montant_ttc_cents ELSE f.montant_ttc_cents END,
                et.etat
         FROM factures f JOIN factures_etats et ON et.facture_id = f.id WHERE f.chantier_id = $1
         ORDER BY f.numero IS NULL, f.date_facture, f.id",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await?;
    let (mut ht, mut ttc) = (0, 0);
    let factures = lignes
        .into_iter()
        .map(|(id, genre, numero, date, montant_ht, montant_ttc, statut)| {
            if numero.is_some() {
                ht += montant_ht;
                ttc += montant_ttc;
            }
            let (libelle, classe) = factures::statut(&statut);
            LigneFacture {
                id,
                titre: factures::titre(&genre, numero),
                genre: factures::libelle_genre(&genre),
                date: date.format("%d/%m/%Y").to_string(),
                montant_ht: format_euros(montant_ht),
                montant_ttc: format_euros(montant_ttc),
                statut: libelle,
                classe_statut: classe,
            }
        })
        .collect();
    Ok(Recap { factures, facture_ht: format_euros(ht), facture_ttc: format_euros(ttc), reste_a_facturer: format_euros(total_cents - ht) })
}

// ─── L'en-tête repris du devis ──────────────────────────────────────────────

#[derive(FromRow)]
struct Entete {
    client_entreprise_id: Option<i64>,
    client_contact_id: Option<i64>,
    conditions_paiement_id: Option<i64>,
    taux_tva_id: Option<i64>,
    reference: String,
    responsable: String,
    devis_numero: String,
}

/// Le client de facturation du devis (le client lui-même, ou celui choisi dans « facturé à »).
async fn entete(tx: &mut Transaction<'_, Postgres>, chantier_id: i64) -> Result<Option<Entete>, sqlx::Error> {
    sqlx::query_as(
        "SELECT CASE WHEN d.facturation_identique THEN d.client_entreprise_id ELSE d.facturation_entreprise_id END AS client_entreprise_id,
                CASE WHEN d.facturation_identique THEN d.client_contact_id ELSE d.facturation_contact_id END AS client_contact_id,
                d.conditions_paiement_id, d.taux_tva_id, d.reference, d.responsable,
                extract(year FROM d.date_devis)::int || '-' || d.numero AS devis_numero
         FROM chantiers ch JOIN devis d ON d.id = ch.devis_id WHERE ch.id = $1 FOR UPDATE OF ch",
    )
    .bind(chantier_id)
    .fetch_optional(&mut **tx)
    .await
}

async fn creer_facture(
    tx: &mut Transaction<'_, Postgres>,
    genre: &str,
    chantier_id: i64,
    avancement_id: Option<i64>,
    e: &Entete,
    reference: &str,
    lignes: &[NouvelleLigne],
) -> Result<i64, sqlx::Error> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO factures (genre, date_facture, reference, responsable, client_entreprise_id, client_contact_id,
                               conditions_paiement_id, taux_tva_id, chantier_id, avancement_id)
         VALUES ($1::text::genre_facture, (now() AT TIME ZONE 'Europe/Brussels')::date, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING id",
    )
    .bind(genre)
    .bind(reference)
    .bind(&e.responsable)
    .bind(e.client_entreprise_id)
    .bind(e.client_contact_id)
    .bind(e.conditions_paiement_id)
    .bind(e.taux_tva_id)
    .bind(chantier_id)
    .bind(avancement_id)
    .fetch_one(&mut **tx)
    .await?;
    factures::ajouter_lignes(tx, id, lignes).await?;
    factures::recalculer(tx, id).await?;
    Ok(id)
}

fn vers_finances(chantier_id: i64, erreur: &str) -> Result<Redirect, Erreur> {
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/finances?erreur={erreur}")))
}

fn vers_facture(id: i64) -> Result<Redirect, Erreur> {
    Ok(Redirect::to(&format!("/factures/{id}?ok=creee")))
}

async fn facture_en_brouillon<'e>(db: impl sqlx::PgExecutor<'e>, chantier_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM factures WHERE chantier_id = $1 AND statut = 'brouillon')").bind(chantier_id).fetch_one(db).await
}

// ─── Acompte ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct Acompte {
    /// « pourcentage » ou « montant ».
    mode: String,
    valeur: String,
}

/// Le montant HT de l'acompte, entre 0 (exclu) et le total du chantier.
fn montant_acompte(a: &Acompte, total_cents: i64) -> Option<(i64, Option<i64>)> {
    let valeur = calcul::lire_decimal_e5(&a.valeur)?;
    let (montant, pourcentage) = match a.mode.as_str() {
        "pourcentage" => (ac::montant_depuis_pourcentage(total_cents, valeur), Some(valeur)),
        "montant" => (ac::montant_depuis_euros(valeur), None),
        _ => return None,
    };
    (montant > 0 && montant <= total_cents).then_some((montant, pourcentage))
}

async fn creer_acompte(State(state): State<AppState>, Path(chantier_id): Path<i64>, Form(a): Form<Acompte>) -> Result<Redirect, Erreur> {
    let total: i64 = avancements::bases_par_taux(&state.db, chantier_id).await?.iter().map(|(_, b)| b).sum();
    let mut tx = state.db.begin().await?;
    let e = entete(&mut tx, chantier_id).await?.ok_or_else(Erreur::introuvable)?;
    if avancements::a_une_finale(&mut *tx, chantier_id).await? {
        return vers_finances(chantier_id, "finale");
    }
    let Some((montant, pourcentage)) = montant_acompte(&a, total) else {
        return vers_finances(chantier_id, "acompte");
    };
    let bases = avancements::bases_par_taux(&state.db, chantier_id).await?;
    let valeurs: Vec<i64> = bases.iter().map(|(_, b)| *b).collect();
    let numero_acompte: i64 = sqlx::query_scalar("SELECT count(*) + 1 FROM factures WHERE chantier_id = $1 AND genre = 'acompte'")
        .bind(chantier_id)
        .fetch_one(&mut *tx)
        .await?;
    let description = match pourcentage {
        Some(p) => format!("Acompte de {} % sur le devis {}", calcul::ecrire_decimal_e5(p), e.devis_numero),
        None => format!("Acompte sur le devis {}", e.devis_numero),
    };
    let lignes: Vec<NouvelleLigne> = bases
        .iter()
        .zip(facture_calcul::repartir(montant, &valeurs))
        .filter(|(_, part)| *part != 0)
        .map(|((taux, _), part)| NouvelleLigne::forfait(description.clone(), *taux, part))
        .collect();
    let reference = format!("Facture d'acompte #{numero_acompte} · {}", e.reference);
    let id = creer_facture(&mut tx, "acompte", chantier_id, None, &e, reference.trim_end_matches(" · "), &lignes).await?;
    tx.commit().await?;
    vers_facture(id)
}

// ─── Avancement ─────────────────────────────────────────────────────────────

async fn facturer_avancement(State(state): State<AppState>, Path((chantier_id, avancement_id)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let mut tx = state.db.begin().await?;
    let e = entete(&mut tx, chantier_id).await?.ok_or_else(Erreur::introuvable)?;
    let a: Option<(i32, String, String)> = sqlx::query_as(
        "SELECT numero, reference, statut::text FROM avancements WHERE id = $1 AND chantier_id = $2 FOR UPDATE",
    )
    .bind(avancement_id)
    .bind(chantier_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (numero, reference, statut) = a.ok_or_else(Erreur::introuvable)?;
    if statut != "valide" {
        return Err(Erreur::conflit("Valide d'abord l'avancement : seul un avancement validé se facture."));
    }
    let deja: Option<i64> = sqlx::query_scalar("SELECT id FROM factures WHERE avancement_id = $1").bind(avancement_id).fetch_optional(&mut *tx).await?;
    if let Some(id) = deja {
        return Ok(Redirect::to(&format!("/factures/{id}")));
    }
    let lignes = avancements::lignes_de_facture(&state.db, chantier_id, numero).await?;
    if lignes.is_empty() {
        return vers_finances(chantier_id, "rien");
    }
    let id = creer_facture(&mut tx, "avancement", chantier_id, Some(avancement_id), &e, &reference, &lignes).await?;
    tx.commit().await?;
    vers_facture(id)
}

// ─── Finale ─────────────────────────────────────────────────────────────────

/// Les acomptes validés du chantier, moins leurs notes de crédit validées, par taux de TVA.
async fn acomptes_par_taux(tx: &mut Transaction<'_, Postgres>, chantier_id: i64) -> Result<(Vec<(i64, i64)>, String), sqlx::Error> {
    let par_taux: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT l.taux_tva_id, sum(CASE WHEN f.genre = 'note_credit' THEN -l.total_cents ELSE l.total_cents END)::bigint
         FROM facture_lignes l JOIN factures f ON f.id = l.facture_id
         WHERE l.genre = 'poste' AND f.numero IS NOT NULL
           AND ((f.genre = 'acompte' AND f.chantier_id = $1)
                OR (f.genre = 'note_credit' AND f.facture_origine_id IN (SELECT id FROM factures WHERE genre = 'acompte' AND chantier_id = $1)))
         GROUP BY l.taux_tva_id ORDER BY l.taux_tva_id",
    )
    .bind(chantier_id)
    .fetch_all(&mut **tx)
    .await?;
    let numeros: Option<String> = sqlx::query_scalar(
        "SELECT string_agg(numero::text, ', ' ORDER BY numero) FROM factures WHERE genre = 'acompte' AND chantier_id = $1 AND numero IS NOT NULL",
    )
    .bind(chantier_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok((par_taux.into_iter().filter(|(_, m)| *m != 0).collect(), numeros.unwrap_or_default()))
}

async fn creer_finale(State(state): State<AppState>, Path(chantier_id): Path<i64>) -> Result<Redirect, Erreur> {
    let mut tx = state.db.begin().await?;
    let e = entete(&mut tx, chantier_id).await?.ok_or_else(Erreur::introuvable)?;
    if avancements::a_une_finale(&mut *tx, chantier_id).await? {
        return vers_finances(chantier_id, "finale");
    }
    let avancement_brouillon: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM avancements WHERE chantier_id = $1 AND statut = 'brouillon')")
            .bind(chantier_id)
            .fetch_one(&mut *tx)
            .await?;
    if avancement_brouillon {
        return vers_finances(chantier_id, "avancement_brouillon");
    }
    // Un acompte en brouillon ne serait pas déduit : on demande de le valider ou de le supprimer d'abord.
    if facture_en_brouillon(&mut *tx, chantier_id).await? {
        return vers_finances(chantier_id, "facture_brouillon");
    }
    let (soldes, mut lignes) = avancements::preparer_solde(&state.db, chantier_id).await?;
    if soldes.is_empty() {
        return vers_finances(chantier_id, "rien");
    }
    let (avancement_id, _) = avancements::inserer_solde(&mut tx, chantier_id, &soldes).await?;
    let (acomptes, numeros) = acomptes_par_taux(&mut tx, chantier_id).await?;
    if !acomptes.is_empty() {
        lignes.push(NouvelleLigne::titre("Acomptes déjà facturés"));
        for (taux, montant) in acomptes {
            lignes.push(NouvelleLigne::forfait(format!("Déduction des acomptes (facture n° {numeros})"), taux, -montant));
        }
    }
    let reference = format!("Facture finale · {}", e.reference);
    let id = creer_facture(&mut tx, "finale", chantier_id, Some(avancement_id), &e, reference.trim_end_matches(" · "), &lignes).await?;
    tx.commit().await?;
    vers_facture(id)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chantiers/{id}/factures/acompte", post(creer_acompte))
        .route("/chantiers/{id}/factures/finale", post(creer_finale))
        .route("/chantiers/{id}/avancements/{avancement}/facturer", post(facturer_avancement))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acompte(mode: &str, valeur: &str) -> Acompte {
        Acompte { mode: mode.to_string(), valeur: valeur.to_string() }
    }

    #[test]
    fn montant_d_un_acompte() {
        // 30 % de 6 603,77 € = 1 981,131 → 1 981,13 €
        assert_eq!(montant_acompte(&acompte("pourcentage", "30"), 660_377), Some((198_113, Some(3_000_000))));
        assert_eq!(montant_acompte(&acompte("montant", "1500,50"), 660_377), Some((150_050, None)));
        // pas d'acompte nul, négatif, supérieur au total ou illisible
        assert_eq!(montant_acompte(&acompte("pourcentage", "0"), 660_377), None);
        assert_eq!(montant_acompte(&acompte("montant", "-10"), 660_377), None);
        assert_eq!(montant_acompte(&acompte("pourcentage", "101"), 660_377), None);
        assert_eq!(montant_acompte(&acompte("montant", "abc"), 660_377), None);
        assert_eq!(montant_acompte(&acompte("autre", "10"), 660_377), None);
    }
}
