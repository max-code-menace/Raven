//! Chantiers (étape 6-1) : créés à l'acceptation d'un devis, liste filtrée sur « En cours », fiche.
//! La référence, le client, l'adresse et le montant viennent du devis accepté (il ne change plus).
//! Les avenants (6-2) sont dans l'onglet Devis/avenants, les avancements (6-3) dans l'onglet Finances.

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::HeaderMap,
    response::{Html, Redirect},
    routing::get,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, Postgres, Transaction};

use crate::commun::{Choix, echapper_like, encoder_url, format_euros};
use crate::{AppState, Erreur};

const PAR_PAGE: i64 = 20;

/// Valeur en base, libellé affiché, couleur du badge.
const STATUTS: [(&str, &str, &str); 2] = [("en_cours", "En cours", "s-chantier"), ("termine", "Terminé", "s-termine")];

fn statut(valeur: &str) -> (&'static str, &'static str) {
    STATUTS.iter().find(|(v, _, _)| *v == valeur).map(|(_, l, c)| (*l, *c)).unwrap_or(("Inconnu", "s-brouillon"))
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

// ─── Création (appelée par l'acceptation du devis) ──────────────────────────

/// Crée le chantier d'un devis qu'on vient d'accepter et renvoie son numéro.
/// Le compteur est incrémenté dans la transaction de l'acceptation : pas de trou, pas de doublon.
pub async fn creer(tx: &mut Transaction<'_, Postgres>, devis_id: i64, date_debut: NaiveDate) -> Result<i64, sqlx::Error> {
    let numero: i64 = sqlx::query_scalar("UPDATE compteurs SET valeur = valeur + 1 WHERE nom = 'chantier' RETURNING valeur")
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO chantiers (numero, devis_id, date_debut) VALUES ($1, $2, $3)")
        .bind(numero)
        .bind(devis_id)
        .bind(date_debut)
        .execute(&mut **tx)
        .await?;
    Ok(numero)
}

// ─── Ce que la liste et la fiche lisent ─────────────────────────────────────

/// Le chantier, son devis, le client et l'adresse du chantier (celle du client ou celle du devis).
const DEPUIS: &str = "FROM chantiers ch
    JOIN devis d ON d.id = ch.devis_id
    LEFT JOIN entreprises e ON e.id = d.client_entreprise_id
    LEFT JOIN contacts c ON c.id = d.client_contact_id
    CROSS JOIN LATERAL (SELECT
        CASE WHEN d.chantier_identique THEN COALESCE(e.rue, c.rue, '') ELSE d.chantier_rue END AS rue,
        CASE WHEN d.chantier_identique THEN COALESCE(e.numero, c.numero, '') ELSE d.chantier_numero END AS numero,
        CASE WHEN d.chantier_identique THEN COALESCE(e.code_postal, c.code_postal, '') ELSE d.chantier_code_postal END AS code_postal,
        CASE WHEN d.chantier_identique THEN COALESCE(e.ville, c.ville, '') ELSE d.chantier_ville END AS ville) a";

/// « Rue 12, 1000 Bruxelles », sans virgule de trop quand une partie manque.
const ADRESSE: &str = "concat_ws(', ', NULLIF(trim(concat_ws(' ', a.rue, a.numero)), ''),
                                   NULLIF(trim(concat_ws(' ', a.code_postal, a.ville)), ''))";

/// Total devis/avenants HT : le devis de base plus les avenants acceptés.
const TOTAL_HT: &str = "(d.montant_ht_cents + COALESCE((SELECT sum(av.montant_ht_cents) FROM devis av
                                                 WHERE av.chantier_id = ch.id AND av.statut = 'accepte'), 0))::bigint";

/// Facturé HT d'un chantier : ses pièces validées, notes de crédit déduites.
const FACTURE_HT: &str = "COALESCE((SELECT sum(CASE WHEN f.genre = 'note_credit' THEN -f.montant_ht_cents ELSE f.montant_ht_cents END)
                                    FROM factures f WHERE f.chantier_id = ch.id AND f.numero IS NOT NULL), 0)";

/// Reste à recevoir TTC des factures du chantier.
const A_RECEVOIR: &str = "COALESCE((SELECT sum(GREATEST(et.reste_cents, 0)) FROM factures f JOIN factures_etats et ON et.facture_id = f.id
                                    WHERE f.chantier_id = ch.id), 0)";

const CLIENT: &str ="COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '')";

// ─── Liste ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Recherche {
    q: String,
    /// « en_cours » (par défaut, comme dans Vertuoza), « termine » ou « tous ».
    statut: String,
    page: String,
}

#[derive(FromRow)]
struct LigneBase {
    id: i64,
    numero: i64,
    date_debut: NaiveDate,
    reference: String,
    adresse: String,
    client: String,
    montant_ht_cents: i64,
    statut: String,
}

pub struct Ligne {
    pub id: i64,
    pub numero: i64,
    pub debut: String,
    pub reference: String,
    pub adresse: String,
    pub client: String,
    pub montant_ht: String,
    pub statut: &'static str,
    pub classe_statut: &'static str,
}

pub struct Liste {
    pub lignes: Vec<Ligne>,
    pub q: String,
    pub statuts: Vec<Choix>,
    pub requete: String,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
    pub total_ht: String,
    pub reste_a_facturer: String,
    pub reste_a_recevoir: String,
}

#[derive(Template)]
#[template(path = "chantiers_liste.html")]
struct PageListe {
    liste: Liste,
}

#[derive(Template)]
#[template(path = "_chantiers_tableau.html")]
struct TableauListe {
    liste: Liste,
}

/// Le filtre de statut demandé ; tout ce qui n'est pas connu revient à « En cours ».
fn filtre_statut(demande: &str) -> &'static str {
    match demande {
        "termine" => "termine",
        "tous" => "tous",
        _ => "en_cours",
    }
}

const CONDITIONS: &str = "WHERE ($1 = ''
           OR ch.numero::text = $1
           OR d.reference ILIKE $2
           OR COALESCE(e.nom, concat_ws(' ', c.prenom, c.nom)) ILIKE $2
           OR concat_ws(' ', a.rue, a.code_postal, a.ville) ILIKE $2)
      AND ($3 = 'tous' OR ch.statut::text = $3)";

async fn liste(State(state): State<AppState>, Query(r): Query<Recherche>, entetes: HeaderMap) -> Result<Html<String>, Erreur> {
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    let filtre = filtre_statut(r.statut.trim());

    let (total, total_ht, reste_a_facturer, reste_a_recevoir): (i64, i64, i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*), COALESCE(sum({TOTAL_HT}), 0)::bigint, COALESCE(sum({TOTAL_HT} - {FACTURE_HT}), 0)::bigint,
                COALESCE(sum({A_RECEVOIR}), 0)::bigint
         {DEPUIS} {CONDITIONS}"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(filtre)
    .fetch_one(&state.db)
    .await?;

    let pages = ((total + PAR_PAGE - 1) / PAR_PAGE).max(1);
    let page: i64 = r.page.trim().parse().unwrap_or(1).clamp(1, pages);

    let lignes: Vec<LigneBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT ch.id, ch.numero, ch.date_debut, d.reference, {ADRESSE} AS adresse, {CLIENT} AS client,
                {TOTAL_HT} AS montant_ht_cents, ch.statut::text AS statut
         {DEPUIS} {CONDITIONS} ORDER BY ch.numero DESC LIMIT $4 OFFSET $5"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(filtre)
    .bind(PAR_PAGE)
    .bind((page - 1) * PAR_PAGE)
    .fetch_all(&state.db)
    .await?;

    let mut statuts: Vec<Choix> =
        STATUTS.iter().map(|(v, l, _)| Choix { valeur: (*v).to_string(), libelle: (*l).to_string(), choisi: *v == filtre }).collect();
    statuts.push(Choix { valeur: "tous".to_string(), libelle: "Tous".to_string(), choisi: filtre == "tous" });

    let liste = Liste {
        lignes: lignes
            .into_iter()
            .map(|l| {
                let (libelle, classe) = statut(&l.statut);
                Ligne {
                    id: l.id,
                    numero: l.numero,
                    debut: date_belge(l.date_debut),
                    reference: l.reference,
                    adresse: l.adresse,
                    client: l.client,
                    montant_ht: format_euros(l.montant_ht_cents),
                    statut: libelle,
                    classe_statut: classe,
                }
            })
            .collect(),
        requete: format!("q={}&statut={filtre}", encoder_url(&q)),
        q,
        statuts,
        page,
        pages,
        total,
        total_ht: format_euros(total_ht),
        reste_a_facturer: format_euros(reste_a_facturer),
        reste_a_recevoir: format_euros(reste_a_recevoir),
    };

    // htmx demande uniquement le tableau ; un rechargement de la page demande tout.
    let partiel = entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request");
    let html = if partiel { TableauListe { liste }.render()? } else { PageListe { liste }.render()? };
    Ok(Html(html))
}

// ─── Fiche ──────────────────────────────────────────────────────────────────

#[derive(FromRow)]
struct FicheBase {
    numero: i64,
    statut: String,
    date_debut: NaiveDate,
    devis_id: i64,
    devis_numero: String,
    reference: String,
    responsable: String,
    date_acceptation: Option<NaiveDate>,
    client: String,
    adresse: String,
    gestionnaire: String,
    architecte: String,
    montant_ht_cents: i64,
    montant_ttc_cents: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

impl Retour {
    /// Les messages sont fixes : rien de ce qui est dans l'adresse n'est affiché tel quel.
    fn message_ok(&self) -> &'static str {
        match self.ok.as_str() {
            "1" => "Le chantier est enregistré.",
            "achat" => "L'achat est ajouté.",
            "achat_retire" => "L'achat est retiré.",
            _ => "",
        }
    }

    fn message_erreur(&self) -> &'static str {
        match self.erreur.as_str() {
            "date" => "La date de début n'est pas valide.",
            "statut" => "Ce statut n'existe pas.",
            "finale" => "Ce chantier a déjà sa facture finale : plus d'avancement ni d'acompte possible.",
            "acompte" => "L'acompte doit être un pourcentage ou un montant HT positif, au plus le total devis/avenants.",
            "rien" => "Il n'y a rien à facturer.",
            "avancement_brouillon" => "Valide ou supprime d'abord l'avancement en brouillon.",
            "facture_brouillon" => "Valide ou supprime d'abord la facture en brouillon de ce chantier (un acompte en brouillon ne serait pas déduit).",
            "achat" => "L'achat n'est pas ajouté : date, catégorie et montant HT (différent de 0) sont obligatoires ; fournisseur et description 300 caractères au maximum.",
            _ => "",
        }
    }
}

#[derive(Template)]
#[template(path = "chantier.html")]
struct PageFiche {
    id: i64,
    onglet: &'static str,
    numero: i64,
    statut: &'static str,
    classe_statut: &'static str,
    statuts: Vec<Choix>,
    date_debut: String,
    /// La même date, au format du champ de formulaire (AAAA-MM-JJ).
    date_debut_champ: String,
    devis_id: i64,
    devis_numero: String,
    reference: String,
    responsable: String,
    date_acceptation: String,
    client: String,
    adresse: String,
    gestionnaire: String,
    architecte: String,
    devis_ht: String,
    devis_ttc: String,
    avenants: Vec<crate::avenants::Ligne>,
    /// Somme des avenants acceptés.
    avenants_ht: String,
    /// Jours ouvrables supplémentaires des avenants acceptés.
    jours_supplementaires: i64,
    /// Devis de base + avenants acceptés.
    total_ht: String,
    /// Les avancements du chantier (onglet Finances) et le reste à avancer.
    finances: crate::avancements::Finances,
    /// Les factures et notes de crédit du chantier.
    factures: crate::factures_chantier::Recap,
    /// Rentabilité, budget d'achat, main d'œuvre, facturation et achats (étape 9-2).
    suivi: crate::suivi_chantier::Suivi,
    message_ok: &'static str,
    message_erreur: &'static str,
}

async fn charger_fiche(state: &AppState, id: i64, onglet: &'static str, retour: Retour) -> Result<Html<String>, Erreur> {
    let fiche: Option<FicheBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT ch.numero, ch.statut::text AS statut, ch.date_debut, d.id AS devis_id,
                extract(year FROM d.date_devis)::int || '-' || d.numero AS devis_numero,
                d.reference, d.responsable, d.date_acceptation, {CLIENT} AS client, {ADRESSE} AS adresse,
                COALESCE((SELECT trim(concat_ws(' ', g.prenom, g.nom)) FROM contacts g WHERE g.id = d.gestionnaire_contact_id), '') AS gestionnaire,
                COALESCE((SELECT trim(concat_ws(' ', r.prenom, r.nom)) FROM contacts r WHERE r.id = d.architecte_contact_id), '') AS architecte,
                d.montant_ht_cents, d.montant_ttc_cents
         {DEPUIS} WHERE ch.id = $1"
    )))
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let f = fiche.ok_or_else(Erreur::introuvable)?;
    let (libelle, classe) = statut(&f.statut);
    let avenants = crate::avenants::recap(&state.db, id).await?;
    let finances = crate::avancements::finances(&state.db, id).await?;
    let factures = crate::factures_chantier::recap(&state.db, id, finances.total_cents).await?;
    let suivi = crate::suivi_chantier::charger(&state.db, id).await?;
    let page = PageFiche {
        id,
        onglet,
        numero: f.numero,
        statut: libelle,
        classe_statut: classe,
        statuts: STATUTS
            .iter()
            .map(|(v, l, _)| Choix { valeur: (*v).to_string(), libelle: (*l).to_string(), choisi: *v == f.statut })
            .collect(),
        date_debut: date_belge(f.date_debut),
        date_debut_champ: f.date_debut.to_string(),
        devis_id: f.devis_id,
        devis_numero: f.devis_numero,
        reference: f.reference,
        responsable: f.responsable,
        date_acceptation: f.date_acceptation.map(date_belge).unwrap_or_default(),
        client: f.client,
        adresse: f.adresse,
        gestionnaire: f.gestionnaire,
        architecte: f.architecte,
        devis_ht: format_euros(f.montant_ht_cents),
        devis_ttc: format_euros(f.montant_ttc_cents),
        total_ht: format_euros(f.montant_ht_cents + avenants.acceptes_ht_cents),
        avenants_ht: format_euros(avenants.acceptes_ht_cents),
        jours_supplementaires: avenants.jours_acceptes,
        avenants: avenants.lignes,
        finances,
        factures,
        suivi,
        message_ok: retour.message_ok(),
        message_erreur: retour.message_erreur(),
    };
    Ok(Html(page.render()?))
}

async fn tableau_de_bord(State(state): State<AppState>, Path(id): Path<i64>, Query(retour): Query<Retour>) -> Result<Html<String>, Erreur> {
    charger_fiche(&state, id, "tableau", retour).await
}

async fn devis_avenants(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    charger_fiche(&state, id, "devis", Retour::default()).await
}

async fn onglet_finances(State(state): State<AppState>, Path(id): Path<i64>, Query(retour): Query<Retour>) -> Result<Html<String>, Erreur> {
    charger_fiche(&state, id, "finances", retour).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Modification {
    statut: String,
    date_debut: String,
}

/// Change le statut et la date de début. Rien d'autre : le reste vient du devis accepté.
async fn enregistrer(State(state): State<AppState>, Path(id): Path<i64>, Form(m): Form<Modification>) -> Result<Redirect, Erreur> {
    let existe: Option<i64> = sqlx::query_scalar("SELECT id FROM chantiers WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    existe.ok_or_else(Erreur::introuvable)?;
    let Some(statut_choisi) = STATUTS.iter().map(|(v, _, _)| *v).find(|v| *v == m.statut.trim()) else {
        return Ok(Redirect::to(&format!("/chantiers/{id}?erreur=statut")));
    };
    let Ok(date) = NaiveDate::parse_from_str(m.date_debut.trim(), "%Y-%m-%d") else {
        return Ok(Redirect::to(&format!("/chantiers/{id}?erreur=date")));
    };
    sqlx::query("UPDATE chantiers SET statut = $1::text::statut_chantier, date_debut = $2, updated_at = now() WHERE id = $3")
        .bind(statut_choisi)
        .bind(date)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Redirect::to(&format!("/chantiers/{id}?ok=1")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chantiers", get(liste))
        .route("/chantiers/{id}", get(tableau_de_bord).post(enregistrer))
        .route("/chantiers/{id}/devis", get(devis_avenants))
        .route("/chantiers/{id}/finances", get(onglet_finances))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_filtre_par_defaut_est_en_cours() {
        assert_eq!(filtre_statut(""), "en_cours");
        assert_eq!(filtre_statut("n_importe_quoi"), "en_cours");
        assert_eq!(filtre_statut("termine"), "termine");
        assert_eq!(filtre_statut("tous"), "tous");
    }

    #[test]
    fn chaque_statut_a_un_libelle() {
        assert_eq!(statut("en_cours").0, "En cours");
        assert_eq!(statut("termine").0, "Terminé");
        assert_eq!(statut("autre").0, "Inconnu");
    }
}
