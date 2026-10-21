//! Données › Importer (refonte du design, étape 1) : une page qui rassemble les imports existants
//! (entreprises, contacts, bibliothèque, métrés Excel si le module est allumé). Elle ne lit rien dans la base.

use askama::Template;
use axum::{Router, response::Html, routing::get};

use crate::{AppState, Erreur};

#[derive(Template)]
#[template(path = "importer.html")]
struct Importer;

async fn importer() -> Result<Html<String>, Erreur> {
    Ok(Html(Importer.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/donnees/importer", get(importer))
}
