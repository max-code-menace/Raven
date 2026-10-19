//! Dupliquer un devis (étape 5c-3) : une copie en brouillon, avec le prochain numéro.
//! C'est la façon de corriger un devis déjà envoyé ou accepté : on ne modifie jamais ce que le client a reçu.

use axum::{
    Router,
    extract::Path,
    response::Redirect,
    routing::post,
};

use crate::base::Base;
use crate::devis_lignes::numero_affiche;
use crate::journal::{self, Genre};
use crate::{AppState, Erreur};

async fn dupliquer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let mut tx = db.begin().await?;

    let source: Option<(i32, i64, Option<i64>)> =
        sqlx::query_as("SELECT extract(year FROM date_devis)::int, numero, chantier_id FROM devis WHERE id = $1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (annee_source, numero_source, chantier) = source.ok_or_else(Erreur::introuvable)?;
    // Un devis prend le prochain numéro de la numérotation continue ; un avenant, le prochain numéro de son chantier.
    // Dans les deux cas, dans la même transaction que la copie.
    let numero = match chantier {
        None => sqlx::query_scalar("UPDATE compteurs SET valeur = valeur + 1 WHERE organisation_id = app_organisation() AND nom = 'devis' RETURNING valeur")
            .fetch_one(&mut *tx)
            .await?,
        Some(chantier_id) => crate::avenants::prochain_numero(&mut tx, chantier_id).await?,
    };

    // La copie repart en brouillon, datée d'aujourd'hui, sans décision du client.
    let nouveau: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, reference, responsable, client_entreprise_id, client_contact_id, taux_tva_id,
                conditions_paiement_id, facturation_identique, facturation_entreprise_id, facturation_contact_id,
                chantier_identique, chantier_rue, chantier_numero, chantier_code_postal, chantier_ville, chantier_pays,
                architecte_contact_id, gestionnaire_contact_id, remarque_1, remarque_2, montant_ht_cents, montant_ttc_cents,
                ajustement_libelle, ajustement_mode, ajustement_valeur, marge_activee, frais_generaux_centiemes,
                chantier_id, jours_ouvrables)
         SELECT $2, (now() AT TIME ZONE 'Europe/Brussels')::date, reference, responsable, client_entreprise_id, client_contact_id,
                taux_tva_id, conditions_paiement_id, facturation_identique, facturation_entreprise_id, facturation_contact_id,
                chantier_identique, chantier_rue, chantier_numero, chantier_code_postal, chantier_ville, chantier_pays,
                architecte_contact_id, gestionnaire_contact_id, remarque_1, remarque_2, montant_ht_cents, montant_ttc_cents,
                ajustement_libelle, ajustement_mode, ajustement_valeur, marge_activee, frais_generaux_centiemes,
                chantier_id, jours_ouvrables
         FROM devis WHERE id = $1
         RETURNING id",
    )
    .bind(id)
    .bind(numero)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5,
                prix_achat_unitaire_e5, taux_tva_id, remarque)
         SELECT $2, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, taux_tva_id, remarque
         FROM devis_lignes WHERE devis_id = $1",
    )
    .bind(id)
    .bind(nouveau)
    .execute(&mut *tx)
    .await?;

    // Le numéro affiché d'un devis est « année-numéro », l'année étant celle de sa date ; un avenant garde son numéro.
    let annee_copie: i32 = sqlx::query_scalar("SELECT extract(year FROM date_devis)::int FROM devis WHERE id = $1")
        .bind(nouveau)
        .fetch_one(&mut *tx)
        .await?;
    let avenant = chantier.is_some();
    let source = numero_affiche(annee_source, numero_source, avenant);
    let copie = numero_affiche(annee_copie, numero, avenant);
    let (cree, depuis) = if avenant {
        (format!("Avenant créé par duplication de l'avenant {source}"), format!("Dupliqué vers l'avenant {copie}"))
    } else {
        (format!("Devis créé par duplication du devis {source}"), format!("Dupliqué vers le devis {copie}"))
    };
    journal::noter(&mut tx, nouveau, Genre::Creation, &cree).await?;
    journal::noter(&mut tx, id, Genre::Duplication, &depuis).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/devis/{nouveau}")))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/dupliquer", post(dupliquer))
}
