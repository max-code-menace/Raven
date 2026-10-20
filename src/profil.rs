//! Le profil de l'organisation (étape 3) : son vocabulaire (« Chantier » ou « Projet ») et ses modules allumés.
//!
//! Il est lu une fois par page (avec la session, `connexion::exiger_connexion`, ou pour l'organisation n° 1 dans
//! `crate::app`) et reste connu pendant toute la page (`tokio::task_local`) : les gabarits l'utilisent directement,
//! `{{ crate::profil::mot("Chantier") }}`, `{% if crate::profil::actuel().avenants %}`, sans que chaque écran ait à le
//! transmettre. Hors d'une page (ligne de commande, tests de fonctions), c'est le profil du bâtiment, tout allumé.
//!
//! « chantier » et « projet » sont tous deux masculins : « le », « du », « un », « nouveau » restent justes.

use sqlx::PgConnection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profil {
    /// « Projet » au lieu de « Chantier ».
    pub projet: bool,
    /// Avancements et factures d'avancement.
    pub avancements: bool,
    pub avenants: bool,
    /// Achats du chantier, factures fournisseurs saisies, rentabilité et budget d'achat.
    pub achats: bool,
    /// Import du modèle Excel de métré (« Depuis Excel »).
    pub import_metres: bool,
}

impl Profil {
    pub const BATIMENT: Profil = Profil { projet: false, avancements: true, avenants: true, achats: true, import_metres: false };
}

tokio::task_local! {
    static PROFIL: Profil;
}

/// Le profil de la page en cours.
pub fn actuel() -> Profil {
    PROFIL.try_with(|p| *p).unwrap_or(Profil::BATIMENT)
}

/// Exécute la page avec ce profil.
pub async fn avec<F: std::future::Future>(profil: Profil, page: F) -> F::Output {
    PROFIL.scope(profil, page).await
}

/// Le profil d'une organisation.
pub async fn charger(db: &mut PgConnection, organisation: i64) -> Result<Profil, sqlx::Error> {
    let (vocabulaire, avancements, avenants, achats, import_metres): (String, bool, bool, bool, bool) = sqlx::query_as(
        "SELECT vocabulaire, module_avancements, module_avenants, module_achats, module_import_metres FROM organisations WHERE id = $1",
    )
    .bind(organisation)
    .fetch_one(db)
    .await?;
    Ok(Profil { projet: vocabulaire == "projet", avancements, avenants, achats, import_metres })
}

/// L'organisation de la connexion parle-t-elle de « projet » ? Lu dans la base (les PDF s'en servent : ils sont
/// aussi fabriqués pour la page client en ligne, hors de toute session).
pub async fn projet_en_base(db: &mut PgConnection) -> Result<bool, sqlx::Error> {
    let projet: Option<bool> =
        sqlx::query_scalar("SELECT vocabulaire = 'projet' FROM organisations WHERE id = app_organisation()").fetch_optional(db).await?;
    Ok(projet.unwrap_or(false))
}

/// Le mot de l'organisation pour une forme de « chantier » : « Chantier », « chantier », « Chantiers », « chantiers ».
pub fn mot(forme: &str) -> &'static str {
    mot_pour(actuel().projet, forme)
}

fn mot_pour(projet: bool, forme: &str) -> &'static str {
    match (projet, forme) {
        (false, "Chantier") => "Chantier",
        (false, "Chantiers") => "Chantiers",
        (false, "chantiers") => "chantiers",
        (false, _) => "chantier",
        (true, "Chantier") => "Projet",
        (true, "Chantiers") => "Projets",
        (true, "chantiers") => "projets",
        (true, _) => "projet",
    }
}

/// Le nom d'un type de ligne (« Main d'œuvre », « Sous-traitant »…) pour l'organisation : les métiers « Projet »
/// parlent de « Prestation » et de « Sous-traitance » (choix de Mahx du 20/10/2026 : des mots du métier, pas du
/// chantier). Le type lui-même, et donc les calculs (heures, rentabilité), ne changent pas.
pub fn type_ligne(libelle: &'static str) -> &'static str {
    type_ligne_pour(actuel().projet, libelle)
}

fn type_ligne_pour(projet: bool, libelle: &'static str) -> &'static str {
    match (projet, libelle) {
        (true, "Main d'œuvre") => "Prestation",
        (true, "Sous-traitant") => "Sous-traitance",
        _ => libelle,
    }
}

/// Le nom d'une catégorie de postes, au pluriel (rentabilité, achats, suivi) : « Main d'œuvre » devient
/// « Prestations », « Sous-traitants » devient « Sous-traitance » pour les métiers « Projet ».
pub fn categorie(libelle: &'static str) -> &'static str {
    match (actuel().projet, libelle) {
        (true, "Main d'œuvre") => "Prestations",
        (true, "Sous-traitants") => "Sous-traitance",
        _ => libelle,
    }
}

/// Remplace « chantier » (toutes ses formes, en mot entier) par le mot de l'organisation dans un texte écrit par
/// l'application (jamais dans un texte saisi par l'utilisateur).
pub fn adapter(texte: &str) -> String {
    if !actuel().projet {
        return texte.to_string();
    }
    let mut sortie = String::with_capacity(texte.len());
    let mut reste = texte;
    while let Some(pos) = reste.find(['C', 'c']) {
        let (avant, depuis) = reste.split_at(pos);
        sortie.push_str(avant);
        let forme = ["Chantiers", "chantiers", "Chantier", "chantier"].into_iter().find(|f| depuis.starts_with(f));
        let lettre = |c: char| c.is_alphanumeric() || c == '_';
        let precede_lettre = sortie.chars().last().is_some_and(lettre);
        match forme {
            Some(f) if !precede_lettre && !depuis[f.len()..].starts_with(lettre) => {
                sortie.push_str(mot_pour(true, f));
                reste = &depuis[f.len()..];
            }
            _ => {
                sortie.push_str(&depuis[..1]);
                reste = &depuis[1..];
            }
        }
    }
    sortie.push_str(reste);
    sortie
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn le_mot_suit_le_profil() {
        assert_eq!(mot("Chantier"), "Chantier", "hors d'une page : bâtiment");
        let projet = Profil { projet: true, ..Profil::BATIMENT };
        avec(projet, async {
            assert_eq!((mot("Chantier"), mot("chantiers"), mot("Chantiers"), mot("chantier")), ("Projet", "projets", "Projets", "projet"));
            assert_eq!(
                adapter("Chantier n° 3 · Nouveau chantier · 2 chantiers · Chantiers en cours · Achats du chantier"),
                "Projet n° 3 · Nouveau projet · 2 projets · Projets en cours · Achats du projet"
            );
            assert_eq!(adapter("chantiers_liste et Préchantier"), "chantiers_liste et Préchantier", "mots entiers seulement");
            assert_eq!((type_ligne("Main d'œuvre"), type_ligne("Sous-traitant"), type_ligne("Fourniture")), ("Prestation", "Sous-traitance", "Fourniture"));
            assert_eq!((categorie("Main d'œuvre"), categorie("Sous-traitants"), categorie("Équipements")), ("Prestations", "Sous-traitance", "Équipements"));
        })
        .await;
        assert_eq!(adapter("Chantier"), "Chantier");
        assert_eq!((type_ligne("Main d'œuvre"), categorie("Main d'œuvre")), ("Main d'œuvre", "Main d'œuvre"), "bâtiment");
    }
}

// ─── Gardes des adresses d'un module ────────────────────────────────────────
// Posées sur les routes d'un module (`route_layer`) : module éteint, l'adresse répond « module éteint » (409), même si
// on la tape à la main.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{Html, IntoResponse, Response},
};

async fn exiger(allume: bool, module: &str, requete: Request, suite: Next) -> Response {
    if allume {
        suite.run(requete).await
    } else {
        let message = format!("<p>Le module « {module} » est éteint : un propriétaire peut l'allumer dans Réglages › Métier et modules.</p>");
        (StatusCode::CONFLICT, Html(message)).into_response()
    }
}

pub async fn exiger_avancements(requete: Request, suite: Next) -> Response {
    exiger(actuel().avancements, "Avancements", requete, suite).await
}

pub async fn exiger_avenants(requete: Request, suite: Next) -> Response {
    exiger(actuel().avenants, "Avenants", requete, suite).await
}

pub async fn exiger_achats(requete: Request, suite: Next) -> Response {
    exiger(actuel().achats, "Achats et rentabilité", requete, suite).await
}

pub async fn exiger_import_metres(requete: Request, suite: Next) -> Response {
    exiger(actuel().import_metres, "Import de métrés Excel", requete, suite).await
}
