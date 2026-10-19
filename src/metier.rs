//! Réglages › Métier et modules (étape 3) : le métier de l'organisation, son vocabulaire (« Chantier » ou « Projet »)
//! et ses modules. Un propriétaire les change ; un membre les voit.
//!
//! Éteindre un module le retire des écrans (et ses adresses répondent « module éteint ») ; les données déjà saisies
//! restent et réapparaissent si on le rallume. L'assistant de démarrage règle tout selon le métier choisi (bâtiment :
//! « Chantier » et tous les modules ; autres métiers : « Projet », modules éteints).

use askama::Template;
use axum::{
    Router,
    extract::{Form, Query},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;

use crate::base::{Base, Utilisateur};
use crate::demarrage::PROFILS;
use crate::{AppState, Erreur};

/// Les modules : (champ du formulaire et colonne, nom, explication).
pub const MODULES: [(&str, &str, &str); 4] = [
    ("module_avancements", "Avancements", "Facturer au fur et à mesure (pourcentage réalisé par poste), puis le solde."),
    ("module_avenants", "Avenants", "Corriger un devis accepté (travaux en plus ou en moins)."),
    ("module_achats", "Achats et rentabilité", "Achats saisis par chantier ou projet, budget d'achat, rentabilité, factures fournisseurs."),
    ("module_import_metres", "Import de métrés Excel", "Créer un devis ou un avenant depuis un tableau Excel de métré (POSTE, DESCRIPTION, Qté, UNITE, PRIX)."),
];

struct Module {
    champ: &'static str,
    nom: &'static str,
    explication: &'static str,
    allume: bool,
}

#[derive(Template)]
#[template(path = "metier.html")]
struct Page {
    profil_metier: String,
    vocabulaire: String,
    modules: Vec<Module>,
    profils: &'static [(&'static str, &'static str)],
    proprietaire: bool,
    enregistre: bool,
    erreur: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

async fn afficher(mut db: Base, Utilisateur(moi): Utilisateur, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let organisation = db.organisation;
    let (profil_metier, vocabulaire, avancements, avenants, achats, import): (String, String, bool, bool, bool, bool) = sqlx::query_as(
        "SELECT profil_metier, vocabulaire, module_avancements, module_avenants, module_achats, module_import_metres
         FROM organisations WHERE id = $1",
    )
    .bind(organisation)
    .fetch_one(&mut *db)
    .await?;
    let allumes = [avancements, avenants, achats, import];
    let page = Page {
        profil_metier,
        vocabulaire,
        modules: MODULES.iter().zip(allumes).map(|(&(champ, nom, explication), allume)| Module { champ, nom, explication, allume }).collect(),
        profils: &PROFILS,
        proprietaire: crate::equipe::role(&mut db, moi, organisation).await?.as_deref() == Some("proprietaire"),
        enregistre: r.ok == "1",
        erreur: r.erreur == "droits",
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Saisie {
    profil_metier: String,
    vocabulaire: String,
    module_avancements: String,
    module_avenants: String,
    module_achats: String,
    module_import_metres: String,
}

async fn enregistrer(mut db: Base, Utilisateur(moi): Utilisateur, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let organisation = db.organisation;
    if crate::equipe::role(&mut db, moi, organisation).await?.as_deref() != Some("proprietaire") {
        return Ok(Redirect::to("/parametres/metier?erreur=droits").into_response());
    }
    let profil = if PROFILS.iter().any(|(v, _)| *v == s.profil_metier) { s.profil_metier.as_str() } else { "autre" };
    let vocabulaire = if s.vocabulaire == "projet" { "projet" } else { "chantier" };
    let coche = |v: &str| v == "on";
    sqlx::query(
        "UPDATE organisations SET profil_metier = $2, vocabulaire = $3, module_avancements = $4, module_avenants = $5,
                                  module_achats = $6, module_import_metres = $7
         WHERE id = $1",
    )
    .bind(organisation)
    .bind(profil)
    .bind(vocabulaire)
    .bind(coche(&s.module_avancements))
    .bind(coche(&s.module_avenants))
    .bind(coche(&s.module_achats))
    .bind(coche(&s.module_import_metres))
    .execute(&mut *db)
    .await?;
    Ok(Redirect::to("/parametres/metier?ok=1").into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/parametres/metier", get(afficher).post(enregistrer))
}
