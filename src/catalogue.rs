//! Les bibliothèques de Raven (étape 4-1) : Réglages › Bibliothèques.
//!
//! Le catalogue commun (migration 0035) est rangé en arbre métier › bibliothèque, sans aucun prix. Une organisation
//! coche les bibliothèques qui lui servent, quel que soit son métier : leurs ouvrages sont **copiés** dans sa propre
//! bibliothèque (prix vides, « à compléter »), et elle en fait ce qu'elle veut. Décocher retire les copies que
//! l'utilisateur n'a pas changées et **garde celles qu'il a modifiées** (`ouvrages.personnalise`), pour ne jamais lui
//! faire perdre son travail. Recocher ne crée pas de doublon.
//!
//! L'assistant de démarrage coche les bibliothèques du métier choisi (`cocher_metier`). Un propriétaire change les
//! choix ; un membre les voit. « Vider ma bibliothèque » (propriétaire seulement) supprime tous les ouvrages et décoche
//! tout ; les devis gardent leurs lignes (ce sont des copies).

use askama::Template;
use axum::{
    Router,
    extract::Query,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::PgConnection;

use crate::base::{Base, Utilisateur};
use crate::{AppState, Erreur};

/// Coche une bibliothèque et copie ses ouvrages qui ne sont pas encore dans la bibliothèque de l'organisation (ni
/// gardés d'une fois précédente). La catégorie de la copie est le nom de la bibliothèque. Renvoie le nombre d'ouvrages
/// ajoutés.
pub async fn cocher(db: &mut PgConnection, code: &str) -> Result<u64, sqlx::Error> {
    sqlx::query("INSERT INTO organisation_bibliotheques (bibliotheque) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(code)
        .execute(&mut *db)
        .await?;
    let fait = sqlx::query(
        "INSERT INTO ouvrages (description, genre, categorie, unite, catalogue_ouvrage_id)
         SELECT c.description, c.genre, b.nom, c.unite, c.id
         FROM catalogue_ouvrages c JOIN catalogue_bibliotheques b ON b.code = c.bibliotheque
         WHERE c.bibliotheque = $1 AND NOT EXISTS (SELECT 1 FROM ouvrages o WHERE o.catalogue_ouvrage_id = c.id)
         ORDER BY c.position",
    )
    .bind(code)
    .execute(&mut *db)
    .await?;
    Ok(fait.rows_affected())
}

/// Décoche une bibliothèque : ses copies non modifiées sont retirées. Renvoie (retirés, gardés car modifiés).
pub async fn decocher(db: &mut PgConnection, code: &str) -> Result<(u64, i64), sqlx::Error> {
    sqlx::query("DELETE FROM organisation_bibliotheques WHERE bibliotheque = $1").bind(code).execute(&mut *db).await?;
    let retires = sqlx::query(
        "DELETE FROM ouvrages o USING catalogue_ouvrages c
         WHERE o.catalogue_ouvrage_id = c.id AND c.bibliotheque = $1 AND NOT o.personnalise",
    )
    .bind(code)
    .execute(&mut *db)
    .await?
    .rows_affected();
    let gardes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ouvrages o JOIN catalogue_ouvrages c ON c.id = o.catalogue_ouvrage_id WHERE c.bibliotheque = $1",
    )
    .bind(code)
    .fetch_one(&mut *db)
    .await?;
    Ok((retires, gardes))
}

/// Coche toutes les bibliothèques d'un métier (assistant de démarrage). « autre » n'en a aucune.
pub async fn cocher_metier(db: &mut PgConnection, metier: &str) -> Result<(), sqlx::Error> {
    let codes: Vec<String> = sqlx::query_scalar("SELECT code FROM catalogue_bibliotheques WHERE metier = $1 ORDER BY position")
        .bind(metier)
        .fetch_all(&mut *db)
        .await?;
    for code in codes {
        cocher(db, &code).await?;
    }
    Ok(())
}

// ─── Écran ──────────────────────────────────────────────────────────────────

pub struct Bibliotheque {
    pub code: String,
    pub nom: String,
    pub nombre: i64,
    pub cochee: bool,
}

pub struct Metier {
    pub nom: String,
    /// Le métier de l'organisation, ou un métier dont une bibliothèque est cochée : ouvert d'office.
    pub ouvert: bool,
    pub bibliotheques: Vec<Bibliotheque>,
}

#[derive(Template)]
#[template(path = "catalogue.html")]
struct Page {
    metiers: Vec<Metier>,
    proprietaire: bool,
    message_ok: String,
    erreur: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ajoutes: Option<u64>,
    retires: Option<u64>,
    gardes: Option<i64>,
    erreur: String,
}

/// « 3 ouvrages ajoutés · 2 retirés · 1 gardé (modifié) » ; vide s'il n'y a rien à dire.
fn message(r: &Retour) -> String {
    let pluriel = |n: i64| if n > 1 { "s" } else { "" };
    let mut parties = Vec::new();
    if let Some(n) = r.ajoutes.filter(|n| *n > 0) {
        parties.push(format!("{n} ouvrage{} ajouté{} à votre bibliothèque", pluriel(n as i64), pluriel(n as i64)));
    }
    if let Some(n) = r.retires.filter(|n| *n > 0) {
        parties.push(format!("{n} retiré{}", pluriel(n as i64)));
    }
    if let Some(n) = r.gardes.filter(|n| *n > 0) {
        parties.push(if n > 1 { format!("{n} gardés (vous les aviez modifiés)") } else { "1 gardé (vous l'aviez modifié)".to_string() });
    }
    match (r.ajoutes.is_some() || r.retires.is_some(), parties.is_empty()) {
        (false, _) => String::new(),
        (true, true) => "Choix enregistrés.".to_string(),
        (true, false) => format!("Choix enregistrés : {}.", parties.join(" · ")),
    }
}

async fn est_proprietaire(db: &mut Base, moi: i64) -> Result<bool, sqlx::Error> {
    let organisation = db.organisation;
    Ok(crate::equipe::role(db, moi, organisation).await?.as_deref() == Some("proprietaire"))
}

async fn afficher(mut db: Base, Utilisateur(moi): Utilisateur, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let organisation = db.organisation;
    let metier_organisation: String =
        sqlx::query_scalar("SELECT profil_metier FROM organisations WHERE id = $1").bind(organisation).fetch_one(&mut *db).await?;
    let lignes: Vec<(String, String, String, String, i64, bool)> = sqlx::query_as(
        "SELECT m.code, m.nom, b.code, b.nom, (SELECT count(*) FROM catalogue_ouvrages c WHERE c.bibliotheque = b.code),
                EXISTS (SELECT 1 FROM organisation_bibliotheques o WHERE o.bibliotheque = b.code)
         FROM catalogue_bibliotheques b JOIN catalogue_metiers m ON m.code = b.metier
         ORDER BY m.position, b.position",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut metiers: Vec<(String, Metier)> = Vec::new();
    for (code_metier, nom_metier, code, nom, nombre, cochee) in lignes {
        if metiers.last().is_none_or(|(c, _)| *c != code_metier) {
            let ouvert = code_metier == metier_organisation;
            metiers.push((code_metier, Metier { nom: nom_metier, ouvert, bibliotheques: Vec::new() }));
        }
        let (_, m) = metiers.last_mut().expect("ajouté juste au-dessus");
        m.ouvert |= cochee;
        m.bibliotheques.push(Bibliotheque { code, nom, nombre, cochee });
    }
    let page = Page {
        metiers: metiers.into_iter().map(|(_, m)| m).collect(),
        proprietaire: est_proprietaire(&mut db, moi).await?,
        message_ok: message(&r),
        erreur: r.erreur == "droits",
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Choix {
    bibliotheque: Vec<String>,
}

async fn enregistrer(mut db: Base, Utilisateur(moi): Utilisateur, Form(choix): Form<Choix>) -> Result<Response, Erreur> {
    if !est_proprietaire(&mut db, moi).await? {
        return Ok(Redirect::to("/parametres/bibliotheques?erreur=droits").into_response());
    }
    let mut tx = db.begin().await?;
    let toutes: Vec<(String, bool)> = sqlx::query_as(
        "SELECT b.code, EXISTS (SELECT 1 FROM organisation_bibliotheques o WHERE o.bibliotheque = b.code)
         FROM catalogue_bibliotheques b ORDER BY b.position",
    )
    .fetch_all(&mut *tx)
    .await?;
    let (mut ajoutes, mut retires, mut gardes) = (0, 0, 0);
    for (code, cochee) in toutes {
        let voulue = choix.bibliotheque.contains(&code);
        if voulue && !cochee {
            ajoutes += cocher(&mut tx, &code).await?;
        } else if !voulue && cochee {
            let (r, g) = decocher(&mut tx, &code).await?;
            retires += r;
            gardes += g;
        }
    }
    tx.commit().await?;
    Ok(Redirect::to(&format!("/parametres/bibliotheques?ajoutes={ajoutes}&retires={retires}&gardes={gardes}")).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Confirmation {
    confirme: String,
}

/// « Vider ma bibliothèque » (case « Je confirme » cochée) : tous les ouvrages de l'organisation sont supprimés et plus
/// rien n'est coché.
async fn vider(mut db: Base, Utilisateur(moi): Utilisateur, Form(c): Form<Confirmation>) -> Result<Response, Erreur> {
    if !est_proprietaire(&mut db, moi).await? {
        return Ok(Redirect::to("/bibliotheque?erreur=droits").into_response());
    }
    if c.confirme != "oui" {
        return Ok(Redirect::to("/bibliotheque").into_response());
    }
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM organisation_bibliotheques").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM ouvrages").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Redirect::to("/bibliotheque?ok=vide").into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/parametres/bibliotheques", get(afficher).post(enregistrer))
        .route("/bibliotheque/vider", post(vider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_message_apres_enregistrement() {
        let r = |a, re, g| Retour { ajoutes: Some(a), retires: Some(re), gardes: Some(g), erreur: String::new() };
        assert_eq!(message(&Retour::default()), "");
        assert_eq!(message(&r(0, 0, 0)), "Choix enregistrés.");
        assert_eq!(message(&r(1, 0, 0)), "Choix enregistrés : 1 ouvrage ajouté à votre bibliothèque.");
        assert_eq!(
            message(&r(36, 25, 2)),
            "Choix enregistrés : 36 ouvrages ajoutés à votre bibliothèque · 25 retirés · 2 gardés (vous les aviez modifiés)."
        );
    }
}
