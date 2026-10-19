//! Paiements des factures (étape 8-1) : paiement partiel ou complet, méthode de paiement, reste à recevoir.
//!
//! Le statut de paiement (« Partiellement payée », « Payée », « En retard ») n'est pas enregistré : la vue
//! `factures_etats` le calcule à chaque lecture depuis les paiements, les notes de crédit et l'échéance.
//! Un paiement ne dépasse jamais le reste à recevoir ; il se retire en cas d'erreur (noté dans l'historique).

use axum::{
    Router,
    extract::{Form, Path},
    response::Redirect,
    routing::post,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};

use crate::base::Base;
use crate::commun::format_euros;
use crate::fiches::euros_vers_centimes;
use crate::{AppState, Erreur};

const LONGUEUR_MAX_REMARQUE: usize = 500;

/// Code de l'état, libellé, couleur de la pastille.
pub(crate) const ETATS: [(&str, &str, &str); 7] = [
    ("brouillon", "Brouillon", "s-brouillon"),
    ("a_envoyer", "À envoyer", "s-a-envoyer"),
    ("envoyee", "Envoyée", "s-envoye"),
    ("partielle", "Partiellement payée", "s-partiel"),
    ("en_retard", "En retard", "s-retard"),
    ("payee", "Payée", "s-accepte"),
    ("creditee", "Créditée", "s-termine"),
];

pub(crate) fn etat(code: &str) -> (&'static str, &'static str) {
    ETATS.iter().find(|(v, _, _)| *v == code).map(|(_, l, c)| (*l, *c)).unwrap_or(("Inconnu", "s-brouillon"))
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// L'état d'une facture tel que la vue `factures_etats` le calcule.
#[derive(FromRow, Clone, Debug)]
pub(crate) struct Solde {
    pub(crate) paye_cents: i64,
    pub(crate) credite_cents: i64,
    pub(crate) reste_cents: i64,
    pub(crate) dernier_paiement: Option<NaiveDate>,
    pub(crate) etat: String,
    pub(crate) jours_retard: i32,
}

impl Solde {
    /// Le petit texte sous la pastille : « le 16/10/2026 », « reste 1 852,54 € », « échue le 22/09/2026 · 24 jours ».
    pub(crate) fn detail(&self, echeance: Option<NaiveDate>) -> String {
        match self.etat.as_str() {
            "payee" => self.dernier_paiement.map(|d| format!("le {}", date_belge(d))).unwrap_or_default(),
            "partielle" => format!("reste {}", format_euros(self.reste_cents)),
            "en_retard" => match echeance {
                Some(e) => format!("échue le {} · {} jour{}", date_belge(e), self.jours_retard, if self.jours_retard > 1 { "s" } else { "" }),
                None => String::new(),
            },
            "a_envoyer" | "envoyee" => echeance.map(|e| format!("échéance {}", date_belge(e))).unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// On peut encore encaisser quelque chose sur cette facture.
    pub(crate) fn a_recevoir(&self) -> bool {
        self.reste_cents > 0
    }
}

pub(crate) async fn solde<'e>(db: impl sqlx::PgExecutor<'e>, facture_id: i64) -> Result<Solde, sqlx::Error> {
    sqlx::query_as(
        "SELECT paye_cents, credite_cents, reste_cents, dernier_paiement, etat, jours_retard FROM factures_etats WHERE facture_id = $1",
    )
    .bind(facture_id)
    .fetch_one(db)
    .await
}

/// Les méthodes de paiement proposées (id, libellé).
pub(crate) async fn methodes(db: &mut PgConnection) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as("SELECT id, libelle FROM methodes_paiement WHERE actif ORDER BY position, id").fetch_all(&mut *db).await
}

/// Un paiement, tel que la fiche de la facture le montre.
pub struct LignePaiement {
    pub id: i64,
    pub date: String,
    pub methode: String,
    pub montant: String,
    pub remarque: String,
}

pub(crate) async fn lignes(db: &mut PgConnection, facture_id: i64) -> Result<Vec<LignePaiement>, sqlx::Error> {
    let lignes: Vec<(i64, NaiveDate, String, i64, String)> = sqlx::query_as(
        "SELECT p.id, p.date_paiement, m.libelle, p.montant_cents, p.remarque
         FROM paiements p JOIN methodes_paiement m ON m.id = p.methode_id
         WHERE p.facture_id = $1 ORDER BY p.date_paiement, p.id",
    )
    .bind(facture_id)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(id, date, methode, montant, remarque)| LignePaiement { id, date: date_belge(date), methode, montant: format_euros(montant), remarque })
        .collect())
}

/// Écrit un événement dans l'historique de la facture (même transaction que le changement).
pub(crate) async fn noter(tx: &mut Transaction<'_, Postgres>, facture_id: i64, genre: &str, detail: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO facture_evenements (facture_id, genre, detail) VALUES ($1, $2, $3)")
        .bind(facture_id)
        .bind(genre)
        .bind(detail)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

// ─── Ajouter un paiement ────────────────────────────────────────────────────

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub(crate) struct NouveauPaiement {
    /// « complet » (le reste à recevoir) ou « partiel » (le montant saisi).
    pub(crate) genre: String,
    pub(crate) date: String,
    pub(crate) montant: String,
    pub(crate) methode_id: String,
    pub(crate) remarque: String,
}

/// Ce qui ne va pas dans le paiement saisi.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refus {
    Date,
    Montant,
    Methode,
    Remarque,
}

impl Refus {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Refus::Date => "paiement_date",
            Refus::Montant => "paiement_montant",
            Refus::Methode => "paiement_methode",
            Refus::Remarque => "paiement_remarque",
        }
    }
}

/// Vérifie le paiement saisi : (date, montant en centimes, méthode, remarque).
/// Un paiement complet vaut le reste à recevoir ; un paiement partiel est positif et ne dépasse pas ce reste.
pub(crate) fn lire(p: &NouveauPaiement, reste_cents: i64, aujourdhui: NaiveDate, methodes: &[(i64, String)]) -> Result<(NaiveDate, i64, i64, String), Refus> {
    let date = NaiveDate::parse_from_str(p.date.trim(), "%Y-%m-%d").ok().filter(|d| *d <= aujourdhui).ok_or(Refus::Date)?;
    let montant = match p.genre.as_str() {
        "complet" => reste_cents,
        _ => euros_vers_centimes(&p.montant).ok_or(Refus::Montant)?,
    };
    if montant <= 0 || montant > reste_cents {
        return Err(Refus::Montant);
    }
    let methode = p.methode_id.trim().parse::<i64>().ok().filter(|m| methodes.iter().any(|(i, _)| i == m)).ok_or(Refus::Methode)?;
    let remarque = p.remarque.trim().to_string();
    if remarque.chars().count() > LONGUEUR_MAX_REMARQUE {
        return Err(Refus::Remarque);
    }
    Ok((date, montant, methode, remarque))
}

async fn ajouter(mut db: Base, Path(id): Path<i64>, Form(p): Form<NouveauPaiement>) -> Result<Redirect, Erreur> {
    let methodes = methodes(&mut *db).await?;
    let mut tx = db.begin().await?;
    // Verrouille la facture : deux paiements en même temps ne dépassent pas le reste à recevoir.
    let facture: Option<(Option<i64>, String)> =
        sqlx::query_as("SELECT numero, genre::text FROM factures WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let (numero, genre) = facture.ok_or_else(Erreur::introuvable)?;
    if numero.is_none() || genre == "note_credit" {
        return Err(Erreur::conflit("Un paiement s'enregistre sur une facture validée (pas sur un brouillon ni une note de crédit)."));
    }
    let s = solde(&mut *tx, id).await?;
    if !s.a_recevoir() {
        return Err(Erreur::conflit("Cette facture est déjà soldée : il n'y a plus rien à recevoir."));
    }
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *tx).await?;
    let (date, montant, methode, remarque) = match lire(&p, s.reste_cents, aujourdhui, &methodes) {
        Ok(v) => v,
        Err(refus) => return Ok(Redirect::to(&format!("/factures/{id}?erreur={}&payer=1#paiement", refus.code()))),
    };
    sqlx::query("INSERT INTO paiements (facture_id, date_paiement, montant_cents, methode_id, remarque) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(date)
        .bind(montant)
        .bind(methode)
        .bind(&remarque)
        .execute(&mut *tx)
        .await?;
    let libelle = methodes.iter().find(|(i, _)| *i == methode).map(|(_, l)| l.as_str()).unwrap_or("");
    let complet = montant == s.reste_cents;
    noter(
        &mut tx,
        id,
        "paiement",
        &format!("{} de {} le {} ({libelle})", if complet { "Paiement complet" } else { "Paiement partiel" }, format_euros(montant), date_belge(date)),
    )
    .await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/factures/{id}?ok={}", if complet { "payee" } else { "paiement" })))
}

/// Retire un paiement saisi par erreur (noté dans l'historique de la facture).
async fn supprimer(mut db: Base, Path((id, paiement)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let mut tx = db.begin().await?;
    let ligne: Option<(NaiveDate, i64)> =
        sqlx::query_as("DELETE FROM paiements WHERE id = $1 AND facture_id = $2 RETURNING date_paiement, montant_cents")
            .bind(paiement)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (date, montant) = ligne.ok_or_else(Erreur::introuvable)?;
    noter(&mut tx, id, "paiement_supprime", &format!("Paiement de {} du {} retiré", format_euros(montant), date_belge(date))).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/factures/{id}?ok=paiement_retire")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/factures/{id}/paiements", post(ajouter))
        .route("/factures/{id}/paiements/{paiement}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    fn saisie(genre: &str, date: &str, montant: &str, methode: &str) -> NouveauPaiement {
        NouveauPaiement {
            genre: genre.to_string(),
            date: date.to_string(),
            montant: montant.to_string(),
            methode_id: methode.to_string(),
            remarque: String::new(),
        }
    }

    const METHODES: [(i64, &str); 2] = [(1, "Virement"), (2, "Bancontact")];

    fn methodes() -> Vec<(i64, String)> {
        METHODES.iter().map(|(i, l)| (*i, l.to_string())).collect()
    }

    #[test]
    fn un_paiement_complet_vaut_le_reste() {
        let r = lire(&saisie("complet", "2026-10-16", "n'importe quoi", "1"), 385_254, jour(2026, 10, 16), &methodes());
        assert_eq!(r, Ok((jour(2026, 10, 16), 385_254, 1, String::new())));
    }

    #[test]
    fn un_paiement_partiel_ne_depasse_pas_le_reste() {
        let auj = jour(2026, 10, 16);
        assert_eq!(lire(&saisie("partiel", "2026-10-01", "2 000,00", "2"), 385_254, auj, &methodes()).map(|v| v.1), Ok(200_000));
        assert_eq!(lire(&saisie("partiel", "2026-10-01", "3852,55", "1"), 385_254, auj, &methodes()), Err(Refus::Montant));
        assert_eq!(lire(&saisie("partiel", "2026-10-01", "0", "1"), 385_254, auj, &methodes()), Err(Refus::Montant));
        assert_eq!(lire(&saisie("partiel", "2026-10-01", "-5", "1"), 385_254, auj, &methodes()), Err(Refus::Montant));
        assert_eq!(lire(&saisie("partiel", "2026-10-01", "abc", "1"), 385_254, auj, &methodes()), Err(Refus::Montant));
    }

    #[test]
    fn pas_de_paiement_dans_le_futur_ni_de_methode_inconnue() {
        let auj = jour(2026, 10, 16);
        assert_eq!(lire(&saisie("complet", "2026-10-17", "", "1"), 100, auj, &methodes()), Err(Refus::Date));
        assert_eq!(lire(&saisie("complet", "16/10/2026", "", "1"), 100, auj, &methodes()), Err(Refus::Date));
        assert_eq!(lire(&saisie("complet", "2026-10-16", "", "9"), 100, auj, &methodes()), Err(Refus::Methode));
    }

    #[test]
    fn chaque_etat_a_un_libelle() {
        assert_eq!(etat("partielle").0, "Partiellement payée");
        assert_eq!(etat("payee").0, "Payée");
        assert_eq!(etat("en_retard").0, "En retard");
        assert_eq!(etat("x").0, "Inconnu");
    }

    #[test]
    fn le_detail_sous_la_pastille() {
        let s = |etat: &str, reste: i64, jours: i32| Solde {
            paye_cents: 200_000,
            credite_cents: 0,
            reste_cents: reste,
            dernier_paiement: Some(jour(2026, 10, 1)),
            etat: etat.to_string(),
            jours_retard: jours,
        };
        let echeance = Some(jour(2026, 9, 22));
        assert_eq!(s("payee", 0, 0).detail(echeance), "le 01/10/2026");
        assert_eq!(s("partielle", 185_254, 0).detail(echeance), "reste 1\u{a0}852,54\u{a0}€");
        assert_eq!(s("en_retard", 185_254, 24).detail(echeance), "échue le 22/09/2026 · 24 jours");
        assert_eq!(s("en_retard", 185_254, 1).detail(echeance), "échue le 22/09/2026 · 1 jour");
        assert_eq!(s("envoyee", 385_254, 0).detail(echeance), "échéance 22/09/2026");
    }
}
