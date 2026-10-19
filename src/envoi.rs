//! Envoyer un devis, un avenant, une facture ou une note de crédit par e-mail (étape 8-2), et l'écran
//! Réglages › E-mails (modèles modifiables, état de l'envoi, journal des e-mails).
//!
//! L'e-mail est prérempli depuis le modèle ; on peut tout changer avant d'envoyer. Le PDF est joint.
//! Envoyé : le devis passe à « Envoyé », la facture à « Envoyée ». Sans clé Brevo : seulement enregistré.

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use sqlx::PgPool;

use crate::devis_statut::Action;
use crate::emails::{self, Issue, Message};
use crate::journal::{self, Genre};
use crate::{AppState, Erreur, factures, paiements};

// ─── Le formulaire d'envoi ──────────────────────────────────────────────────

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct Saisie {
    pub(crate) destinataires: String,
    pub(crate) copie: String,
    pub(crate) objet: String,
    pub(crate) corps: String,
}

pub struct Ancien {
    pub quand: String,
    pub destinataires: String,
    pub objet: String,
    pub statut: &'static str,
    pub classe: &'static str,
}

#[derive(Template)]
#[template(path = "email.html")]
pub(crate) struct Page {
    pub(crate) titre: String,
    pub(crate) retour: String,
    pub(crate) action: String,
    pub(crate) nav: &'static str,
    pub(crate) saisie: Saisie,
    pub(crate) pieces: Vec<String>,
    pub(crate) avertissements: Vec<String>,
    pub(crate) erreurs: Vec<String>,
    pub(crate) envoi_actif: bool,
    pub(crate) anciens: Vec<Ancien>,
}

pub(crate) fn statut_email(code: &str) -> (&'static str, &'static str) {
    match code {
        "envoye" => ("Envoyé", "s-accepte"),
        "echec" => ("Échec", "s-echec"),
        _ => ("Enregistré (non envoyé)", "s-brouillon"),
    }
}

pub(crate) async fn anciens(db: &PgPool, devis: Option<i64>, facture: Option<i64>) -> Result<Vec<Ancien>, sqlx::Error> {
    let lignes: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT to_char(created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), destinataires, objet, statut
         FROM emails WHERE ($1::bigint IS NOT NULL AND devis_id = $1) OR ($2::bigint IS NOT NULL AND facture_id = $2)
         ORDER BY created_at DESC, id DESC LIMIT 20",
    )
    .bind(devis)
    .bind(facture)
    .fetch_all(db)
    .await?;
    Ok(lignes
        .into_iter()
        .map(|(quand, destinataires, objet, statut)| {
            let (statut, classe) = statut_email(&statut);
            Ancien { quand, destinataires, objet, statut, classe }
        })
        .collect())
}

/// Vérifie la saisie : (destinataires, copie) ou la liste des erreurs.
pub(crate) fn verifier(s: &Saisie) -> Result<(Vec<String>, Vec<String>), Vec<String>> {
    let mut erreurs = Vec::new();
    let destinataires = emails::lire_adresses(&s.destinataires);
    match &destinataires {
        None => erreurs.push("Une adresse du destinataire n'est pas valable.".to_string()),
        Some(liste) if liste.is_empty() => erreurs.push("Indique au moins un destinataire.".to_string()),
        _ => {}
    }
    let copie = emails::lire_adresses(&s.copie);
    if copie.is_none() {
        erreurs.push("Une adresse en copie n'est pas valable.".to_string());
    }
    if s.objet.trim().is_empty() || s.objet.chars().count() > emails::LONGUEUR_MAX_OBJET {
        erreurs.push("L'objet est obligatoire (300 caractères au maximum).".to_string());
    }
    if s.corps.chars().count() > emails::LONGUEUR_MAX_CORPS {
        erreurs.push("Le message est trop long (20 000 caractères au maximum).".to_string());
    }
    if erreurs.is_empty() { Ok((destinataires.unwrap_or_default(), copie.unwrap_or_default())) } else { Err(erreurs) }
}

// ─── Devis et avenants ──────────────────────────────────────────────────────

/// (statut, avenant, titre « Devis N° 2026-57 »).
async fn infos_devis(db: &PgPool, id: i64) -> Result<(String, bool, String), Erreur> {
    let ligne: Option<(String, Option<i64>, i64, i32)> =
        sqlx::query_as("SELECT statut::text, chantier_id, numero, extract(year FROM date_devis)::int FROM devis WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    let (statut, chantier, numero, annee) = ligne.ok_or_else(Erreur::introuvable)?;
    let avenant = chantier.is_some();
    let titre = format!("{} N° {}", if avenant { "Avenant" } else { "Devis" }, crate::devis_lignes::numero_affiche(annee, numero, avenant));
    Ok((statut, avenant, titre))
}

async fn page_devis(db: &PgPool, id: i64, saisie: Option<Saisie>, erreurs: Vec<String>) -> Result<Html<String>, Erreur> {
    let (_, avenant, titre) = infos_devis(db, id).await?;
    let saisie = match saisie {
        Some(s) => s,
        None => {
            let (variables, email) = emails::variables_devis(db, id).await?;
            let (objet, corps) = emails::modele(db, if avenant { "avenant" } else { "devis" }).await?;
            Saisie { destinataires: email, copie: String::new(), objet: emails::remplir(&objet, &variables), corps: emails::remplir(&corps, &variables) }
        }
    };
    let page = Page {
        titre: format!("Envoyer par e-mail - {titre}"),
        retour: format!("/devis/{id}/apercu"),
        action: format!("/devis/{id}/email"),
        nav: if avenant { "chantiers" } else { "devis" },
        saisie,
        pieces: vec![format!("{titre} (PDF, conditions générales comprises)")],
        avertissements: crate::devis_pdf::charger_donnees(db, id).await?.a_completer(),
        erreurs,
        envoi_actif: emails::envoi_actif(),
        anciens: anciens(db, Some(id), None).await?,
    };
    Ok(Html(page.render()?))
}

async fn formulaire_devis(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    page_devis(&state.db, id, None, Vec::new()).await
}

async fn envoyer_devis(State(state): State<AppState>, Path(id): Path<i64>, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let (_, avenant, _) = infos_devis(&state.db, id).await?;
    let (destinataires, copie) = match verifier(&s) {
        Ok(v) => v,
        Err(erreurs) => return Ok(page_devis(&state.db, id, Some(s), erreurs).await?.into_response()),
    };
    let (nom, pdf) = crate::devis_pdf::pdf_du_devis(&state.db, id).await?;
    let a_qui = destinataires.join(", ");
    let (_, issue) = emails::envoyer(
        &state.db,
        Message {
            modele: if avenant { "avenant" } else { "devis" },
            devis_id: Some(id),
            facture_id: None,
            destinataires,
            copie,
            objet: s.objet.trim().to_string(),
            corps: s.corps,
            pieces: vec![(nom, pdf)],
        },
    )
    .await?;

    let mut tx = state.db.begin().await?;
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let detail = match issue {
        Issue::Envoye => format!("E-mail envoyé à {a_qui}"),
        Issue::Enregistre => format!("E-mail enregistré pour {a_qui} (mode test : rien n'est parti)"),
        Issue::Echec => format!("Échec de l'envoi à {a_qui}"),
    };
    journal::noter(&mut tx, id, Genre::Email, &detail).await?;
    let nouveau = match issue {
        Issue::Envoye if Action::MarquerEnvoye.permise_depuis(&statut) => Some("envoye"),
        Issue::Echec if matches!(statut.as_str(), "brouillon" | "a_envoyer") => Some("echec_envoi"),
        _ => None,
    };
    if let Some(vers) = nouveau {
        sqlx::query("UPDATE devis SET statut = $1::text::statut_devis, statut_date = now(), updated_at = now() WHERE id = $2")
            .bind(vers)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let detail = format!("{} → {} (e-mail)", crate::devis::statut(&statut).0, crate::devis::statut(vers).0);
        journal::noter(&mut tx, id, Genre::Statut, &detail).await?;
    }
    tx.commit().await?;
    let suite = match issue {
        Issue::Envoye => "ok=email_envoye",
        Issue::Enregistre => "ok=email_enregistre",
        Issue::Echec => "erreur=email_echec",
    };
    Ok(Redirect::to(&format!("/devis/{id}?{suite}")).into_response())
}

// ─── Factures et notes de crédit ────────────────────────────────────────────

async fn facture_validee(db: &PgPool, id: i64) -> Result<factures::Facture, Erreur> {
    let f = factures::charger(db, id).await?.ok_or_else(Erreur::introuvable)?;
    if f.numero.is_none() {
        return Err(Erreur::conflit("Valide d'abord la facture : un brouillon ne s'envoie pas."));
    }
    Ok(f)
}

async fn page_facture(db: &PgPool, f: &factures::Facture, saisie: Option<Saisie>, erreurs: Vec<String>) -> Result<Html<String>, Erreur> {
    let titre = factures::titre(&f.genre, f.numero, f.date_facture);
    let saisie = match saisie {
        Some(s) => s,
        None => {
            let (variables, email) = emails::variables_facture(db, f.id).await?;
            let code = if f.genre == "note_credit" { "note_credit" } else { "facture" };
            let (objet, corps) = emails::modele(db, code).await?;
            Saisie { destinataires: email, copie: String::new(), objet: emails::remplir(&objet, &variables), corps: emails::remplir(&corps, &variables) }
        }
    };
    let page = Page {
        titre: format!("Envoyer par e-mail - {titre}"),
        retour: format!("/factures/{}", f.id),
        action: format!("/factures/{}/email", f.id),
        nav: if f.genre == "note_credit" { "notes" } else { "factures" },
        saisie,
        pieces: vec![format!("{titre} (PDF, conditions générales comprises)")],
        avertissements: Vec::new(),
        erreurs,
        envoi_actif: emails::envoi_actif(),
        anciens: anciens(db, None, Some(f.id)).await?,
    };
    Ok(Html(page.render()?))
}

async fn formulaire_facture(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let f = facture_validee(&state.db, id).await?;
    page_facture(&state.db, &f, None, Vec::new()).await
}

/// Envoie une facture (ou un rappel) : PDF joint, journal, statut « Envoyée » si l'e-mail est parti.
pub(crate) async fn envoyer_piece(
    db: &PgPool,
    f: &factures::Facture,
    modele: &'static str,
    destinataires: Vec<String>,
    copie: Vec<String>,
    objet: String,
    corps: String,
) -> Result<Issue, Erreur> {
    let (nom, pdf) = crate::archive::pdf(db, f).await?;
    let a_qui = destinataires.join(", ");
    let (_, issue) = emails::envoyer(
        db,
        Message { modele, devis_id: None, facture_id: Some(f.id), destinataires, copie, objet, corps, pieces: vec![(nom, pdf)] },
    )
    .await?;
    let rappel = modele.starts_with("rappel");
    let quoi = if rappel { "Rappel" } else { "E-mail" };
    let detail = match issue {
        Issue::Envoye => format!("{quoi} envoyé à {a_qui}"),
        Issue::Enregistre => format!("{quoi} enregistré pour {a_qui} (mode test : rien n'est parti)"),
        Issue::Echec => format!("Échec de l'envoi à {a_qui}"),
    };
    let mut tx = db.begin().await?;
    paiements::noter(&mut tx, f.id, if rappel { "rappel" } else { "email" }, &detail).await?;
    if issue == Issue::Envoye {
        factures::marquer_envoyee_dans(&mut tx, f.id, "Envoyée par e-mail").await?;
    }
    tx.commit().await?;
    Ok(issue)
}

async fn envoyer_facture(State(state): State<AppState>, Path(id): Path<i64>, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let f = facture_validee(&state.db, id).await?;
    let (destinataires, copie) = match verifier(&s) {
        Ok(v) => v,
        Err(erreurs) => return Ok(page_facture(&state.db, &f, Some(s), erreurs).await?.into_response()),
    };
    let modele = if f.genre == "note_credit" { "note_credit" } else { "facture" };
    let issue = envoyer_piece(&state.db, &f, modele, destinataires, copie, s.objet.trim().to_string(), s.corps).await?;
    let suite = match issue {
        Issue::Envoye => "ok=email_envoye",
        Issue::Enregistre => "ok=email_enregistre",
        Issue::Echec => "erreur=email_echec",
    };
    Ok(Redirect::to(&format!("/factures/{id}?{suite}")).into_response())
}

// ─── Réglages › E-mails ─────────────────────────────────────────────────────

pub struct ModeleVue {
    pub code: String,
    pub nom: String,
    pub objet: String,
    pub corps: String,
    pub ouvert: bool,
}

pub struct EmailVue {
    pub quand: String,
    pub modele: String,
    pub destinataires: String,
    pub objet: String,
    pub corps: String,
    pub pieces: String,
    pub statut: &'static str,
    pub classe: &'static str,
    pub erreur: String,
    pub lien: String,
}

#[derive(Template)]
#[template(path = "reglages_emails.html")]
struct PageReglages {
    rappel_premier: i32,
    rappel_suivant: i32,
    envoi_actif: bool,
    expediteur: String,
    adresse_publique: String,
    modeles: Vec<ModeleVue>,
    variables: Vec<(&'static str, &'static str)>,
    journal: Vec<EmailVue>,
    message_ok: &'static str,
    message_erreur: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RetourReglages {
    ok: String,
    erreur: String,
    modele: String,
}

async fn reglages(State(state): State<AppState>, Query(r): Query<RetourReglages>) -> Result<Html<String>, Erreur> {
    let modeles: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT code, nom, objet, corps FROM modeles_email ORDER BY position").fetch_all(&state.db).await?;
    let journal: Vec<(String, String, String, String, String, Vec<String>, String, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT to_char(created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI'), modele, destinataires, objet, corps, pieces,
                statut, erreur, devis_id, facture_id
         FROM emails ORDER BY created_at DESC, id DESC LIMIT 50",
    )
    .fetch_all(&state.db)
    .await?;
    let noms: Vec<(String, String)> = modeles.iter().map(|(c, n, _, _)| (c.clone(), n.clone())).collect();
    let (rappel_premier, rappel_suivant) = crate::rappels::delais(&state.db).await?;
    let (nom_societe, email_societe, _) = emails::societe(&state.db).await?;
    let page = PageReglages {
        envoi_actif: emails::envoi_actif(),
        expediteur: if email_societe.is_empty() { String::new() } else { format!("{nom_societe} <{email_societe}>") },
        adresse_publique: emails::adresse_publique(),
        modeles: modeles
            .into_iter()
            .map(|(code, nom, objet, corps)| ModeleVue { ouvert: code == r.modele, code, nom, objet, corps })
            .collect(),
        variables: emails::VARIABLES.to_vec(),
        journal: journal
            .into_iter()
            .map(|(quand, modele, destinataires, objet, corps, pieces, statut, erreur, devis, facture)| {
                let (statut, classe) = statut_email(&statut);
                EmailVue {
                    quand,
                    modele: noms.iter().find(|(c, _)| *c == modele).map(|(_, n)| n.clone()).unwrap_or(modele),
                    destinataires,
                    objet,
                    corps,
                    pieces: pieces.join(", "),
                    statut,
                    classe,
                    erreur,
                    lien: match (devis, facture) {
                        (Some(d), _) => format!("/devis/{d}"),
                        (_, Some(f)) => format!("/factures/{f}"),
                        _ => String::new(),
                    },
                }
            })
            .collect(),
        message_ok: match r.ok.as_str() {
            "modele" => "Le modèle est enregistré.",
            "delais" => "Les délais des rappels sont enregistrés.",
            _ => "",
        },
        message_erreur: match r.erreur.as_str() {
            "modele" => "Le modèle n'est pas enregistré : l'objet est obligatoire (300 caractères), le message 20 000 caractères au maximum.",
            "delais" => "Les délais ne sont pas enregistrés : un nombre de jours entre 1 et 365.",
            _ => "",
        },
        rappel_premier,
        rappel_suivant,
    };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaisieModele {
    objet: String,
    corps: String,
}

async fn enregistrer_modele(State(state): State<AppState>, Path(code): Path<String>, Form(m): Form<SaisieModele>) -> Result<Redirect, Erreur> {
    let existe: Option<String> = sqlx::query_scalar("SELECT code FROM modeles_email WHERE code = $1").bind(&code).fetch_optional(&state.db).await?;
    let code = existe.ok_or_else(Erreur::introuvable)?;
    let objet = m.objet.trim();
    if objet.is_empty() || objet.chars().count() > emails::LONGUEUR_MAX_OBJET || m.corps.chars().count() > emails::LONGUEUR_MAX_CORPS {
        return Ok(Redirect::to(&format!("/parametres/emails?erreur=modele&modele={code}#modele-{code}")));
    }
    sqlx::query("UPDATE modeles_email SET objet = $1, corps = $2, updated_at = now() WHERE code = $3")
        .bind(objet)
        .bind(m.corps.replace("\r\n", "\n"))
        .bind(&code)
        .execute(&state.db)
        .await?;
    Ok(Redirect::to(&format!("/parametres/emails?ok=modele&modele={code}#modele-{code}")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devis/{id}/email", get(formulaire_devis).post(envoyer_devis))
        .route("/factures/{id}/email", get(formulaire_facture).post(envoyer_facture))
        .route("/parametres/emails", get(reglages))
        .route("/parametres/emails/{code}", post(enregistrer_modele))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saisie(a: &str, cc: &str, objet: &str) -> Saisie {
        Saisie { destinataires: a.into(), copie: cc.into(), objet: objet.into(), corps: "Bonjour".into() }
    }

    #[test]
    fn la_saisie_est_verifiee() {
        assert_eq!(verifier(&saisie("a@b.be", "", "Devis")), Ok((vec!["a@b.be".to_string()], vec![])));
        assert!(verifier(&saisie("", "", "Devis")).is_err());
        assert!(verifier(&saisie("a@b", "", "Devis")).is_err());
        assert!(verifier(&saisie("a@b.be", "x", "Devis")).is_err());
        assert!(verifier(&saisie("a@b.be", "", "  ")).is_err());
    }
}
