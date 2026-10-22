//! Le statut d'un devis (étape 5c-2) : marquer comme envoyé, accepter, refuser, remettre en brouillon.
//! Chaque changement est écrit dans le journal du devis, dans la même transaction.
//! « Envoyer par e-mail » (étape 8-2, `envoi.rs`) passe aussi le devis à « Envoyé » ; « Marquer comme envoyé » reste pour un envoi fait autrement.

use axum::{
    Router,
    extract::{Form, Path},
    response::Redirect,
    routing::post,
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::PgConnection;

use crate::base::Base;
use crate::devis::statut;
use crate::journal::{self, Genre};
use crate::{AppState, Erreur};

const LONGUEUR_MAX_MOTIF: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    MarquerEnvoye,
    Accepter,
    Refuser,
    RemettreEnBrouillon,
}

impl Action {
    /// Le statut obtenu.
    fn vers(self) -> &'static str {
        match self {
            Action::MarquerEnvoye => "envoye",
            Action::Accepter => "accepte",
            Action::Refuser => "refuse",
            Action::RemettreEnBrouillon => "brouillon",
        }
    }

    /// Ce que le devis peut faire selon son statut actuel.
    /// - un devis accepté est final (son chantier est créé à l'acceptation) ;
    /// - un devis signé par le client ne peut qu'être accepté ;
    /// - un devis envoyé ou refusé peut être remis en brouillon (c'est noté dans l'historique) ;
    ///   pour changer un devis accepté, on le duplique.
    pub fn permise_depuis(self, statut: &str) -> bool {
        match (self, statut) {
            (_, "accepte") => false,
            (Action::Accepter, _) => true,
            (Action::Refuser, "signe") => false,
            (Action::Refuser, _) => true,
            (Action::MarquerEnvoye, "brouillon" | "a_envoyer" | "echec_envoi") => true,
            (Action::MarquerEnvoye, _) => false,
            (Action::RemettreEnBrouillon, "a_envoyer" | "envoye" | "echec_envoi" | "refuse") => true,
            (Action::RemettreEnBrouillon, _) => false,
        }
    }
}

/// Ce que l'en-tête du devis propose comme boutons.
pub struct Barre {
    pub peut_marquer_envoye: bool,
    pub peut_accepter: bool,
    pub peut_refuser: bool,
    pub peut_remettre_en_brouillon: bool,
    /// La date du jour (Belgique), proposée par défaut.
    pub aujourdhui: String,
    /// La date du devis : on ne peut pas accepter ou refuser avant.
    pub date_devis: String,
    /// « Accepté le 15/10/2026 », « Refusé le 15/10/2026 : trop cher »…
    pub resume: String,
    /// Le chantier né de ce devis, ou celui que cet avenant corrige : (id, numéro).
    pub chantier: Option<(i64, i64)>,
    /// « Devis N° 2026-57 » ou « Avenant N° 2 ».
    pub titre: String,
    /// La liste des devis, ou le chantier pour un avenant.
    pub retour: String,
    /// Pour le titre de la page : « Modifier un devis - {référence} » (le numéro tant qu'il n'y a pas de référence).
    pub reference: String,
    pub brouillon: bool,
    pub avenant: bool,
    /// La pastille du statut (design de Raven).
    pub pastille: crate::composants::Pastille,
    /// Le nom du client, sous le titre (vide s'il n'est pas encore choisi).
    pub client: String,
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

pub async fn charger_barre(db: &mut PgConnection, id: i64) -> Result<Barre, Erreur> {
    let ligne: Option<(String, NaiveDate, NaiveDate, Option<NaiveDate>, Option<NaiveDate>, String)> = sqlx::query_as(
        "SELECT statut::text, date_devis, (now() AT TIME ZONE 'Europe/Brussels')::date, date_acceptation, date_refus, motif_refus
         FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let (statut_devis, date_devis, aujourdhui, acceptation, refus, motif) = ligne.ok_or_else(Erreur::introuvable)?;
    let resume = match (statut_devis.as_str(), acceptation, refus) {
        ("accepte", Some(d), _) => format!("Accepté le {}", date_belge(d)),
        ("refuse", _, Some(d)) if motif.is_empty() => format!("Refusé le {}", date_belge(d)),
        ("refuse", _, Some(d)) => format!("Refusé le {} : {motif}", date_belge(d)),
        // Signé en ligne par le client (étape 9-1) : il reste à l'accepter.
        ("signe", _, _) => {
            let signature: (Option<String>, String) = sqlx::query_as(
                "SELECT to_char(signe_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), signe_par FROM devis WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut *db)
            .await?;
            match signature {
                (Some(quand), par) => format!("Signé en ligne le {quand} par {par} : à accepter"),
                _ => String::new(),
            }
        }
        _ => String::new(),
    };
    let (numero, annee, chantier_avenant, reference): (i64, i32, Option<i64>, String) =
        sqlx::query_as("SELECT numero, extract(year FROM date_devis)::int, chantier_id, reference FROM devis WHERE id = $1").bind(id).fetch_one(&mut *db).await?;
    let avenant = chantier_avenant.is_some();
    let client: String = sqlx::query_scalar(
        "SELECT COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '')
         FROM devis d LEFT JOIN entreprises e ON e.id = d.client_entreprise_id LEFT JOIN contacts c ON c.id = d.client_contact_id
         WHERE d.id = $1",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await?;
    let chantier: Option<(i64, i64)> = sqlx::query_as("SELECT id, numero FROM chantiers WHERE devis_id = $1 OR id = $2")
        .bind(id)
        .bind(chantier_avenant)
        .fetch_optional(&mut *db)
        .await?;
    let titre = format!("{} N° {}", if avenant { "Avenant" } else { "Devis" }, crate::devis_lignes::numero_affiche(annee, numero, avenant));
    let retour = match (avenant, chantier) {
        (true, Some((chantier_id, _))) => format!("/chantiers/{chantier_id}/devis"),
        _ => "/devis".to_string(),
    };
    Ok(Barre {
        peut_marquer_envoye: Action::MarquerEnvoye.permise_depuis(&statut_devis),
        peut_accepter: Action::Accepter.permise_depuis(&statut_devis),
        peut_refuser: Action::Refuser.permise_depuis(&statut_devis),
        peut_remettre_en_brouillon: Action::RemettreEnBrouillon.permise_depuis(&statut_devis),
        aujourdhui: aujourdhui.to_string(),
        date_devis: date_devis.to_string(),
        resume,
        chantier,
        brouillon: statut_devis == "brouillon",
        avenant,
        pastille: crate::composants::pastille_devis(&statut_devis),
        client,
        reference,
        titre,
        retour,
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Decision {
    date: String,
    motif: String,
}

async fn changer(mut db: Base, id: i64, action: Action, decision: Decision) -> Result<Redirect, Erreur> {
    let retour = |code: &str| Ok(Redirect::to(&format!("/devis/{id}?{code}")));
    let mut tx = db.begin().await?;
    let ligne: Option<(String, NaiveDate, NaiveDate, bool)> = sqlx::query_as(
        "SELECT statut::text, date_devis, (now() AT TIME ZONE 'Europe/Brussels')::date, chantier_id IS NOT NULL
         FROM devis WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let (avant, date_devis, aujourdhui, avenant) = ligne.ok_or_else(Erreur::introuvable)?;
    if !action.permise_depuis(&avant) {
        return Err(Erreur::conflit("Ce changement de statut n'est pas possible pour ce devis."));
    }

    // Accepter ou refuser demande la date de la décision du client : pas avant le devis, pas dans le futur.
    let mut date = None;
    let motif = decision.motif.trim().to_string();
    if matches!(action, Action::Accepter | Action::Refuser) {
        let Some(d) = NaiveDate::parse_from_str(decision.date.trim(), "%Y-%m-%d").ok().filter(|d| *d >= date_devis && *d <= aujourdhui) else {
            return retour("erreur=date");
        };
        date = Some(d);
        if motif.chars().count() > LONGUEUR_MAX_MOTIF {
            return retour("erreur=motif");
        }
    }

    let vers = action.vers();
    let (date_acceptation, date_refus, motif_refus) = match action {
        Action::Accepter => (date, None, String::new()),
        Action::Refuser => (None, date, motif.clone()),
        // Les autres changements effacent la décision précédente (elle reste dans l'historique).
        _ => (None, None, String::new()),
    };
    sqlx::query(
        "UPDATE devis SET statut = $1::text::statut_devis, statut_date = now(), date_acceptation = $2, date_refus = $3,
                motif_refus = $4, updated_at = now()
         WHERE id = $5",
    )
    .bind(vers)
    .bind(date_acceptation)
    .bind(date_refus)
    .bind(&motif_refus)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    let mut detail = format!("{} → {}", statut(&avant).0, statut(vers).0);    if let Some(d) = date {
        detail.push_str(&format!(" (décision du {})", date_belge(d)));
    }
    if !motif_refus.is_empty() {
        detail.push_str(&format!(" : {motif_refus}"));
    }
    journal::noter(&mut tx, id, Genre::Statut, &detail).await?;

    // Accepter un devis crée le chantier dans la même transaction : jamais l'un sans l'autre.
    // Un avenant appartient déjà à son chantier : il ne crée rien.
    if let (Action::Accepter, Some(d), false) = (action, date, avenant) {
        let numero = crate::chantiers::creer(&mut tx, id, d).await?;
        journal::noter(&mut tx, id, Genre::Chantier, &format!("{} n° {numero} créé", crate::profil::mot("Chantier"))).await?;
    }
    tx.commit().await?;
    retour(&format!("ok={vers}"))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/devis/{id}/statut/envoye",
            post(|db: Base, Path(id): Path<i64>| changer(db, id, Action::MarquerEnvoye, Decision::default())),
        )
        .route(
            "/devis/{id}/statut/accepter",
            post(|db: Base, Path(id): Path<i64>, Form(d): Form<Decision>| changer(db, id, Action::Accepter, d)),
        )
        .route(
            "/devis/{id}/statut/refuser",
            post(|db: Base, Path(id): Path<i64>, Form(d): Form<Decision>| changer(db, id, Action::Refuser, d)),
        )
        .route(
            "/devis/{id}/statut/brouillon",
            post(|db: Base, Path(id): Path<i64>| changer(db, id, Action::RemettreEnBrouillon, Decision::default())),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOUS: [&str; 8] = ["brouillon", "a_envoyer", "envoi_en_cours", "envoye", "echec_envoi", "signe", "accepte", "refuse"];

    #[test]
    fn un_devis_accepte_ne_change_plus() {
        for a in [Action::MarquerEnvoye, Action::Accepter, Action::Refuser, Action::RemettreEnBrouillon] {
            assert!(!a.permise_depuis("accepte"), "{a:?}");
        }
    }

    #[test]
    fn ce_qu_un_brouillon_peut_faire() {
        assert!(Action::MarquerEnvoye.permise_depuis("brouillon"));
        assert!(Action::Accepter.permise_depuis("brouillon"));
        assert!(Action::Refuser.permise_depuis("brouillon"));
        assert!(!Action::RemettreEnBrouillon.permise_depuis("brouillon"));
    }

    #[test]
    fn un_devis_signe_ne_peut_qu_etre_accepte() {
        assert!(Action::Accepter.permise_depuis("signe"));
        assert!(!Action::Refuser.permise_depuis("signe"));
        assert!(!Action::RemettreEnBrouillon.permise_depuis("signe"));
    }

    #[test]
    fn remettre_en_brouillon_seulement_depuis_envoye_ou_refuse() {
        let permis: Vec<&str> = TOUS.into_iter().filter(|s| Action::RemettreEnBrouillon.permise_depuis(s)).collect();
        assert_eq!(permis, vec!["a_envoyer", "envoye", "echec_envoi", "refuse"]);
    }

    #[test]
    fn marquer_envoye_une_seule_fois() {
        assert!(!Action::MarquerEnvoye.permise_depuis("envoye"));
        assert!(!Action::MarquerEnvoye.permise_depuis("refuse"));
        assert!(Action::MarquerEnvoye.permise_depuis("echec_envoi"));
    }

    #[test]
    fn chaque_action_mene_a_un_statut_connu() {
        for a in [Action::MarquerEnvoye, Action::Accepter, Action::Refuser, Action::RemettreEnBrouillon] {
            assert!(TOUS.contains(&a.vers()), "{a:?}");
        }
    }
}
