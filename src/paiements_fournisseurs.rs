//! Paiements des factures fournisseurs reçues (23/10/2026, priorité 2 de la comparaison avec Vertuoza, où une facture
//! fournisseur passe de « À valider » à « À payer », « Part. payé » puis « Payé ») : paiement complet ou partiel, date,
//! méthode ; reste à payer, échéance et retard.
//!
//! Comme pour les factures clients (`paiements`), le statut n'est pas enregistré : la vue `factures_recues_etats` le
//! calcule à chaque lecture depuis les paiements et l'échéance. Un paiement ne dépasse jamais le reste à payer (vérifié
//! aussi par la base) ; une note de crédit fournisseur ne se paie pas. Un paiement saisi par erreur se retire.

use axum::{
    Router,
    extract::{Form, Path},
    response::Redirect,
    routing::post,
};
use chrono::NaiveDate;
use sqlx::{FromRow, PgConnection};

use crate::base::Base;
use crate::commun::format_euros;
use crate::paiements::{self, LignePaiement, NouveauPaiement};
use crate::{AppState, Erreur};

/// Code de l'état, libellé, couleur de la pastille. Une note de crédit n'a pas d'état de paiement.
pub(crate) const ETATS: [(&str, &str, &str); 4] = [
    ("a_payer", "À payer", "s-envoye"),
    ("partielle", "Partiellement payée", "s-partiel"),
    ("en_retard", "En retard", "s-retard"),
    ("payee", "Payée", "s-accepte"),
];

/// Le libellé et la couleur de la pastille ; vides pour une note de crédit.
pub(crate) fn etat(code: &str) -> (&'static str, &'static str) {
    ETATS.iter().find(|(v, _, _)| *v == code).map(|(_, l, c)| (*l, *c)).unwrap_or(("", ""))
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// L'état de paiement d'une facture reçue, tel que la vue `factures_recues_etats` le calcule.
#[derive(FromRow, Clone, Debug)]
pub(crate) struct Solde {
    pub(crate) paye_cents: i64,
    pub(crate) reste_cents: i64,
    pub(crate) dernier_paiement: Option<NaiveDate>,
    pub(crate) etat: String,
    pub(crate) jours_retard: i32,
}

impl Solde {
    /// Le petit texte sous la pastille : « le 16/10/2026 », « reste 520,98 € », « échue le 22/09/2026 · 24 jours »,
    /// « échéance 19/11/2026 ».
    pub(crate) fn detail(&self, echeance: Option<NaiveDate>) -> String {
        match self.etat.as_str() {
            "payee" => self.dernier_paiement.map(|d| format!("le {}", date_belge(d))).unwrap_or_default(),
            "partielle" => format!("reste {}", format_euros(self.reste_cents)),
            "en_retard" => match echeance {
                Some(e) => format!("échue le {} · {} jour{}", date_belge(e), self.jours_retard, if self.jours_retard > 1 { "s" } else { "" }),
                None => String::new(),
            },
            "a_payer" => echeance.map(|e| format!("échéance {}", date_belge(e))).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

pub(crate) async fn solde<'e>(db: impl sqlx::PgExecutor<'e>, facture_recue_id: i64) -> Result<Solde, sqlx::Error> {
    sqlx::query_as(
        "SELECT paye_cents, reste_cents, dernier_paiement, etat, jours_retard FROM factures_recues_etats WHERE facture_recue_id = $1",
    )
    .bind(facture_recue_id)
    .fetch_one(db)
    .await
}

/// Les paiements d'une facture reçue, du plus ancien au plus récent.
pub(crate) async fn lignes(db: &mut PgConnection, facture_recue_id: i64) -> Result<Vec<LignePaiement>, sqlx::Error> {
    let lignes: Vec<(i64, NaiveDate, String, i64, String)> = sqlx::query_as(
        "SELECT p.id, p.date_paiement, m.libelle, p.montant_cents, p.remarque
         FROM paiements_fournisseurs p JOIN methodes_paiement m ON m.id = p.methode_id
         WHERE p.facture_recue_id = $1 ORDER BY p.date_paiement, p.id",
    )
    .bind(facture_recue_id)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(id, date, methode, montant, remarque)| LignePaiement { id, date: date_belge(date), methode, montant: format_euros(montant), remarque })
        .collect())
}

/// Ce qui reste à payer aux fournisseurs (TTC), et la part en retard avec le nombre de factures en retard.
#[derive(FromRow, Debug, Default)]
pub(crate) struct Totaux {
    pub(crate) a_payer_cents: i64,
    pub(crate) en_retard_cents: i64,
    pub(crate) nombre_en_retard: i64,
}

pub(crate) async fn totaux(db: &mut PgConnection) -> Result<Totaux, sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(sum(reste_cents), 0)::bigint AS a_payer_cents,
                COALESCE(sum(reste_cents) FILTER (WHERE etat = 'en_retard'), 0)::bigint AS en_retard_cents,
                count(*) FILTER (WHERE etat = 'en_retard') AS nombre_en_retard
         FROM factures_recues_etats",
    )
    .fetch_one(db)
    .await
}

async fn ajouter(mut db: Base, Path(id): Path<i64>, Form(p): Form<NouveauPaiement>) -> Result<Redirect, Erreur> {
    let methodes = paiements::methodes(&mut *db).await?;
    let mut tx = db.begin().await?;
    // Verrouille la facture : deux paiements en même temps ne dépassent pas le reste à payer.
    let genre: Option<String> =
        sqlx::query_scalar("SELECT genre FROM factures_recues WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    if genre.ok_or_else(Erreur::introuvable)? != "facture" {
        return Err(Erreur::conflit("Une note de crédit fournisseur ne se paie pas : elle réduit ce que vous devez au fournisseur."));
    }
    let s = solde(&mut *tx, id).await?;
    if s.reste_cents <= 0 {
        return Err(Erreur::conflit("Cette facture est déjà payée : il n'y a plus rien à payer."));
    }
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *tx).await?;
    let (date, montant, methode, remarque) = match paiements::lire(&p, s.reste_cents, aujourdhui, &methodes) {
        Ok(v) => v,
        Err(refus) => return Ok(Redirect::to(&format!("/achats/recues/{id}?erreur={}&payer=1#paiement", refus.code()))),
    };
    sqlx::query("INSERT INTO paiements_fournisseurs (facture_recue_id, date_paiement, montant_cents, methode_id, remarque) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(date)
        .bind(montant)
        .bind(methode)
        .bind(&remarque)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/achats/recues/{id}?ok={}#paiement", if montant == s.reste_cents { "payee" } else { "paiement" })))
}

/// Retire un paiement saisi par erreur.
async fn supprimer(mut db: Base, Path((id, paiement)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let retire = sqlx::query("DELETE FROM paiements_fournisseurs WHERE id = $1 AND facture_recue_id = $2")
        .bind(paiement)
        .bind(id)
        .execute(&mut *db)
        .await?;
    if retire.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to(&format!("/achats/recues/{id}?ok=paiement_retire#paiement")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/achats/recues/{id}/paiements", post(ajouter))
        .route("/achats/recues/{id}/paiements/{paiement}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    fn solde(etat: &str, reste: i64, jours: i32) -> Solde {
        Solde { paye_cents: 0, reste_cents: reste, dernier_paiement: Some(jour(2026, 10, 16)), etat: etat.into(), jours_retard: jours }
    }

    #[test]
    fn chaque_etat_a_un_libelle() {
        for (code, _, _) in ETATS {
            assert!(!etat(code).0.is_empty(), "{code}");
        }
        assert_eq!(etat("note_credit"), ("", ""), "une note de crédit n'a pas de pastille de paiement");
    }

    #[test]
    fn le_detail_sous_la_pastille() {
        let echeance = Some(jour(2026, 9, 22));
        assert_eq!(solde("payee", 0, 0).detail(echeance), "le 16/10/2026");
        assert_eq!(solde("partielle", 52_098, 0).detail(echeance), "reste 520,98\u{a0}€");
        assert_eq!(solde("en_retard", 100, 24).detail(echeance), "échue le 22/09/2026 · 24 jours");
        assert_eq!(solde("en_retard", 100, 1).detail(echeance), "échue le 22/09/2026 · 1 jour");
        assert_eq!(solde("a_payer", 100, 0).detail(echeance), "échéance 22/09/2026");
        assert_eq!(solde("a_payer", 100, 0).detail(None), "");
    }
}
