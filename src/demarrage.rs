//! L'assistant de démarrage (étape 2) : la première page d'une nouvelle organisation, une seule fois.
//!
//! Tant qu'il n'est pas fait, toutes les pages y mènent (`connexion::exiger_connexion`). On y donne en une fois ce
//! qu'il faut pour envoyer un premier devis ou une première facture : l'identité de l'entreprise (mêmes règles que
//! Réglages › Société), le régime de TVA, le compte bancaire, les **derniers numéros déjà utilisés** cette année (pour
//! continuer la numérotation d'un autre logiciel sans trou ni doublon) et le métier.
//! Les numéros ne se règlent ici que si l'organisation n'a encore aucune pièce numérotée.

use askama::Template;
use axum::{
    Router,
    extract::Form,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;
use sqlx::PgConnection;

use crate::base::{Base, Utilisateur};
use crate::commun::{Choix, PAYS, choix_fixes};
use crate::societe::{self, REGIMES_TVA, SocieteForm};
use crate::{AppState, Erreur};

/// Les métiers proposés (le profil règlera le vocabulaire et les modules à l'étape 3, la bibliothèque à l'étape 4).
pub const PROFILS: [(&str, &str); 5] = [
    ("batiment", "Bâtiment (rénovation, menuiserie, gyproc, maçonnerie…)"),
    ("espaces_verts", "Espaces verts (jardin, élagage, abattage…)"),
    ("peinture", "Peinture et décoration"),
    ("graphisme", "Graphisme, communication, services"),
    ("autre", "Autre métier"),
];

const NUMERO_MAX: i64 = 999_999;

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Saisie {
    nom: String,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    email: String,
    telephone: String,
    numero_tva: String,
    regime_tva: String,
    iban: String,
    bic: String,
    dernier_devis: String,
    derniere_facture: String,
    derniere_note_credit: String,
    profil_metier: String,
}

#[derive(Template)]
#[template(path = "demarrage.html")]
struct Page {
    s: Saisie,
    erreurs: Vec<String>,
    pays: Vec<Choix>,
    regimes: &'static [(&'static str, &'static str)],
    profils: &'static [(&'static str, &'static str)],
    annee: i32,
}

async fn annee(db: &mut PgConnection) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar("SELECT EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(db).await
}

async fn deja_fait(db: &mut Base) -> Result<bool, sqlx::Error> {
    let organisation = db.organisation;
    sqlx::query_scalar("SELECT demarre_le IS NOT NULL FROM organisations WHERE id = $1").bind(organisation).fetch_one(&mut **db).await
}

fn page(s: Saisie, erreurs: Vec<String>, annee: i32) -> Page {
    let pays = choix_fixes(&PAYS, std::slice::from_ref(&s.pays));
    Page { s, erreurs, pays, regimes: &REGIMES_TVA, profils: &PROFILS, annee }
}

async fn afficher(mut db: Base, Utilisateur(utilisateur): Utilisateur) -> Result<Response, Erreur> {
    if deja_fait(&mut db).await? {
        return Ok(Redirect::to("/").into_response());
    }
    let organisation = db.organisation;
    let (nom, profil): (String, String) =
        sqlx::query_as("SELECT nom, profil_metier FROM organisations WHERE id = $1").bind(organisation).fetch_one(&mut *db).await?;
    let email: String = sqlx::query_scalar("SELECT email FROM utilisateurs WHERE id = $1").bind(utilisateur).fetch_one(&mut *db).await?;
    let s = Saisie {
        nom,
        pays: "BE".into(),
        email,
        regime_tva: "assujetti".into(),
        dernier_devis: "0".into(),
        derniere_facture: "0".into(),
        derniere_note_credit: "0".into(),
        profil_metier: profil,
        ..Default::default()
    };
    let annee = annee(&mut db).await?;
    Ok(Html(page(s, Vec::new(), annee).render()?).into_response())
}

/// « 124 » → 124 ; vide → 0.
fn numero(saisie: &str, libelle: &str, erreurs: &mut Vec<String>) -> i64 {
    let t = saisie.trim();
    if t.is_empty() {
        return 0;
    }
    match t.parse::<i64>() {
        Ok(n) if (0..=NUMERO_MAX).contains(&n) => n,
        _ => {
            erreurs.push(format!("{libelle} : indique un nombre entier entre 0 et {NUMERO_MAX}."));
            0
        }
    }
}

async fn enregistrer(mut db: Base, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    if deja_fait(&mut db).await? {
        return Ok(Redirect::to("/").into_response());
    }
    let annee = annee(&mut db).await?;
    let form = SocieteForm::depuis_demarrage(
        &s.nom, &s.rue, &s.numero, &s.code_postal, &s.ville, &s.pays, &s.email, &s.telephone, &s.numero_tva, &s.regime_tva, &s.iban, &s.bic,
    );
    let mut erreurs = Vec::new();
    let valeurs = societe::valider(&form).map_err(|e| erreurs.extend(e)).ok();
    let dernier_devis = numero(&s.dernier_devis, "Dernier devis", &mut erreurs);
    let derniere_facture = numero(&s.derniere_facture, "Dernière facture", &mut erreurs);
    let derniere_note_credit = numero(&s.derniere_note_credit, "Dernière note de crédit", &mut erreurs);
    if !PROFILS.iter().any(|(v, _)| *v == s.profil_metier) {
        erreurs.push("Choisis ton métier.".to_string());
    }
    let Some(valeurs) = valeurs.filter(|_| erreurs.is_empty()) else {
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page(s, erreurs, annee).render()?)).into_response());
    };

    let organisation = db.organisation;
    let mut tx = db.begin().await?;
    let fait: bool = sqlx::query_scalar("SELECT demarre_le IS NOT NULL FROM organisations WHERE id = $1 FOR UPDATE")
        .bind(organisation)
        .fetch_one(&mut *tx)
        .await?;
    if fait {
        return Ok(Redirect::to("/").into_response());
    }
    societe::enregistrer(&mut tx, &valeurs).await?;
    // Les derniers numéros déjà utilisés : seulement tant qu'aucune pièce n'est numérotée ici.
    let pieces: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM factures WHERE numero IS NOT NULL) OR EXISTS (SELECT 1 FROM devis) OR EXISTS (SELECT 1 FROM chantiers)",
    )
    .fetch_one(&mut *tx)
    .await?;
    if !pieces {
        sqlx::query("UPDATE compteurs SET valeur = $1 WHERE organisation_id = app_organisation() AND nom = 'devis'")
            .bind(dernier_devis)
            .execute(&mut *tx)
            .await?;
        for (nom, valeur) in [("facture", derniere_facture), ("note_credit", derniere_note_credit)] {
            sqlx::query(
                "UPDATE compteurs SET valeur = $2, annee = CASE WHEN $2 > 0 THEN $3 END WHERE organisation_id = app_organisation() AND nom = $1",
            )
            .bind(nom)
            .bind(valeur)
            .bind(annee)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query("UPDATE organisations SET nom = $2, profil_metier = $3, demarre_le = now() WHERE id = $1")
        .bind(organisation)
        .bind(s.nom.trim())
        .bind(&s.profil_metier)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Redirect::to("/").into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/demarrage", get(afficher).post(enregistrer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeros_saisis() {
        let mut erreurs = Vec::new();
        assert_eq!(numero(" 124 ", "x", &mut erreurs), 124);
        assert_eq!(numero("", "x", &mut erreurs), 0);
        assert!(erreurs.is_empty());
        numero("-1", "Dernière facture", &mut erreurs);
        numero("douze", "x", &mut erreurs);
        numero("1000000", "x", &mut erreurs);
        assert_eq!(erreurs.len(), 3);
        assert!(erreurs[0].starts_with("Dernière facture"));
    }
}
