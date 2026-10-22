//! Métré dans la ligne (refonte du design, étape 4-2) : les mesures d'une ligne chiffrée du devis, dépliées juste sous
//! elle (longueur × largeur, ou une seule longueur ; une déduction compte en négatif). Leur somme devient la quantité
//! de la ligne : totaux, avancements, PDF et factures lisent la quantité comme avant.

use askama::Template;
use axum::{
    Router,
    extract::{Form as FormSimple, Path},
    response::Html,
    routing::post,
};
use serde::Deserialize;
use sqlx::{Postgres, Transaction};

use crate::base::Base;
use crate::calcul;
use crate::devis_lignes::{Bloc, Maj, Mesure, charger_encodage_metre, charger_entete, exiger_brouillon, recalculer};
use crate::{AppState, Erreur};

const LONGUEUR_MAX_LIBELLE: usize = 200;

/// Vérifie que la ligne est une ligne chiffrée de ce devis et la verrouille avec le devis.
async fn ligne_chiffree(tx: &mut Transaction<'_, Postgres>, id: i64, ligne_id: i64) -> Result<Option<i64>, Erreur> {
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut **tx).await?;
    let ligne: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT genre::text, quantite_e5 FROM devis_lignes WHERE id = $1 AND devis_id = $2")
            .bind(ligne_id)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let (genre, quantite) = ligne.ok_or_else(Erreur::introuvable)?;
    if calcul::Categorie::depuis_genre(&genre).is_none() {
        return Err(Erreur::conflit("Seule une ligne chiffrée a un métré."));
    }
    Ok(quantite)
}

/// Recopie la somme des mesures dans la quantité de la ligne, puis recalcule les totaux du devis.
/// Sans mesure, la ligne garde sa quantité.
async fn reporter(tx: &mut Transaction<'_, Postgres>, id: i64, ligne_id: i64) -> Result<(), sqlx::Error> {
    let mesures: Vec<Mesure> = sqlx::query_as(
        "SELECT id, ligne_id, libelle, longueur_e5, largeur_e5, deduction FROM devis_metres WHERE ligne_id = $1",
    )
    .bind(ligne_id)
    .fetch_all(&mut **tx)
    .await?;
    if !mesures.is_empty() {
        let somme: i64 = mesures.iter().map(Mesure::resultat_e5).sum();
        sqlx::query("UPDATE devis_lignes SET quantite_e5 = $1, updated_at = now() WHERE id = $2")
            .bind(somme)
            .bind(ligne_id)
            .execute(&mut **tx)
            .await?;
    }
    recalculer(tx, id).await
}

/// Ajoute une mesure. La première reprend la quantité de la ligne (la quantité ne change pas) ; les suivantes
/// partent de 0.
async fn ajouter(mut db: Base, Path((id, ligne_id)): Path<(i64, i64)>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = db.begin().await?;
    let quantite = ligne_chiffree(&mut tx, id, ligne_id).await?;
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_metres WHERE ligne_id = $1").bind(ligne_id).fetch_one(&mut *tx).await?;
    let depart = if nombre == 0 { quantite.unwrap_or(0) } else { 0 };
    sqlx::query(
        "INSERT INTO devis_metres (ligne_id, position, longueur_e5, deduction)
         VALUES ($1, COALESCE((SELECT max(position) FROM devis_metres WHERE ligne_id = $1), 0) + 1, $2, $3)",
    )
    .bind(ligne_id)
    .bind(depart.abs())
    .bind(depart < 0)
    .execute(&mut *tx)
    .await?;
    reporter(&mut tx, id, ligne_id).await?;
    tx.commit().await?;
    let enc = charger_encodage_metre(&mut *db, id, &entete, String::new(), Some(ligne_id)).await?;
    Ok(Html(Bloc { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MajMesure {
    libelle: String,
    longueur: String,
    largeur: String,
    deduction: Option<String>,
}

/// Une dimension : vide = 0 (longueur) ou absente (largeur) ; jamais négative.
fn lire_dimension(texte: &str, nom: &str) -> Result<Option<i64>, String> {
    match texte.trim() {
        "" => Ok(None),
        t => match calcul::lire_decimal_e5(t) {
            Some(v) if v >= 0 => Ok(Some(v)),
            Some(_) => Err(format!("La {nom} est positive : cochez « À déduire » pour retirer une surface.")),
            None => Err(format!("La {nom} « {t} » n'est pas un nombre (exemple : 2,48).")),
        },
    }
}

fn lire(f: &MajMesure) -> Result<(String, i64, Option<i64>, bool), String> {
    let libelle = f.libelle.trim();
    if libelle.chars().count() > LONGUEUR_MAX_LIBELLE {
        return Err(format!("Le libellé de la mesure est trop long ({LONGUEUR_MAX_LIBELLE} caractères au maximum)."));
    }
    let longueur = lire_dimension(&f.longueur, "longueur")?.unwrap_or(0);
    let largeur = lire_dimension(&f.largeur, "largeur")?;
    Ok((libelle.to_string(), longueur, largeur, f.deduction.is_some()))
}

/// Enregistre une mesure (à la sortie du champ). Ne renvoie que les nombres qui changent, pour ne pas toucher aux
/// champs en cours de saisie.
async fn modifier(
    mut db: Base,
    Path((id, ligne_id, mesure_id)): Path<(i64, i64, i64)>,
    FormSimple(f): FormSimple<MajMesure>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = db.begin().await?;
    ligne_chiffree(&mut tx, id, ligne_id).await?;
    let message = match lire(&f) {
        Ok((libelle, longueur, largeur, deduction)) => {
            let faite = sqlx::query(
                "UPDATE devis_metres SET libelle = $1, longueur_e5 = $2, largeur_e5 = $3, deduction = $4 WHERE id = $5 AND ligne_id = $6",
            )
            .bind(libelle)
            .bind(longueur)
            .bind(largeur)
            .bind(deduction)
            .bind(mesure_id)
            .bind(ligne_id)
            .execute(&mut *tx)
            .await?;
            if faite.rows_affected() == 0 {
                return Err(Erreur::introuvable());
            }
            reporter(&mut tx, id, ligne_id).await?;
            String::new()
        }
        Err(m) => m,
    };
    tx.commit().await?;
    let enc = charger_encodage_metre(&mut *db, id, &entete, message, Some(ligne_id)).await?;
    Ok(Html(Maj { enc }.render()?))
}

/// Retire une mesure. Quand il n'en reste aucune, la ligne garde sa dernière quantité et redevient saisie à la main.
async fn supprimer(mut db: Base, Path((id, ligne_id, mesure_id)): Path<(i64, i64, i64)>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = db.begin().await?;
    ligne_chiffree(&mut tx, id, ligne_id).await?;
    sqlx::query("DELETE FROM devis_metres WHERE id = $1 AND ligne_id = $2").bind(mesure_id).bind(ligne_id).execute(&mut *tx).await?;
    reporter(&mut tx, id, ligne_id).await?;
    tx.commit().await?;
    let enc = charger_encodage_metre(&mut *db, id, &entete, String::new(), Some(ligne_id)).await?;
    Ok(Html(Bloc { enc }.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devis/{id}/lignes/{ligne_id}/mesures", post(ajouter))
        .route("/devis/{id}/lignes/{ligne_id}/mesures/{mesure_id}", post(modifier))
        .route("/devis/{id}/lignes/{ligne_id}/mesures/{mesure_id}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_lues() {
        let f = |l: &str, w: &str, d: bool| MajMesure { libelle: " Mur A ".into(), longueur: l.into(), largeur: w.into(), deduction: d.then(String::new) };
        assert_eq!(lire(&f("2,48", "2,5", false)), Ok(("Mur A".to_string(), 248_000, Some(250_000), false)));
        assert_eq!(lire(&f("", "", true)), Ok(("Mur A".to_string(), 0, None, true)));
        assert!(lire(&f("-1", "", false)).unwrap_err().contains("À déduire"));
        assert!(lire(&f("2", "abc", false)).unwrap_err().contains("largeur"));
    }
}
