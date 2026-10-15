//! Éditeur de devis, onglet Visualisation : le vrai PDF du devis, tel que le client le recevra,
//! et la liste de ce qu'il reste à compléter avant l'envoi.

use askama::Template;
use axum::{
    Router,
    extract::{Path, State},
    response::Html,
    routing::get,
};

use crate::devis_pdf::charger_donnees;
use crate::devis_statut::{Barre, charger_barre};
use crate::{AppState, Erreur};

#[derive(Template)]
#[template(path = "devis_apercu.html")]
struct Page {
    id: i64,
    numero: String,
    statut: &'static str,
    classe_statut: &'static str,
    onglet: &'static str,
    barre: Barre,
    a_completer: Vec<String>,
}

async fn afficher(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let donnees = charger_donnees(&state.db, id).await?;
    let entete = crate::devis_lignes::charger_entete(&state.db, id).await?;
    let page = Page {
        id,
        numero: donnees.numero.clone(),
        statut: entete.statut,
        classe_statut: entete.classe_statut,
        onglet: "visualisation",
        barre: charger_barre(&state.db, id).await?,
        a_completer: donnees.a_completer(),
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/apercu", get(afficher))
}
