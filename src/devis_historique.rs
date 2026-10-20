//! Éditeur de devis, onglet Historique (étape 5c-3) : ce qui s'est passé sur ce devis, et quand.

use askama::Template;
use axum::{
    Router,
    extract::Path,
    response::Html,
    routing::get,
};
use sqlx::FromRow;

use crate::base::Base;
use crate::devis_lignes::charger_entete;
use crate::devis_statut::{Barre, charger_barre};
use crate::journal::Genre;
use crate::{AppState, Erreur};

#[derive(FromRow)]
struct EvenementBase {
    genre: String,
    detail: String,
    /// Déjà au format belge : « 15/10/2026 · 11:30 ».
    quand: String,
}

pub struct Evenement {
    pub quand: String,
    pub genre: &'static str,
    pub detail: String,
}

#[derive(Template)]
#[template(path = "devis_historique.html")]
struct Page {
    id: i64,
    statut: &'static str,
    classe_statut: &'static str,
    onglet: &'static str,
    barre: Barre,
    evenements: Vec<Evenement>,
    /// Le dossier de preuve de la signature en ligne, s'il y en a un.
    dossier: Option<crate::signature::Dossier>,
}

async fn afficher(mut db: Base, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    let evenements: Vec<EvenementBase> = sqlx::query_as(
        "SELECT genre, detail, to_char(created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI') AS quand
         FROM devis_evenements WHERE devis_id = $1 ORDER BY created_at DESC, id DESC",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?;
    let page = Page {
        id,
        statut: entete.statut,
        classe_statut: entete.classe_statut,
        onglet: "historique",
        barre: charger_barre(&mut *db, id).await?,
        dossier: crate::signature::dossier(&mut *db, id).await?,
        evenements: evenements
            .into_iter()
            .map(|e| Evenement { quand: e.quand, genre: Genre::libelle(&e.genre), detail: e.detail })
            .collect(),
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/historique", get(afficher))
}
