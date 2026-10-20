//! Éditeur de devis, onglet Visualisation : le vrai PDF du devis, tel que le client le recevra,
//! et la liste de ce qu'il reste à compléter avant l'envoi.

use askama::Template;
use axum::{
    Router,
    extract::Path,
    response::Html,
    routing::get,
};

use crate::base::Base;
use crate::devis_pdf::charger_donnees;
use crate::devis_statut::{Barre, charger_barre};
use crate::{AppState, Erreur};

#[derive(Template)]
#[template(path = "devis_apercu.html")]
struct Page {
    id: i64,
    statut: &'static str,
    classe_statut: &'static str,
    onglet: &'static str,
    barre: Barre,
    a_completer: Vec<String>,
    /// L'adresse de la page client en ligne (le client consulte et signe).
    lien_client: String,
}

async fn afficher(mut db: Base, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let donnees = charger_donnees(&mut *db, id).await?;
    let entete = crate::devis_lignes::charger_entete(&mut *db, id).await?;
    let page = Page {
        id,
        statut: entete.statut,
        classe_statut: entete.classe_statut,
        onglet: "visualisation",
        barre: charger_barre(&mut *db, id).await?,
        a_completer: donnees.a_completer(),
        lien_client: crate::page_client::lien(&mut *db, crate::emails::Piece::Devis, id).await?,
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/apercu", get(afficher))
}
