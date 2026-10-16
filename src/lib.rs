pub mod avancement_calcul;
pub mod avancements;
pub mod avenants;
pub mod chantiers;
pub mod commun;
pub mod db;
pub mod calcul;
pub mod devis;
pub mod devis_apercu;
pub mod devis_copie;
pub mod devis_editeur;
pub mod devis_historique;
pub mod devis_lignes;
pub mod devis_pdf;
pub mod devis_statut;
pub mod devis_totaux;
pub mod facture_calcul;
pub mod factures;
pub mod fiches;
pub mod journal;
pub mod pdf;
pub mod societe;

use askama::Template;
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use sqlx::PgPool;

/// Ce que tous les écrans partagent : la connexion à la base.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
}

/// Erreur renvoyée à l'utilisateur : un code HTTP et un message simple.
/// Le détail technique est écrit dans la console du serveur, jamais affiché.
pub struct Erreur {
    code: StatusCode,
    message: &'static str,
}

impl Erreur {
    pub fn introuvable() -> Self {
        Erreur { code: StatusCode::NOT_FOUND, message: "Page introuvable." }
    }

    /// Le PDF n'a pas pu être fabriqué. Le détail technique reste dans la console du serveur.
    pub fn pdf(detail: &str) -> Self {
        eprintln!("Erreur PDF : {detail}");
        Erreur { code: StatusCode::INTERNAL_SERVER_ERROR, message: "Le PDF n'a pas pu être généré." }
    }

    /// L'action demandée n'est plus possible dans l'état actuel (exemple : modifier un devis envoyé).
    pub fn conflit(message: &'static str) -> Self {
        Erreur { code: StatusCode::CONFLICT, message }
    }
}

impl IntoResponse for Erreur {
    fn into_response(self) -> Response {
        (self.code, Html(format!("<p>{}</p>", self.message))).into_response()
    }
}

impl From<sqlx::Error> for Erreur {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("Erreur base de données : {e}");
        Erreur { code: StatusCode::INTERNAL_SERVER_ERROR, message: "Une erreur est survenue." }
    }
}

impl From<askama::Error> for Erreur {
    fn from(e: askama::Error) -> Self {
        eprintln!("Erreur d'affichage : {e}");
        Erreur { code: StatusCode::INTERNAL_SERVER_ERROR, message: "Une erreur est survenue." }
    }
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate;

async fn home() -> Result<Html<String>, Erreur> {
    Ok(Html(HomeTemplate.render()?))
}

// Fragment HTML renvoyé à htmx quand on clique sur « Tester le serveur ».
async fn sante(State(state): State<AppState>) -> Result<Html<&'static str>, Erreur> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Html("Le serveur et la base répondent."))
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/sante", get(sante))
        .merge(devis::routes())
        .merge(devis_editeur::routes())
        .merge(devis_lignes::routes())
        .merge(devis_totaux::routes())
        .merge(devis_apercu::routes())
        .merge(devis_pdf::routes())
        .merge(devis_statut::routes())
        .merge(devis_copie::routes())
        .merge(devis_historique::routes())
        .merge(chantiers::routes())
        .merge(avenants::routes())
        .merge(avancements::routes())
        .merge(factures::routes())
        .merge(fiches::routes())
        .merge(societe::routes())
        .with_state(state)
}
