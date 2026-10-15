//! Devis : liste avec recherche et filtres, création d'un brouillon.
//! (L'éditeur, les lignes, les totaux et le PDF viennent aux étapes suivantes.)

use askama::Template;
use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    response::{Html, Redirect},
    routing::get,
};
use axum_extra::extract::Query;
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgPool};

use crate::commun::{Choix, echapper_like, encoder_url, format_euros};
use crate::{AppState, Erreur};

const PAR_PAGE: i64 = 20;

/// Valeur en base, libellé affiché, couleur du badge.
const STATUTS: [(&str, &str, &str); 8] = [
    ("brouillon", "Brouillon", "badge-ghost"),
    ("a_envoyer", "À envoyer", "badge-warning"),
    ("envoi_en_cours", "Envoi en cours", "badge-warning badge-outline"),
    ("envoye", "Envoyé", "badge-info"),
    ("echec_envoi", "Échec envoi", "badge-error"),
    ("signe", "Signé", "badge-secondary"),
    ("accepte", "Accepté", "badge-success"),
    ("refuse", "Refusé", "badge-error badge-outline"),
];

pub(crate) fn statut(valeur: &str) -> (&'static str, &'static str) {
    STATUTS
        .iter()
        .find(|(v, _, _)| *v == valeur)
        .map(|(_, l, c)| (*l, *c))
        .unwrap_or(("Inconnu", "badge-ghost"))
}

// ─── Création ───────────────────────────────────────────────────────────────

/// Crée un devis brouillon avec le prochain numéro. Le compteur et le devis sont dans la même
/// transaction : pas de doublon si deux personnes créent en même temps, pas de trou si ça échoue.
async fn creer_brouillon(db: &PgPool) -> Result<i64, sqlx::Error> {
    let mut tx = db.begin().await?;
    let numero: i64 = sqlx::query_scalar("UPDATE compteurs SET valeur = valeur + 1 WHERE nom = 'devis' RETURNING valeur")
        .fetch_one(&mut *tx)
        .await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES ($1) RETURNING id")
        .bind(numero)
        .fetch_one(&mut *tx)
        .await?;
    crate::journal::noter(&mut tx, id, crate::journal::Genre::Creation, "Devis créé").await?;
    tx.commit().await?;
    Ok(id)
}

async fn nouveau(State(state): State<AppState>) -> Result<Redirect, Erreur> {
    let id = creer_brouillon(&state.db).await?;
    // On arrive directement dans l'éditeur du nouveau brouillon.
    Ok(Redirect::to(&format!("/devis/{id}")))
}

// ─── Liste ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Recherche {
    q: String,
    statut: Vec<String>,
    du: String,
    au: String,
    page: String,
}

#[derive(FromRow)]
struct LigneBase {
    id: i64,
    annee: i32,
    numero: i64,
    date_devis: NaiveDate,
    client: Option<String>,
    reference: String,
    montant_ht_cents: i64,
    statut: String,
    statut_date: String,
}

pub struct Ligne {
    pub id: i64,
    pub numero: String,
    pub date: String,
    pub client: String,
    pub reference: String,
    pub montant_ht: String,
    pub statut: &'static str,
    pub classe_statut: &'static str,
    pub date_statut: String,
}

impl From<LigneBase> for Ligne {
    fn from(l: LigneBase) -> Self {
        let (libelle, classe) = statut(&l.statut);
        Ligne {
            id: l.id,
            numero: format!("{}-{}", l.annee, l.numero),
            date: l.date_devis.format("%d/%m/%Y").to_string(),
            client: l.client.unwrap_or_default(),
            reference: l.reference,
            montant_ht: format_euros(l.montant_ht_cents),
            statut: libelle,
            classe_statut: classe,
            // Un brouillon n'a pas de date de statut à montrer.
            date_statut: if l.statut == "brouillon" { String::new() } else { l.statut_date },
        }
    }
}

pub struct Liste {
    pub lignes: Vec<Ligne>,
    pub q: String,
    pub statuts: Vec<Choix>,
    pub du: String,
    pub au: String,
    /// Les filtres sous forme d'adresse (« q=…&statut=… »), pour les liens de pagination.
    pub requete: String,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
    pub total_ht: String,
    pub total_ttc: String,
}

#[derive(Template)]
#[template(path = "devis_liste.html")]
struct PageListe {
    liste: Liste,
}

/// Seulement le tableau : c'est ce que htmx remplace quand on tape ou qu'on filtre.
#[derive(Template)]
#[template(path = "_devis_tableau.html")]
struct TableauListe {
    liste: Liste,
}

fn date_du_formulaire(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// Jointures et conditions communes au comptage, aux totaux et à la liste.
const DEPUIS: &str = "FROM devis d
    LEFT JOIN entreprises e ON e.id = d.client_entreprise_id
    LEFT JOIN contacts c ON c.id = d.client_contact_id
    WHERE ($1 = ''
           OR d.numero::text ILIKE $2
           OR (extract(year FROM d.date_devis)::int || '-' || d.numero) ILIKE $2
           OR d.reference ILIKE $2
           OR COALESCE(e.nom, concat_ws(' ', c.prenom, c.nom)) ILIKE $2)
      AND (cardinality($3::text[]) = 0 OR d.statut::text = ANY($3))
      AND ($4::date IS NULL OR d.date_devis >= $4)
      AND ($5::date IS NULL OR d.date_devis <= $5)";

async fn liste(State(state): State<AppState>, Query(r): Query<Recherche>, entetes: HeaderMap) -> Result<Html<String>, Erreur> {
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    // Un statut inconnu est ignoré au lieu de provoquer une erreur.
    let statuts_choisis: Vec<String> =
        STATUTS.iter().filter(|(v, _, _)| r.statut.iter().any(|s| s == v)).map(|(v, _, _)| (*v).to_string()).collect();
    let du = date_du_formulaire(&r.du);
    let au = date_du_formulaire(&r.au);

    let (total, total_ht, total_ttc): (i64, i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*), COALESCE(sum(d.montant_ht_cents), 0)::bigint, COALESCE(sum(d.montant_ttc_cents), 0)::bigint {DEPUIS}"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(&statuts_choisis)
    .bind(du)
    .bind(au)
    .fetch_one(&state.db)
    .await?;

    let pages = ((total + PAR_PAGE - 1) / PAR_PAGE).max(1);
    let page: i64 = r.page.trim().parse().unwrap_or(1).clamp(1, pages);

    let lignes: Vec<LigneBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT d.id, extract(year FROM d.date_devis)::int AS annee, d.numero, d.date_devis,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), '')) AS client,
                d.reference, d.montant_ht_cents, d.statut::text AS statut,
                to_char(d.statut_date AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY') AS statut_date
         {DEPUIS} ORDER BY d.date_devis DESC, d.numero DESC LIMIT $6 OFFSET $7"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(&statuts_choisis)
    .bind(du)
    .bind(au)
    .bind(PAR_PAGE)
    .bind((page - 1) * PAR_PAGE)
    .fetch_all(&state.db)
    .await?;

    let mut morceaux = vec![format!("q={}", encoder_url(&q))];
    morceaux.extend(statuts_choisis.iter().map(|s| format!("statut={s}")));
    morceaux.push(format!("du={}", du.map(|d| d.to_string()).unwrap_or_default()));
    morceaux.push(format!("au={}", au.map(|d| d.to_string()).unwrap_or_default()));

    let liste = Liste {
        lignes: lignes.into_iter().map(Ligne::from).collect(),
        q,
        statuts: STATUTS
            .iter()
            .map(|(v, l, _)| Choix {
                valeur: (*v).to_string(),
                libelle: (*l).to_string(),
                choisi: statuts_choisis.iter().any(|s| s == v),
            })
            .collect(),
        du: du.map(|d| d.to_string()).unwrap_or_default(),
        au: au.map(|d| d.to_string()).unwrap_or_default(),
        requete: morceaux.join("&"),
        page,
        pages,
        total,
        total_ht: format_euros(total_ht),
        total_ttc: format_euros(total_ttc),
    };

    // htmx demande uniquement le tableau ; un rechargement de la page demande tout.
    let partiel = entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request");
    let html = if partiel { TableauListe { liste }.render()? } else { PageListe { liste }.render()? };
    Ok(Html(html))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis", get(liste).post(nouveau))
}
