//! Les organisations (les entreprises clientes de Raven) : création et liste.
//!
//! Une nouvelle organisation reçoit tout ce dont ses pages ont besoin : ses réglages (délais des rappels), ses
//! compteurs à zéro (devis, chantiers, factures, notes de crédit : sa première facture sera la n° 1) et ses modèles
//! d'e-mails (copie des textes de départ neutres, `modeles_email_defaut`). Sa société reste à remplir (Réglages ›
//! Société) : les pages et les PDF le signalent déjà quand elle manque.

use sqlx::{Connection as _, PgPool};

/// Les compteurs de numérotation d'une organisation.
const COMPTEURS: [&str; 4] = ["devis", "chantier", "facture", "note_credit"];

/// Crée une organisation complète et rend son numéro. Tout est créé dans une seule transaction : en cas d'erreur, rien
/// ne reste.
pub async fn creer(db: &PgPool, nom: &str) -> Result<i64, String> {
    let nom = nom.trim();
    if nom.is_empty() {
        return Err("Le nom de l'organisation est obligatoire.".to_string());
    }
    let erreur = |e: sqlx::Error| e.to_string();
    let mut conn = db.acquire().await.map_err(erreur)?;
    let mut tx = conn.begin().await.map_err(erreur)?;
    let id: i64 = sqlx::query_scalar("INSERT INTO organisations (nom) VALUES ($1) RETURNING id").bind(nom).fetch_one(&mut *tx).await.map_err(erreur)?;
    // Le temps de cette transaction seulement, la connexion travaille dans la nouvelle organisation.
    sqlx::query("SELECT set_config('app.organisation_id', $1, true)").bind(id.to_string()).execute(&mut *tx).await.map_err(erreur)?;
    sqlx::query("INSERT INTO reglages DEFAULT VALUES").execute(&mut *tx).await.map_err(erreur)?;
    for compteur in COMPTEURS {
        sqlx::query("INSERT INTO compteurs (nom, valeur) VALUES ($1, 0)").bind(compteur).execute(&mut *tx).await.map_err(erreur)?;
    }
    sqlx::query("INSERT INTO modeles_email (code, nom, position, objet, corps) SELECT code, nom, position, objet, corps FROM modeles_email_defaut")
        .execute(&mut *tx)
        .await
        .map_err(erreur)?;
    tx.commit().await.map_err(erreur)?;
    Ok(id)
}

/// Les organisations (numéro et nom).
pub async fn liste(db: &PgPool) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as("SELECT id, nom FROM organisations ORDER BY id").fetch_all(db).await
}
