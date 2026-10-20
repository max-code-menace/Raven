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
//!
//! Étape 4-4 : quand le back office publie de nouveaux ouvrages dans une bibliothèque cochée, l'organisation voit
//! « N nouveaux ouvrages disponibles » et les ajoute d'un clic (`ajouter_nouveautes`) ; rien d'autre ne change.

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

/// Une bibliothèque proposée aux organisations : publiée et pas retirée du catalogue (back office, étape 4-3).
const OFFERTE: &str = "b.publiee_le IS NOT NULL AND b.retiree_le IS NULL";
/// Un ouvrage du catalogue qu'on copie : publié et pas retiré.
const COPIABLE: &str = "c.publie_le IS NOT NULL AND c.retire_le IS NULL";

/// Coche une bibliothèque et copie ses ouvrages (publiés, pas retirés) qui ne sont pas encore dans la bibliothèque de
/// l'organisation : ni copiés avant, ni gardés d'une fois précédente, ni dans la corbeille. La catégorie de la copie
/// est le nom de la bibliothèque. Sert aussi à ajouter les nouveaux ouvrages d'une bibliothèque déjà cochée (4-4) :
/// seuls ceux qui manquent sont copiés. Renvoie le nombre d'ouvrages ajoutés.
pub async fn cocher(db: &mut PgConnection, code: &str) -> Result<u64, sqlx::Error> {
    sqlx::query("INSERT INTO organisation_bibliotheques (bibliotheque) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(code)
        .execute(&mut *db)
        .await?;
    let fait = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO ouvrages (description, genre, categorie, unite, catalogue_ouvrage_id)
         SELECT c.description, c.genre, b.nom, c.unite, c.id
         FROM catalogue_ouvrages c JOIN catalogue_bibliotheques b ON b.code = c.bibliotheque
         WHERE c.bibliotheque = $1 AND {COPIABLE} AND NOT EXISTS (SELECT 1 FROM ouvrages o WHERE o.catalogue_ouvrage_id = c.id)
         ORDER BY c.position"
    )))
    .bind(code)
    .execute(&mut *db)
    .await?;
    Ok(fait.rows_affected())
}

/// Décoche une bibliothèque : ses copies non modifiées sont retirées ; celles que l'utilisateur a modifiées et celles
/// qui sont dans la corbeille restent. Renvoie (retirés, gardés car modifiés).
pub async fn decocher(db: &mut PgConnection, code: &str) -> Result<(u64, i64), sqlx::Error> {
    sqlx::query("DELETE FROM organisation_bibliotheques WHERE bibliotheque = $1").bind(code).execute(&mut *db).await?;
    let retires = sqlx::query(
        "DELETE FROM ouvrages o USING catalogue_ouvrages c
         WHERE o.catalogue_ouvrage_id = c.id AND c.bibliotheque = $1 AND NOT o.personnalise AND o.supprime_le IS NULL",
    )
    .bind(code)
    .execute(&mut *db)
    .await?
    .rows_affected();
    let gardes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ouvrages o JOIN catalogue_ouvrages c ON c.id = o.catalogue_ouvrage_id
         WHERE c.bibliotheque = $1 AND o.supprime_le IS NULL",
    )
    .bind(code)
    .fetch_one(&mut *db)
    .await?;
    Ok((retires, gardes))
}

/// Coche toutes les bibliothèques proposées d'un métier (assistant de démarrage). « autre » n'en a aucune.
pub async fn cocher_metier(db: &mut PgConnection, metier: &str) -> Result<(), sqlx::Error> {
    let codes: Vec<String> =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT b.code FROM catalogue_bibliotheques b WHERE b.metier = $1 AND {OFFERTE} ORDER BY b.position")))
            .bind(metier)
            .fetch_all(&mut *db)
            .await?;
    for code in codes {
        cocher(db, &code).await?;
    }
    Ok(())
}

/// Le nombre d'ouvrages publiés depuis, dans les bibliothèques cochées par l'organisation, qu'elle n'a pas encore
/// (étape 4-4). Un ouvrage qu'elle a mis à la corbeille n'en fait pas partie : il ne revient pas.
pub async fn nouveautes(db: &mut PgConnection) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM catalogue_ouvrages c
         JOIN catalogue_bibliotheques b ON b.code = c.bibliotheque
         JOIN organisation_bibliotheques ob ON ob.bibliotheque = b.code
         WHERE {OFFERTE} AND {COPIABLE} AND NOT EXISTS (SELECT 1 FROM ouvrages o WHERE o.catalogue_ouvrage_id = c.id)"
    )))
    .fetch_one(db)
    .await
}

// ─── Écran ──────────────────────────────────────────────────────────────────

pub struct Bibliotheque {
    pub code: String,
    pub nom: String,
    pub nombre: i64,
    pub cochee: bool,
    /// Retirée du catalogue : plus proposée ; une organisation qui l'avait cochée la voit encore et peut la décocher.
    pub retiree: bool,
    /// Les nouveaux ouvrages publiés qu'elle n'a pas encore (bibliothèque cochée).
    pub nouveaux: i64,
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
    nouveautes: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ajoutes: Option<u64>,
    retires: Option<u64>,
    gardes: Option<i64>,
    /// Après « Ajouter les nouveaux ouvrages » : combien.
    nouveaux: Option<u64>,
    erreur: String,
}

/// « 3 ouvrages ajoutés · 2 retirés · 1 gardé (modifié) » ; vide s'il n'y a rien à dire.
fn message(r: &Retour) -> String {
    let pluriel = |n: i64| if n > 1 { "s" } else { "" };
    if let Some(n) = r.nouveaux {
        return match n {
            0 => "Aucun nouvel ouvrage à ajouter.".to_string(),
            1 => "1 nouvel ouvrage ajouté à votre bibliothèque, sans prix : indiquez le vôtre.".to_string(),
            n => format!("{n} nouveaux ouvrages ajoutés à votre bibliothèque, sans prix : indiquez les vôtres."),
        };
    }
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

/// Les bibliothèques montrées : celles que le catalogue propose, plus celles que l'organisation a cochées (même
/// retirées depuis). (code, cochée, proposée).
async fn visibles(db: &mut PgConnection) -> Result<Vec<(String, bool, bool)>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT b.code, EXISTS (SELECT 1 FROM organisation_bibliotheques o WHERE o.bibliotheque = b.code), {OFFERTE}
         FROM catalogue_bibliotheques b ORDER BY b.position"
    )))
    .fetch_all(db)
    .await
    .map(|l: Vec<(String, bool, bool)>| l.into_iter().filter(|(_, cochee, offerte)| *cochee || *offerte).collect())
}

async fn afficher(mut db: Base, Utilisateur(moi): Utilisateur, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let organisation = db.organisation;
    let metier_organisation: String =
        sqlx::query_scalar("SELECT profil_metier FROM organisations WHERE id = $1").bind(organisation).fetch_one(&mut *db).await?;
    let lignes: Vec<(String, String, String, String, i64, bool, bool, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT m.code, m.nom, b.code, b.nom,
                (SELECT count(*) FROM catalogue_ouvrages c WHERE c.bibliotheque = b.code AND {COPIABLE}),
                EXISTS (SELECT 1 FROM organisation_bibliotheques o WHERE o.bibliotheque = b.code) AS cochee,
                b.retiree_le IS NOT NULL,
                (SELECT count(*) FROM catalogue_ouvrages c
                 WHERE c.bibliotheque = b.code AND {COPIABLE} AND EXISTS (SELECT 1 FROM organisation_bibliotheques o WHERE o.bibliotheque = b.code)
                   AND NOT EXISTS (SELECT 1 FROM ouvrages x WHERE x.catalogue_ouvrage_id = c.id))
         FROM catalogue_bibliotheques b JOIN catalogue_metiers m ON m.code = b.metier
         WHERE b.publiee_le IS NOT NULL AND (b.retiree_le IS NULL OR EXISTS (SELECT 1 FROM organisation_bibliotheques o WHERE o.bibliotheque = b.code))
         ORDER BY m.position, b.position"
    )))
    .fetch_all(&mut *db)
    .await?;
    let mut metiers: Vec<(String, Metier)> = Vec::new();
    for (code_metier, nom_metier, code, nom, nombre, cochee, retiree, nouveaux) in lignes {
        if metiers.last().is_none_or(|(c, _)| *c != code_metier) {
            let ouvert = code_metier == metier_organisation;
            metiers.push((code_metier, Metier { nom: nom_metier, ouvert, bibliotheques: Vec::new() }));
        }
        let (_, m) = metiers.last_mut().expect("ajouté juste au-dessus");
        m.ouvert |= cochee;
        // Une bibliothèque retirée n'a plus de nouveautés.
        let nouveaux = if retiree { 0 } else { nouveaux };
        m.bibliotheques.push(Bibliotheque { code, nom, nombre, cochee, retiree, nouveaux });
    }
    let page = Page {
        metiers: metiers.into_iter().map(|(_, m)| m).collect(),
        proprietaire: est_proprietaire(&mut db, moi).await?,
        message_ok: message(&r),
        erreur: r.erreur == "droits",
        nouveautes: nouveautes(&mut db).await?,
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
    let (mut ajoutes, mut retires, mut gardes) = (0, 0, 0);
    for (code, cochee, offerte) in visibles(&mut tx).await? {
        let voulue = choix.bibliotheque.contains(&code);
        // On ne coche qu'une bibliothèque que le catalogue propose ; une bibliothèque retirée se décoche seulement.
        if voulue && !cochee && offerte {
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

/// « Ajouter les nouveaux ouvrages » (4-4) : les ouvrages publiés depuis dans les bibliothèques cochées (et toujours
/// proposées) sont copiés, sans prix. Rien d'autre ne change : les ouvrages déjà là restent tels quels.
async fn ajouter_nouveautes(mut db: Base, Utilisateur(moi): Utilisateur) -> Result<Response, Erreur> {
    if !est_proprietaire(&mut db, moi).await? {
        return Ok(Redirect::to("/parametres/bibliotheques?erreur=droits").into_response());
    }
    let mut tx = db.begin().await?;
    let mut ajoutes = 0;
    for (code, cochee, offerte) in visibles(&mut tx).await? {
        if cochee && offerte {
            ajoutes += cocher(&mut tx, &code).await?;
        }
    }
    tx.commit().await?;
    Ok(Redirect::to(&format!("/parametres/bibliotheques?nouveaux={ajoutes}")).into_response())
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
        .route("/parametres/bibliotheques/nouveautes", post(ajouter_nouveautes))
        .route("/bibliotheque/vider", post(vider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_message_apres_enregistrement() {
        let r = |a, re, g| Retour { ajoutes: Some(a), retires: Some(re), gardes: Some(g), ..Default::default() };
        assert_eq!(message(&Retour::default()), "");
        assert_eq!(message(&r(0, 0, 0)), "Choix enregistrés.");
        assert_eq!(message(&r(1, 0, 0)), "Choix enregistrés : 1 ouvrage ajouté à votre bibliothèque.");
        assert_eq!(
            message(&r(36, 25, 2)),
            "Choix enregistrés : 36 ouvrages ajoutés à votre bibliothèque · 25 retirés · 2 gardés (vous les aviez modifiés)."
        );
        let n = |n| Retour { nouveaux: Some(n), ..Default::default() };
        assert_eq!(message(&n(0)), "Aucun nouvel ouvrage à ajouter.");
        assert_eq!(message(&n(1)), "1 nouvel ouvrage ajouté à votre bibliothèque, sans prix : indiquez le vôtre.");
        assert_eq!(message(&n(3)), "3 nouveaux ouvrages ajoutés à votre bibliothèque, sans prix : indiquez les vôtres.");
    }
}
