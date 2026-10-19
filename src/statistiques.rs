//! Statistiques réduites (étape 9-4), d'après l'onglet « Général » des statistiques de Vertuoza :
//! chiffre d'affaires, encaissements, impayés (à payer et en retard), devis (émis, en cours, acceptés, refusés),
//! taux de conversion et de perte, panier moyen, et le chiffre d'affaires mois par mois. Filtre par période.
//!
//! Définitions (à valider) : chiffre d'affaires = factures et notes de crédit **validées** de la période (date de la
//! pièce), notes de crédit déduites ; encaissé = paiements datés dans la période ; devis = devis de la période (date
//! du devis, pas les avenants ni les brouillons) ; taux de conversion = acceptés ÷ émis.

use askama::Template;
use axum::{
    Router,
    extract::Query,
    response::Html,
    routing::get,
};
use chrono::{Datelike, Duration, NaiveDate};
use serde::Deserialize;
use sqlx::PgConnection;

use crate::base::Base;
use crate::commun::{Choix, format_euros};
use crate::{AppState, Erreur};

pub(crate) const PERIODES: [(&str, &str); 5] = [
    ("annee", "Cette année"),
    ("mois", "Ce mois-ci"),
    ("douze_mois", "12 derniers mois"),
    ("annee_passee", "L'année passée"),
    ("perso", "Autre période"),
];

/// Les dates de la période demandée ; une période inconnue ou mal remplie revient à « Cette année ».
pub(crate) fn periode(code: &str, du: &str, au: &str, aujourdhui: NaiveDate) -> (&'static str, NaiveDate, NaiveDate) {
    let jour = |a: i32, m: u32, j: u32| NaiveDate::from_ymd_opt(a, m, j).expect("date valable");
    let annee = aujourdhui.year();
    match code {
        "mois" => ("mois", jour(annee, aujourdhui.month(), 1), aujourdhui),
        "douze_mois" => {
            let debut = jour(if aujourdhui.month() == 12 { annee } else { annee - 1 }, aujourdhui.month() % 12 + 1, 1);
            ("douze_mois", debut, aujourdhui)
        }
        "annee_passee" => ("annee_passee", jour(annee - 1, 1, 1), jour(annee - 1, 12, 31)),
        "perso" => {
            let lire = |t: &str| NaiveDate::parse_from_str(t.trim(), "%Y-%m-%d").ok();
            match (lire(du), lire(au)) {
                (Some(d), Some(a)) if d <= a && a - d <= Duration::days(3 * 366) => ("perso", d, a),
                _ => ("annee", jour(annee, 1, 1), aujourdhui),
            }
        }
        _ => ("annee", jour(annee, 1, 1), aujourdhui),
    }
}

/// Une part en pour cent, en texte (« 42,86 % »), « — » sans total.
pub(crate) fn part(n: i64, total: i64) -> String {
    if total <= 0 {
        return "—".to_string();
    }
    let centiemes = (n * 10_000 + total / 2) / total;
    crate::calcul::taux_en_texte(centiemes as i32)
}

pub struct Mois {
    pub libelle: String,
    pub montant: String,
    /// Hauteur de la barre, de 0 à 100 (par rapport au meilleur mois).
    pub hauteur: i64,
    pub negatif: bool,
}

#[derive(Template)]
#[template(path = "statistiques.html")]
struct Page {
    periodes: Vec<Choix>,
    du: String,
    au: String,
    periode_texte: String,
    ca_ht: String,
    ca_ttc: String,
    nombre_factures: i64,
    encaisse: String,
    a_payer: String,
    nombre_a_payer: i64,
    en_retard: String,
    nombre_en_retard: i64,
    devis_emis: i64,
    devis_emis_montant: String,
    devis_en_cours: i64,
    devis_en_cours_montant: String,
    devis_acceptes: i64,
    devis_acceptes_montant: String,
    devis_refuses: i64,
    taux_conversion: String,
    taux_perte: String,
    panier_moyen: String,
    mois: Vec<Mois>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Filtre {
    periode: String,
    du: String,
    au: String,
}

/// Chiffre d'affaires HT de chaque mois entre deux dates (notes de crédit déduites).
async fn par_mois(db: &mut PgConnection, debut: NaiveDate, fin: NaiveDate) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT to_char(m, 'MM/YYYY'),
                COALESCE((SELECT sum(CASE WHEN f.genre = 'note_credit' THEN -f.montant_ht_cents ELSE f.montant_ht_cents END)
                          FROM factures f WHERE f.numero IS NOT NULL
                            AND f.date_facture BETWEEN GREATEST(m::date, $1) AND LEAST((m + interval '1 month - 1 day')::date, $2)), 0)::bigint
         FROM generate_series(date_trunc('month', $1::date), date_trunc('month', $2::date), interval '1 month') m
         ORDER BY m",
    )
    .bind(debut)
    .bind(fin)
    .fetch_all(&mut *db)
    .await
}

async fn afficher(mut db: Base, Query(f): Query<Filtre>) -> Result<Html<String>, Erreur> {
        let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await?;
    let (code, debut, fin) = periode(&f.periode, &f.du, &f.au, aujourdhui);

    let (nombre_factures, ca_ht, ca_ttc): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE genre <> 'note_credit'),
                COALESCE(sum(CASE WHEN genre = 'note_credit' THEN -montant_ht_cents ELSE montant_ht_cents END), 0)::bigint,
                COALESCE(sum(CASE WHEN genre = 'note_credit' THEN -montant_ttc_cents ELSE montant_ttc_cents END), 0)::bigint
         FROM factures WHERE numero IS NOT NULL AND date_facture BETWEEN $1 AND $2",
    )
    .bind(debut)
    .bind(fin)
    .fetch_one(&mut *db)
    .await?;
    let encaisse: i64 = sqlx::query_scalar("SELECT COALESCE(sum(montant_cents), 0)::bigint FROM paiements WHERE date_paiement BETWEEN $1 AND $2")
        .bind(debut)
        .bind(fin)
        .fetch_one(&mut *db)
        .await?;
    // Les impayés : la situation d'aujourd'hui, quelle que soit la période.
    let (nombre_a_payer, a_payer, nombre_en_retard, en_retard): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE etat <> 'en_retard'), COALESCE(sum(reste_cents) FILTER (WHERE etat <> 'en_retard'), 0)::bigint,
                count(*) FILTER (WHERE etat = 'en_retard'), COALESCE(sum(reste_cents) FILTER (WHERE etat = 'en_retard'), 0)::bigint
         FROM factures_etats WHERE reste_cents > 0",
    )
    .fetch_one(&mut *db)
    .await?;
    let (emis, emis_montant, en_cours, en_cours_montant, acceptes, acceptes_montant, refuses): (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(montant_ht_cents), 0)::bigint,
                count(*) FILTER (WHERE statut IN ('a_envoyer', 'envoi_en_cours', 'envoye', 'echec_envoi', 'signe')),
                COALESCE(sum(montant_ht_cents) FILTER (WHERE statut IN ('a_envoyer', 'envoi_en_cours', 'envoye', 'echec_envoi', 'signe')), 0)::bigint,
                count(*) FILTER (WHERE statut = 'accepte'), COALESCE(sum(montant_ht_cents) FILTER (WHERE statut = 'accepte'), 0)::bigint,
                count(*) FILTER (WHERE statut = 'refuse')
         FROM devis WHERE chantier_id IS NULL AND statut <> 'brouillon' AND date_devis BETWEEN $1 AND $2",
    )
    .bind(debut)
    .bind(fin)
    .fetch_one(&mut *db)
    .await?;

    let mois = par_mois(&mut *db, debut, fin).await?;
    let plus_grand = mois.iter().map(|(_, m)| m.abs()).max().unwrap_or(0);
    let page = Page {
        periodes: PERIODES.iter().map(|(v, l)| Choix { valeur: (*v).into(), libelle: (*l).into(), choisi: *v == code }).collect(),
        du: debut.to_string(),
        au: fin.to_string(),
        periode_texte: format!("du {} au {}", debut.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
        ca_ht: format_euros(ca_ht),
        ca_ttc: format_euros(ca_ttc),
        nombre_factures,
        encaisse: format_euros(encaisse),
        a_payer: format_euros(a_payer),
        nombre_a_payer,
        en_retard: format_euros(en_retard),
        nombre_en_retard,
        devis_emis: emis,
        devis_emis_montant: format_euros(emis_montant),
        devis_en_cours: en_cours,
        devis_en_cours_montant: format_euros(en_cours_montant),
        devis_acceptes: acceptes,
        devis_acceptes_montant: format_euros(acceptes_montant),
        devis_refuses: refuses,
        taux_conversion: part(acceptes, emis),
        taux_perte: part(refuses, emis),
        panier_moyen: if acceptes > 0 { format_euros(acceptes_montant / acceptes) } else { "—".to_string() },
        mois: mois
            .into_iter()
            .map(|(libelle, m)| Mois {
                hauteur: if plus_grand > 0 { (m.abs() * 100 / plus_grand).max(if m != 0 { 2 } else { 0 }) } else { 0 },
                negatif: m < 0,
                montant: format_euros(m),
                libelle,
            })
            .collect(),
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/statistiques", get(afficher))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    #[test]
    fn les_periodes() {
        let auj = jour(2026, 10, 16);
        assert_eq!(periode("", "", "", auj), ("annee", jour(2026, 1, 1), auj));
        assert_eq!(periode("mois", "", "", auj), ("mois", jour(2026, 10, 1), auj));
        assert_eq!(periode("douze_mois", "", "", auj), ("douze_mois", jour(2025, 11, 1), auj));
        assert_eq!(periode("douze_mois", "", "", jour(2026, 12, 5)), ("douze_mois", jour(2026, 1, 1), jour(2026, 12, 5)));
        assert_eq!(periode("annee_passee", "", "", auj), ("annee_passee", jour(2025, 1, 1), jour(2025, 12, 31)));
        assert_eq!(periode("perso", "2026-03-01", "2026-03-31", auj), ("perso", jour(2026, 3, 1), jour(2026, 3, 31)));
        // Dates à l'envers ou illisibles : cette année.
        assert_eq!(periode("perso", "2026-03-31", "2026-03-01", auj).0, "annee");
        assert_eq!(periode("perso", "x", "", auj).0, "annee");
    }

    #[test]
    fn les_parts() {
        assert_eq!(part(3, 7), "42,86 %");
        assert_eq!(part(1, 2), "50 %");
        assert_eq!(part(0, 5), "0 %");
        assert_eq!(part(1, 0), "—");
    }
}
