pub mod avancement_calcul;
pub mod administration;
pub mod bibliotheque;
pub mod bibliotheque_import;
pub mod avancements;
pub mod base;
pub mod avenants;
pub mod chantiers;
pub mod commun;
pub mod composants;
pub mod comptes;
pub mod connexion;
pub mod contacts_import;
pub mod db;
pub mod demarrage;
pub mod calcul;
pub mod catalogue;
pub mod devis;
pub mod donnees;
pub mod devis_apercu;
pub mod devis_copie;
pub mod devis_editeur;
pub mod devis_historique;
pub mod devis_lignes;
pub mod devis_metre;
pub mod devis_pdf;
pub mod devis_statut;
pub mod devis_totaux;
pub mod emails;
pub mod emails_systeme;
pub mod envoi;
pub mod equipe;
pub mod exports;
pub mod facture_calcul;
pub mod factures;
pub mod factures_chantier;
pub mod factures_recues;
pub mod facture_pdf;
pub mod fiches;
pub mod import_devis;
pub mod import_excel;
pub mod journal;
pub mod metier;
pub mod organisations;
pub mod page_client;
pub mod paiements;
pub mod paiements_fournisseurs;
pub mod profil;
pub mod pdf;
pub mod peppol;
pub mod rappels;
pub mod recurrences;
pub mod archive;
pub mod mentions_tva;
pub mod reprise;
pub mod retard;
pub mod signature;
pub mod reprise_devis;
pub mod societe;
pub mod statistiques;
pub mod suivi_chantier;
pub mod tableau_de_bord;
pub mod ubl;

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

    /// Une erreur du serveur, sans détail pour l'utilisateur (le détail est déjà écrit dans la console).
    pub fn interne() -> Self {
        Erreur { code: StatusCode::INTERNAL_SERVER_ERROR, message: "Une erreur est survenue." }
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

// Fragment HTML renvoyé à htmx quand on clique sur « Tester le serveur ».
async fn sante(State(state): State<AppState>) -> Result<Html<&'static str>, Erreur> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Html("Le serveur et la base répondent."))
}

/// Toutes les pages du site, sans la connexion ni l'organisation (assemblées par `app` et `application`).
fn pages() -> Router<AppState> {
    Router::new()
        .route("/", get(tableau_de_bord::afficher))
        .route("/sante", get(sante))
        .merge(devis::routes())
        .merge(devis_editeur::routes())
        .merge(devis_lignes::routes())
        .merge(devis_metre::routes())
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
        .merge(factures_chantier::routes())
        .merge(facture_pdf::routes())
        .merge(archive::routes())
        .merge(signature::routes())
        .merge(paiements::routes())
        .merge(envoi::routes())
        .merge(rappels::routes())
        .merge(recurrences::routes())
        .merge(exports::routes())
        .merge(donnees::routes())
        .merge(composants::routes())
        .merge(page_client::routes())
        .merge(comptes::routes())
        .merge(demarrage::routes())
        .merge(equipe::routes())
        .merge(metier::routes())
        .merge(peppol::routes())
        .merge(factures_recues::routes())
        .merge(paiements_fournisseurs::routes())
        .merge(catalogue::routes())
        .merge(suivi_chantier::routes())
        .merge(bibliotheque::routes())
        .merge(bibliotheque_import::routes())
        .merge(statistiques::routes())
        .merge(import_devis::routes())
        .merge(contacts_import::routes())
        .merge(fiches::routes())
        .merge(societe::routes())
}

/// Le site sans connexion, **pour les tests des écrans** : toutes les pages travaillent dans l'organisation n° 1.
/// Jamais servi par `main.rs`.
pub fn app(state: AppState) -> Router {
    pages()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            |State(state): State<AppState>, mut requete: axum::extract::Request, suite: axum::middleware::Next| async move {
                requete.extensions_mut().insert(base::Organisation(1));
                let profil = match state.db.acquire().await {
                    Ok(mut conn) => profil::charger(&mut conn, 1).await.unwrap_or(profil::Profil::BATIMENT),
                    Err(_) => profil::Profil::BATIMENT,
                };
                profil::avec(profil, suite.run(requete)).await
            },
        ))
        .with_state(state)
}

/// Le site tel qu'il est servi (`main.rs`) : toutes les pages demandent d'être connecté, sauf la page de connexion,
/// la page client en ligne et `/sante` (voir `connexion`). L'organisation de chaque page est celle de la session.
pub fn application(state: AppState) -> Router {
    pages()
        .merge(connexion::routes())
        // Le back office de Raven (étape 4-3) : sa propre connexion (`administration::Admin`), aucune organisation.
        .merge(administration::routes())
        .layer(axum::middleware::from_fn_with_state(state.clone(), connexion::exiger_connexion))
        .with_state(state)
}
