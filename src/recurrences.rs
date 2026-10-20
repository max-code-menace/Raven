//! Factures récurrentes (inventaire de Vertuoza, 17/10/2026) : Ubique facture chaque mois la location de deux lots.
//!
//! Comme dans Vertuoza, la récurrence se crée depuis une facture (« Créer/voir la récurrence ») : date de début,
//! date de fin, type (journalière, hebdomadaire, mensuelle, annuelle), « tous les N », et « créer la facture le jour
//! même ou N jours avant la date prévue ». Le nombre d'occurrences trouvées s'affiche pendant la saisie.
//!
//! Semi-automatique, comme les rappels : rien n'est créé ni envoyé sans clic. Quand une échéance arrive, l'écran
//! « Factures récurrentes » (et « À surveiller » sur le tableau de bord) la propose ; « Préparer les factures »
//! crée pour chacune un **brouillon** « Facture récurrente » qui recopie l'en-tête et les lignes de la facture modèle,
//! daté du jour prévu. On le relit, on le valide (il reçoit alors son numéro) et on l'envoie comme d'habitude.

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{Days, Months, NaiveDate};
use serde::Deserialize;
use sqlx::{Connection as _, FromRow, PgConnection};

use crate::base::Base;
use crate::commun::{Choix, format_euros};
use crate::factures;
use crate::{AppState, Erreur};

/// Au plus 10 ans de récurrence, et au plus 500 échéances.
const JOURS_MAX: i64 = 3660;
const OCCURRENCES_MAX: usize = 500;

/// Valeur en base, libellé du type (comme dans Vertuoza), unité de « tous les N ».
pub const FREQUENCES: [(&str, &str, &str); 4] =
    [("jour", "Journalière", "jours"), ("semaine", "Hebdomadaire", "semaines"), ("mois", "Mensuelle", "mois"), ("annee", "Annuelle", "ans")];

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// La date de la `k`-ième échéance (0 : la date de début). Un mois sans le jour voulu prend son dernier jour
/// (début le 31/01 → 28/02, 31/03…), toujours calculé depuis la date de début pour ne pas dériver.
fn echeance(debut: NaiveDate, frequence: &str, intervalle: u32, k: u32) -> Option<NaiveDate> {
    let n = intervalle.checked_mul(k)?;
    match frequence {
        "jour" => debut.checked_add_days(Days::new(u64::from(n))),
        "semaine" => debut.checked_add_days(Days::new(u64::from(n) * 7)),
        "mois" => debut.checked_add_months(Months::new(n)),
        "annee" => debut.checked_add_months(Months::new(n.checked_mul(12)?)),
        _ => None,
    }
}

/// Toutes les dates prévues entre la date de début et la date de fin (incluses).
pub fn occurrences(debut: NaiveDate, fin: NaiveDate, frequence: &str, intervalle: u32) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    if intervalle == 0 {
        return dates;
    }
    for k in 0.. {
        match echeance(debut, frequence, intervalle, k) {
            Some(d) if d <= fin && dates.len() < OCCURRENCES_MAX => dates.push(d),
            _ => break,
        }
    }
    dates
}

/// « Tous les mois », « Tous les 3 mois », « Toutes les semaines ».
pub fn rythme(frequence: &str, intervalle: i32) -> String {
    let unite = FREQUENCES.iter().find(|(v, _, _)| *v == frequence).map(|(_, _, u)| *u).unwrap_or("mois");
    match (frequence, intervalle) {
        ("semaine", 1) => "Toutes les semaines".to_string(),
        (_, 1) => format!("Tous les {unite}"),
        (_, n) => format!("Tous les {n} {unite}"),
    }
}

// ─── En base ────────────────────────────────────────────────────────────────

#[derive(FromRow, Clone)]
struct Recurrence {
    id: i64,
    modele_id: i64,
    date_debut: NaiveDate,
    date_fin: NaiveDate,
    frequence: String,
    intervalle: i32,
    jours_avance: i32,
    preparee_jusqua: Option<NaiveDate>,
}

impl Recurrence {
    fn dates(&self) -> Vec<NaiveDate> {
        occurrences(self.date_debut, self.date_fin, &self.frequence, self.intervalle.max(1) as u32)
    }

    /// Les échéances pas encore préparées : (date prévue, à préparer aujourd'hui).
    fn a_venir(&self, aujourdhui: NaiveDate) -> Vec<(NaiveDate, bool)> {
        self.dates()
            .into_iter()
            .filter(|d| self.preparee_jusqua.is_none_or(|p| *d > p))
            .map(|d| (d, d.checked_sub_days(Days::new(self.jours_avance.max(0) as u64)).is_some_and(|x| x <= aujourdhui)))
            .collect()
    }
}

const COLONNES: &str = "id, modele_id, date_debut, date_fin, frequence, intervalle, jours_avance, preparee_jusqua";

async fn aujourdhui(db: &mut PgConnection) -> Result<NaiveDate, sqlx::Error> {
    sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await
}

async fn toutes(db: &mut PgConnection) -> Result<Vec<Recurrence>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLONNES} FROM recurrences ORDER BY id"))).fetch_all(&mut *db).await
}

/// Le nombre de factures récurrentes à préparer aujourd'hui (pour « À surveiller »).
pub async fn nombre_a_preparer(db: &mut PgConnection) -> Result<usize, sqlx::Error> {
    let jour = aujourdhui(&mut *db).await?;
    Ok(toutes(&mut *db).await?.iter().map(|r| r.a_venir(jour).iter().filter(|(_, due)| *due).count()).sum())
}

/// Crée le brouillon d'une échéance : l'en-tête et les lignes de la facture modèle, datés du jour prévu.
/// Rien si l'échéance a déjà été préparée.
async fn preparer_echeance(db: &mut PgConnection, r: &Recurrence, date: NaiveDate) -> Result<Option<i64>, sqlx::Error> {
    let mut tx = db.begin().await?;
    // Verrouille la récurrence : deux clics en même temps ne préparent pas deux fois la même échéance.
    let deja: Option<NaiveDate> =
        sqlx::query_scalar("SELECT preparee_jusqua FROM recurrences WHERE id = $1 FOR UPDATE").bind(r.id).fetch_one(&mut *tx).await?;
    if deja.is_some_and(|p| date <= p) {
        return Ok(None);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO factures (genre, date_facture, reference, responsable, client_entreprise_id, client_contact_id,
                               conditions_paiement_id, taux_tva_id, remarque, recurrence_id, date_prevue)
         SELECT 'recurrente', $2, reference, responsable, client_entreprise_id, client_contact_id,
                conditions_paiement_id, taux_tva_id, remarque, $3, $2
         FROM factures WHERE id = $1 RETURNING id",
    )
    .bind(r.modele_id)
    .bind(date)
    .bind(r.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO facture_lignes (facture_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5,
                                     taux_tva_id, taux_centiemes, total_cents)
         SELECT $1, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id, taux_centiemes, total_cents
         FROM facture_lignes WHERE facture_id = $2",
    )
    .bind(id)
    .bind(r.modele_id)
    .execute(&mut *tx)
    .await?;
    factures::recalculer(&mut tx, id).await?;
    sqlx::query("UPDATE recurrences SET preparee_jusqua = $2, updated_at = now() WHERE id = $1").bind(r.id).bind(date).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(id))
}

// ─── Écran « Factures récurrentes » ─────────────────────────────────────────

pub struct LigneVue {
    pub modele: i64,
    pub client: String,
    pub reference: String,
    pub montant_ttc: String,
    pub rythme: String,
    pub periode: String,
    pub prochaine: String,
    pub a_preparer: usize,
    pub creees: i64,
}

#[derive(Template)]
#[template(path = "recurrences.html")]
struct PageListe {
    lignes: Vec<LigneVue>,
    total_a_preparer: usize,
    message_ok: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RetourListe {
    preparees: String,
}

async fn liste(mut db: Base, Query(retour): Query<RetourListe>) -> Result<Html<String>, Erreur> {
    let jour = aujourdhui(&mut *db).await?;
    let mut lignes = Vec::new();
    for r in toutes(&mut *db).await? {
        let (entreprise, contact, reference, ttc): (Option<i64>, Option<i64>, String, i64) = sqlx::query_as(
            "SELECT client_entreprise_id, client_contact_id, reference, montant_ttc_cents FROM factures WHERE id = $1",
        )
        .bind(r.modele_id)
        .fetch_one(&mut *db)
        .await?;
        let creees: i64 = sqlx::query_scalar("SELECT count(*) FROM factures WHERE recurrence_id = $1").bind(r.id).fetch_one(&mut *db).await?;
        let a_venir = r.a_venir(jour);
        lignes.push(LigneVue {
            modele: r.modele_id,
            client: factures::nom_client(&mut *db, entreprise, contact).await?,
            reference,
            montant_ttc: format_euros(ttc),
            rythme: rythme(&r.frequence, r.intervalle),
            periode: format!("{} → {}", date_belge(r.date_debut), date_belge(r.date_fin)),
            prochaine: a_venir.first().map(|(d, _)| date_belge(*d)).unwrap_or_else(|| "terminée".to_string()),
            a_preparer: a_venir.iter().filter(|(_, due)| *due).count(),
            creees,
        });
    }
    let total_a_preparer = lignes.iter().map(|l| l.a_preparer).sum();
    let message_ok = match retour.preparees.parse::<usize>() {
        Ok(0) => "Rien à préparer aujourd'hui.".to_string(),
        Ok(n) => format!("{n} facture(s) récurrente(s) préparée(s) en brouillon : relis-les, valide-les puis envoie-les."),
        Err(_) => String::new(),
    };
    Ok(Html(PageListe { lignes, total_a_preparer, message_ok }.render()?))
}

/// « Préparer les factures » : un brouillon pour chaque échéance arrivée.
async fn preparer(mut db: Base) -> Result<Redirect, Erreur> {
    let jour = aujourdhui(&mut *db).await?;
    let mut nombre = 0;
    for r in toutes(&mut *db).await? {
        for (date, due) in r.a_venir(jour) {
            if due && preparer_echeance(&mut *db, &r, date).await?.is_some() {
                nombre += 1;
            }
        }
    }
    Ok(Redirect::to(&format!("/factures/recurrences?preparees={nombre}")))
}

// ─── « Créer/voir la récurrence » d'une facture ─────────────────────────────

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Saisie {
    date_debut: String,
    date_fin: String,
    frequence: String,
    intervalle: String,
    jours_avance: String,
}

struct Valeurs {
    debut: NaiveDate,
    fin: NaiveDate,
    frequence: &'static str,
    intervalle: i32,
    jours_avance: i32,
}

fn verifier(s: &Saisie) -> Result<Valeurs, Vec<String>> {
    let mut erreurs = Vec::new();
    let date = |t: &str| NaiveDate::parse_from_str(t.trim(), "%Y-%m-%d").ok();
    let (debut, fin) = (date(&s.date_debut), date(&s.date_fin));
    if debut.is_none() {
        erreurs.push("Indiquez la date de début.".to_string());
    }
    if fin.is_none() {
        erreurs.push("Indiquez la date de fin.".to_string());
    }
    if let (Some(d), Some(f)) = (debut, fin) {
        if f < d {
            erreurs.push("La date de fin doit suivre la date de début.".to_string());
        } else if (f - d).num_days() > JOURS_MAX {
            erreurs.push("Une récurrence dure au plus 10 ans.".to_string());
        }
    }
    let frequence = FREQUENCES.iter().map(|(v, _, _)| *v).find(|v| *v == s.frequence.trim());
    if frequence.is_none() {
        erreurs.push("Choisissez le type de récurrence.".to_string());
    }
    let intervalle = s.intervalle.trim().parse::<i32>().ok().filter(|n| (1..=365).contains(n));
    if intervalle.is_none() {
        erreurs.push("« Tous les » : un nombre entre 1 et 365.".to_string());
    }
    let jours_avance = match s.jours_avance.trim() {
        "" => Some(0),
        t => t.parse::<i32>().ok().filter(|n| (0..=60).contains(n)),
    };
    if jours_avance.is_none() {
        erreurs.push("« Créer la facture » : le jour même ou de 1 à 60 jours avant.".to_string());
    }
    match (erreurs.is_empty(), debut, fin, frequence, intervalle, jours_avance) {
        (true, Some(debut), Some(fin), Some(frequence), Some(intervalle), Some(jours_avance)) => {
            Ok(Valeurs { debut, fin, frequence, intervalle, jours_avance })
        }
        _ => Err(erreurs),
    }
}

/// Une facture déjà créée par la récurrence.
pub struct Creee {
    pub id: i64,
    pub prevue: String,
    pub numero: String,
    pub statut: &'static str,
    pub classe: &'static str,
}

#[derive(Template)]
#[template(path = "recurrence.html")]
struct PageRecurrence {
    id: i64,
    titre: String,
    client: String,
    montant_ttc: String,
    existe: bool,
    saisie: Saisie,
    frequences: Vec<Choix>,
    delais: Vec<Choix>,
    erreurs: Vec<String>,
    apercu: Apercu,
    creees: Vec<Creee>,
    message_ok: &'static str,
}

/// Le nombre d'occurrences et les premières dates (mis à jour pendant la saisie, htmx).
pub struct Apercu {
    pub nombre: usize,
    pub dates: Vec<String>,
    pub autres: usize,
    pub rythme: String,
}

#[derive(Template)]
#[template(path = "_recurrence_apercu.html")]
struct FragmentApercu {
    apercu: Apercu,
}

fn apercu(s: &Saisie) -> Apercu {
    match verifier(s) {
        Ok(v) => {
            let dates = occurrences(v.debut, v.fin, v.frequence, v.intervalle as u32);
            Apercu {
                nombre: dates.len(),
                autres: dates.len().saturating_sub(12),
                dates: dates.iter().take(12).map(|d| date_belge(*d)).collect(),
                rythme: rythme(v.frequence, v.intervalle),
            }
        }
        Err(_) => Apercu { nombre: 0, dates: Vec::new(), autres: 0, rythme: String::new() },
    }
}

/// La facture modèle, si elle peut porter une récurrence.
async fn modele(db: &mut PgConnection, id: i64) -> Result<factures::Facture, Erreur> {
    let f = factures::charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.peut_recurrer() {
        return Err(Erreur::conflit("Seule une facture simple ou récurrente peut servir de modèle à une récurrence."));
    }
    Ok(f)
}

async fn page(db: &mut PgConnection, f: &factures::Facture, saisie: Option<Saisie>, erreurs: Vec<String>, message_ok: &'static str) -> Result<Html<String>, Erreur> {
    let existante: Option<Recurrence> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLONNES} FROM recurrences WHERE modele_id = $1"))).bind(f.id).fetch_optional(&mut *db).await?;
    let saisie = match (saisie, &existante) {
        (Some(s), _) => s,
        (None, Some(r)) => Saisie {
            date_debut: r.date_debut.to_string(),
            date_fin: r.date_fin.to_string(),
            frequence: r.frequence.clone(),
            intervalle: r.intervalle.to_string(),
            jours_avance: r.jours_avance.to_string(),
        },
        // Proposition : chaque mois pendant un an, à partir du mois qui suit la facture modèle.
        (None, None) => {
            let debut = f.date_facture.checked_add_months(Months::new(1)).unwrap_or(f.date_facture);
            let fin = debut.checked_add_months(Months::new(11)).unwrap_or(debut);
            Saisie {
                date_debut: debut.to_string(),
                date_fin: fin.to_string(),
                frequence: "mois".to_string(),
                intervalle: "1".to_string(),
                jours_avance: "0".to_string(),
            }
        }
    };
    let mut creees = Vec::new();
    if let Some(r) = &existante {
        let lignes: Vec<(i64, Option<i64>, String, NaiveDate)> = sqlx::query_as(
            "SELECT f.id, f.numero, et.etat, COALESCE(f.date_prevue, f.date_facture) FROM factures f
             JOIN factures_etats et ON et.facture_id = f.id WHERE f.recurrence_id = $1 ORDER BY f.date_prevue, f.id",
        )
        .bind(r.id)
        .fetch_all(&mut *db)
        .await?;
        for (id, numero, etat, prevue) in lignes {
            let (statut, classe) = factures::statut(&etat);
            creees.push(Creee { id, prevue: date_belge(prevue), numero: numero.map(|n| n.to_string()).unwrap_or_else(|| "—".into()), statut, classe });
        }
    }
    let ttc: i64 = sqlx::query_scalar("SELECT montant_ttc_cents FROM factures WHERE id = $1").bind(f.id).fetch_one(&mut *db).await?;
    let delai_choisi = saisie.jours_avance.trim().to_string();
    let page = PageRecurrence {
        id: f.id,
        titre: factures::titre(&f.genre, f.numero, f.date_facture),
        client: factures::nom_client(&mut *db, f.client_entreprise_id, f.client_contact_id).await?,
        montant_ttc: format_euros(ttc),
        existe: existante.is_some(),
        frequences: FREQUENCES
            .iter()
            .map(|(v, l, _)| Choix { valeur: (*v).to_string(), libelle: (*l).to_string(), choisi: *v == saisie.frequence })
            .collect(),
        delais: (0..=30)
            .map(|n: i32| Choix {
                valeur: n.to_string(),
                libelle: if n == 0 { "le jour même".to_string() } else { format!("{n} jour(s) avant la date prévue") },
                choisi: n.to_string() == delai_choisi || (n == 0 && delai_choisi.is_empty()),
            })
            .collect(),
        apercu: apercu(&saisie),
        saisie,
        erreurs,
        creees,
        message_ok,
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RetourFiche {
    ok: String,
}

async fn afficher(mut db: Base, Path(id): Path<i64>, Query(r): Query<RetourFiche>) -> Result<Html<String>, Erreur> {
    let f = modele(&mut *db, id).await?;
    let message = match r.ok.as_str() {
        "enregistree" => "La récurrence est enregistrée. Les échéances arrivées sont proposées dans Finances › Factures récurrentes.",
        _ => "",
    };
    page(&mut *db, &f, None, Vec::new(), message).await
}

/// L'aperçu « N occurrence(s) trouvée(s) » pendant la saisie (fragment htmx).
async fn previsualiser(Query(s): Query<Saisie>) -> Result<Html<String>, Erreur> {
    Ok(Html(FragmentApercu { apercu: apercu(&s) }.render()?))
}

async fn enregistrer(mut db: Base, Path(id): Path<i64>, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let f = modele(&mut *db, id).await?;
    let v = match verifier(&s) {
        Ok(v) => v,
        Err(erreurs) => return Ok(page(&mut *db, &f, Some(s), erreurs, "").await?.into_response()),
    };
    sqlx::query(
        "INSERT INTO recurrences (modele_id, date_debut, date_fin, frequence, intervalle, jours_avance) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (modele_id) DO UPDATE SET date_debut = $2, date_fin = $3, frequence = $4, intervalle = $5, jours_avance = $6, updated_at = now()",
    )
    .bind(id)
    .bind(v.debut)
    .bind(v.fin)
    .bind(v.frequence)
    .bind(v.intervalle)
    .bind(v.jours_avance)
    .execute(&mut *db)
    .await?;
    Ok(Redirect::to(&format!("/factures/{id}/recurrence?ok=enregistree")).into_response())
}

/// Arrête la récurrence : plus rien n'est proposé ; les factures déjà créées restent.
async fn supprimer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    sqlx::query("DELETE FROM recurrences WHERE modele_id = $1").bind(id).execute(&mut *db).await?;
    Ok(Redirect::to("/factures/recurrences"))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/factures/recurrences", get(liste))
        .route("/factures/recurrences/preparer", post(preparer))
        .route("/factures/recurrences/apercu", get(previsualiser))
        .route("/factures/{id}/recurrence", get(afficher).post(enregistrer))
        .route("/factures/{id}/recurrence/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    #[test]
    fn les_echeances() {
        // Chaque mois pendant un an : 12 échéances ; un 31 devient le dernier jour du mois, sans dériver ensuite.
        let mois = occurrences(jour(2026, 1, 31), jour(2026, 12, 31), "mois", 1);
        assert_eq!(mois.len(), 12);
        assert_eq!((mois[1], mois[2]), (jour(2026, 2, 28), jour(2026, 3, 31)));
        assert_eq!(occurrences(jour(2026, 10, 1), jour(2027, 9, 30), "mois", 3).len(), 4);
        assert_eq!(occurrences(jour(2026, 10, 1), jour(2026, 10, 29), "semaine", 1).len(), 5);
        assert_eq!(occurrences(jour(2026, 10, 1), jour(2026, 10, 10), "jour", 2).len(), 5);
        assert_eq!(occurrences(jour(2026, 10, 1), jour(2030, 10, 1), "annee", 1).len(), 5);
        assert!(occurrences(jour(2026, 10, 1), jour(2026, 9, 1), "mois", 1).is_empty());
        assert!(occurrences(jour(2026, 10, 1), jour(2036, 10, 1), "jour", 1).len() <= OCCURRENCES_MAX);
    }

    #[test]
    fn le_rythme_en_mots() {
        assert_eq!(rythme("mois", 1), "Tous les mois");
        assert_eq!(rythme("mois", 3), "Tous les 3 mois");
        assert_eq!(rythme("semaine", 1), "Toutes les semaines");
        assert_eq!(rythme("annee", 2), "Tous les 2 ans");
    }

    #[test]
    fn a_preparer_selon_l_avance() {
        let r = Recurrence {
            id: 1,
            modele_id: 1,
            date_debut: jour(2026, 11, 1),
            date_fin: jour(2027, 1, 1),
            frequence: "mois".into(),
            intervalle: 1,
            jours_avance: 5,
            preparee_jusqua: None,
        };
        // Le 27/10, l'échéance du 01/11 est à préparer (5 jours avant) ; celle du 01/12 pas encore.
        let a = r.a_venir(jour(2026, 10, 27));
        assert_eq!(a, vec![(jour(2026, 11, 1), true), (jour(2026, 12, 1), false), (jour(2027, 1, 1), false)]);
        // Une échéance déjà préparée n'est plus proposée.
        let r = Recurrence { preparee_jusqua: Some(jour(2026, 11, 1)), ..r };
        assert_eq!(r.a_venir(jour(2026, 10, 27)).len(), 2);
    }

    #[test]
    fn la_saisie() {
        let s = Saisie {
            date_debut: "2026-11-01".into(),
            date_fin: "2027-10-31".into(),
            frequence: "mois".into(),
            intervalle: "1".into(),
            jours_avance: "".into(),
        };
        let v = verifier(&s).unwrap();
        assert_eq!((v.frequence, v.intervalle, v.jours_avance), ("mois", 1, 0));
        assert_eq!(apercu(&s).nombre, 12);
        assert!(verifier(&Saisie { date_fin: "2026-10-01".into(), ..s.clone() }).is_err());
        assert!(verifier(&Saisie { frequence: "siecle".into(), ..s.clone() }).is_err());
        assert!(verifier(&Saisie { intervalle: "0".into(), ..s.clone() }).is_err());
        assert!(verifier(&Saisie { jours_avance: "90".into(), ..s.clone() }).is_err());
        assert!(verifier(&Saisie { date_fin: "2040-01-01".into(), ..s }).is_err());
    }
}
