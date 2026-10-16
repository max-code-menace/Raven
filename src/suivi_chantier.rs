//! Suivi de chantier complet (étape 9-2), comme le Tableau de bord du chantier de Vertuoza :
//! - **Rentabilité** prévue (devis de base + avenants acceptés : ventes, achats, frais généraux, bénéfice) et actuelle
//!   (facturé HT − achats réels) ;
//! - **Budget d'achat** : achats prévus, dépensés (achats saisis), disponible ;
//! - **Main d'œuvre** : heures prévues dans les devis ;
//! - **Facturation** : facturé, encaissé, reste à recevoir, reste à facturer.
//!
//! Les achats réels se saisissent à la main dans l'onglet Finances (« Achats du chantier ») en attendant la réception
//! des factures fournisseurs par Peppol (phase 2).

use axum::{
    Router,
    extract::{Form, Path, State},
    response::Redirect,
    routing::post,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::PgPool;

use crate::calcul;
use crate::commun::{Choix, format_euros};
use crate::fiches::euros_vers_centimes;
use crate::{AppState, Erreur};

const LONGUEUR_MAX_TEXTE: usize = 300;

pub(crate) const CATEGORIES: [(&str, &str); 5] = [
    ("fourniture", "Fournitures"),
    ("sous_traitant", "Sous-traitants"),
    ("equipement", "Équipements"),
    ("main_oeuvre", "Main d'œuvre"),
    ("autre", "Autres"),
];

fn libelle_categorie(code: &str) -> &'static str {
    CATEGORIES.iter().find(|(c, _)| *c == code).map(|(_, l)| *l).unwrap_or("Autres")
}

pub struct LigneAchat {
    pub id: i64,
    pub date: String,
    pub categorie: &'static str,
    pub fournisseur: String,
    pub description: String,
    pub montant: String,
}

/// Les chiffres du suivi, déjà mis en forme.
pub struct Suivi {
    pub ventes_prevues: String,
    pub achats_prevus: String,
    pub frais_generaux: String,
    pub benefice_prevu: String,
    pub taux_prevu: String,
    pub facture_ht: String,
    pub achats_reels: String,
    pub benefice_actuel: String,
    pub taux_actuel: String,
    pub benefice_actuel_negatif: bool,
    /// Budget d'achat : disponible = prévu − dépensé (négatif : dépassé).
    pub disponible: String,
    pub depasse: bool,
    /// Largeur de la barre « dépensé », de 0 à 100.
    pub pourcentage_budget: i64,
    pub pourcentage_budget_texte: String,
    pub heures_prevues: String,
    pub lignes_sans_achat: usize,
    pub facture_ttc: String,
    pub encaisse: String,
    pub reste_a_recevoir: String,
    pub achats: Vec<LigneAchat>,
    pub categories: Vec<Choix>,
    pub aujourdhui: String,
}

/// Le bénéfice en pour cent du coût (comme le taux de marge des devis), « — » sans coût.
fn taux(benefice: i64, cout: i64) -> String {
    calcul::taux_de_marge(benefice, cout).map(|t| calcul::taux_en_texte(t as i32)).unwrap_or_else(|| "—".to_string())
}

/// Dépensé en pour cent du prévu, borné à 100 pour la barre.
pub(crate) fn part_du_budget(depense: i64, prevu: i64) -> i64 {
    if prevu <= 0 {
        return if depense > 0 { 100 } else { 0 };
    }
    (depense.max(0) * 100 / prevu).clamp(0, 100)
}

pub async fn charger(db: &PgPool, chantier_id: i64) -> Result<Suivi, sqlx::Error> {
    // Le devis de base et les avenants acceptés.
    let devis: Vec<i64> = sqlx::query_scalar(
        "SELECT d.id FROM devis d WHERE d.id = (SELECT devis_id FROM chantiers WHERE id = $1)
         UNION ALL SELECT a.id FROM devis a WHERE a.chantier_id = $1 AND a.statut = 'accepte'",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await?;
    let mut prevu = calcul::Rentable::default();
    let mut sans_achat = 0;
    for id in devis {
        let r = crate::devis_lignes::rentabilite_prevue(db, id).await?;
        prevu.ventes_cents += r.total.ventes_cents;
        prevu.achats_cents += r.total.achats_cents;
        prevu.frais_generaux_cents += r.total.frais_generaux_cents;
        prevu.marge_cents += r.total.marge_cents;
        prevu.heures_e5 += r.total.heures_e5;
        sans_achat += r.lignes_sans_achat;
    }

    let (facture_ht, facture_ttc, encaisse, reste): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(CASE WHEN f.genre = 'note_credit' THEN -f.montant_ht_cents ELSE f.montant_ht_cents END), 0)::bigint,
                COALESCE(sum(CASE WHEN f.genre = 'note_credit' THEN -f.montant_ttc_cents ELSE f.montant_ttc_cents END), 0)::bigint,
                COALESCE(sum(et.paye_cents), 0)::bigint,
                COALESCE(sum(GREATEST(et.reste_cents, 0)), 0)::bigint
         FROM factures f JOIN factures_etats et ON et.facture_id = f.id
         WHERE f.chantier_id = $1 AND f.numero IS NOT NULL",
    )
    .bind(chantier_id)
    .fetch_one(db)
    .await?;

    let lignes: Vec<(i64, NaiveDate, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, date_achat, categorie::text, fournisseur, description, montant_ht_cents
         FROM chantier_achats WHERE chantier_id = $1 ORDER BY date_achat, id",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await?;
    let achats_reels: i64 = lignes.iter().map(|l| l.5).sum();
    let benefice_actuel = facture_ht - achats_reels;
    let disponible = prevu.achats_cents - achats_reels;
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(db).await?;

    Ok(Suivi {
        ventes_prevues: format_euros(prevu.ventes_cents),
        achats_prevus: format_euros(prevu.achats_cents),
        frais_generaux: format_euros(prevu.frais_generaux_cents),
        benefice_prevu: format_euros(prevu.marge_cents),
        taux_prevu: taux(prevu.marge_cents, prevu.cout_cents()),
        facture_ht: format_euros(facture_ht),
        achats_reels: format_euros(achats_reels),
        benefice_actuel: format_euros(benefice_actuel),
        taux_actuel: taux(benefice_actuel, achats_reels),
        benefice_actuel_negatif: benefice_actuel < 0,
        disponible: format_euros(disponible),
        depasse: disponible < 0,
        pourcentage_budget: part_du_budget(achats_reels, prevu.achats_cents),
        pourcentage_budget_texte: if prevu.achats_cents > 0 {
            format!("{} %", achats_reels.max(0) * 100 / prevu.achats_cents)
        } else {
            "—".to_string()
        },
        heures_prevues: if prevu.heures_e5 == 0 { "—".to_string() } else { format!("{} h", calcul::ecrire_decimal_e5(prevu.heures_e5)) },
        lignes_sans_achat: sans_achat,
        facture_ttc: format_euros(facture_ttc),
        encaisse: format_euros(encaisse),
        reste_a_recevoir: format_euros(reste),
        achats: lignes
            .into_iter()
            .map(|(id, date, categorie, fournisseur, description, montant)| LigneAchat {
                id,
                date: date.format("%d/%m/%Y").to_string(),
                categorie: libelle_categorie(&categorie),
                fournisseur,
                description,
                montant: format_euros(montant),
            })
            .collect(),
        categories: CATEGORIES.iter().enumerate().map(|(i, (v, l))| Choix { valeur: (*v).into(), libelle: (*l).into(), choisi: i == 0 }).collect(),
        aujourdhui: aujourdhui.to_string(),
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NouvelAchat {
    date: String,
    categorie: String,
    fournisseur: String,
    description: String,
    montant: String,
}

/// Lit un achat saisi : (date, catégorie, fournisseur, description, montant HT). Un avoir s'écrit avec un « - ».
fn lire(a: &NouvelAchat) -> Option<(NaiveDate, &'static str, String, String, i64)> {
    let date = NaiveDate::parse_from_str(a.date.trim(), "%Y-%m-%d").ok()?;
    let categorie = CATEGORIES.iter().map(|(c, _)| *c).find(|c| *c == a.categorie.trim())?;
    let texte = a.montant.trim();
    let (signe, chiffres) = match texte.strip_prefix('-') {
        Some(reste) => (-1, reste),
        None => (1, texte),
    };
    let montant = signe * euros_vers_centimes(chiffres).filter(|m| *m > 0)?;
    let fournisseur = a.fournisseur.trim().to_string();
    let description = a.description.trim().to_string();
    (fournisseur.chars().count() <= LONGUEUR_MAX_TEXTE && description.chars().count() <= LONGUEUR_MAX_TEXTE)
        .then_some((date, categorie, fournisseur, description, montant))
}

async fn ajouter(State(state): State<AppState>, Path(chantier_id): Path<i64>, Form(a): Form<NouvelAchat>) -> Result<Redirect, Erreur> {
    let existe: Option<i64> = sqlx::query_scalar("SELECT id FROM chantiers WHERE id = $1").bind(chantier_id).fetch_optional(&state.db).await?;
    existe.ok_or_else(Erreur::introuvable)?;
    let Some((date, categorie, fournisseur, description, montant)) = lire(&a) else {
        return Ok(Redirect::to(&format!("/chantiers/{chantier_id}/finances?erreur=achat#achats")));
    };
    sqlx::query(
        "INSERT INTO chantier_achats (chantier_id, date_achat, categorie, fournisseur, description, montant_ht_cents)
         VALUES ($1, $2, $3::text::categorie_achat, $4, $5, $6)",
    )
    .bind(chantier_id)
    .bind(date)
    .bind(categorie)
    .bind(&fournisseur)
    .bind(&description)
    .bind(montant)
    .execute(&state.db)
    .await?;
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/finances?ok=achat#achats")))
}

async fn supprimer(State(state): State<AppState>, Path((chantier_id, achat)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let fait = sqlx::query("DELETE FROM chantier_achats WHERE id = $1 AND chantier_id = $2").bind(achat).bind(chantier_id).execute(&state.db).await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/finances?ok=achat_retire#achats")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chantiers/{id}/achats", post(ajouter))
        .route("/chantiers/{id}/achats/{achat}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn achat(date: &str, categorie: &str, montant: &str) -> NouvelAchat {
        NouvelAchat { date: date.into(), categorie: categorie.into(), fournisseur: "Brico".into(), description: "Panneaux".into(), montant: montant.into() }
    }

    #[test]
    fn un_achat_saisi() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(lire(&achat("2026-10-01", "fourniture", "1 250,50")), Some((d, "fourniture", "Brico".into(), "Panneaux".into(), 125_050)));
        assert_eq!(lire(&achat("2026-10-01", "fourniture", "-50")).map(|a| a.4), Some(-5_000), "un avoir du fournisseur");
        assert_eq!(lire(&achat("2026-10-01", "fourniture", "0")), None);
        assert_eq!(lire(&achat("2026-10-01", "inconnue", "10")), None);
        assert_eq!(lire(&achat("01/10/2026", "fourniture", "10")), None);
    }

    #[test]
    fn la_barre_du_budget() {
        assert_eq!(part_du_budget(500, 1_000), 50);
        assert_eq!(part_du_budget(1_500, 1_000), 100);
        assert_eq!(part_du_budget(0, 0), 0);
        assert_eq!(part_du_budget(10, 0), 100);
        assert_eq!(part_du_budget(-10, 1_000), 0);
    }
}
