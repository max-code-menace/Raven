//! Rappels de paiement (étape 8-3), **semi-automatiques** : l'application calcule chaque jour quelles factures en
//! retard sont à relancer et les montre (écran « Rappels de paiement », tableau de bord, fiche de la facture) ;
//! on envoie d'un clic, une par une ou toutes celles cochées. Rien ne part tout seul.
//!
//! Premier rappel : version douce (« Oubli de paiement ? ») ; les suivants : version ferme (« Rappel de paiement »).
//! Délais réglables dans Réglages › E-mails : premier rappel X jours après l'échéance, suivant Y jours après le précédent.

use askama::Template;
use axum::{
    Router,
    extract::{Path, Query},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use chrono::{Duration, NaiveDate};
use serde::Deserialize;
use sqlx::{FromRow, PgConnection};

use crate::base::Base;
use crate::commun::format_euros;
use crate::emails::{self, Issue};
use crate::envoi::{self, Saisie};
use crate::{AppState, Erreur, factures};

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// (premier rappel après l'échéance, rappel suivant après le précédent), en jours.
pub(crate) async fn delais(db: &mut PgConnection) -> Result<(i32, i32), sqlx::Error> {
    sqlx::query_as("SELECT rappel_premier_jours, rappel_suivant_jours FROM reglages WHERE id = 1").fetch_one(&mut *db).await
}

/// La date à laquelle le prochain rappel est dû.
pub(crate) fn prochain_rappel(echeance: NaiveDate, dernier: Option<NaiveDate>, premier: i32, suivant: i32) -> NaiveDate {
    match dernier {
        None => echeance + Duration::days(premier.into()),
        Some(d) => d + Duration::days(suivant.into()),
    }
}

/// Le modèle du prochain rappel : doux d'abord, ferme ensuite.
pub(crate) fn modele_du_rappel(deja_envoyes: i64) -> &'static str {
    if deja_envoyes == 0 { "rappel_doux" } else { "rappel_ferme" }
}

#[derive(FromRow)]
struct RetardLu {
    id: i64,
    numero: Option<i64>,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    reference: String,
    client: String,
    email: String,
    reste_cents: i64,
    jours_retard: i32,
    nombre: i64,
    dernier: Option<NaiveDate>,
}

/// Une facture en retard, telle que l'écran des rappels la montre.
pub struct Ligne {
    pub id: i64,
    pub numero: String,
    pub date: String,
    pub reference: String,
    pub client: String,
    pub email: String,
    pub reste: String,
    pub reste_cents: i64,
    pub jours_retard: i32,
    /// « 1 rappel, le 06/10/2026 » ou « Aucun ».
    pub rappels: String,
    pub prochain: String,
    /// Le prochain rappel est dû aujourd'hui (ou avant).
    pub a_relancer: bool,
    /// « Oubli de paiement ? » ou « Rappel de paiement ».
    pub prochain_modele: &'static str,
}

/// Toutes les factures en retard, celles à relancer d'abord.
pub(crate) async fn factures_en_retard(db: &mut PgConnection) -> Result<Vec<Ligne>, sqlx::Error> {
    let (premier, suivant) = delais(&mut *db).await?;
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await?;
    let lignes: Vec<RetardLu> = sqlx::query_as(
        "SELECT f.id, f.numero, f.date_facture, f.date_echeance, f.reference,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '') AS client,
                COALESCE(NULLIF(e.email, ''), NULLIF(c.email, ''), '') AS email,
                et.reste_cents, et.jours_retard, r.nombre, r.dernier
         FROM factures f
         JOIN factures_etats et ON et.facture_id = f.id
         LEFT JOIN entreprises e ON e.id = f.client_entreprise_id
         LEFT JOIN contacts c ON c.id = f.client_contact_id
         CROSS JOIN LATERAL (SELECT count(*) AS nombre, max((m.created_at AT TIME ZONE 'Europe/Brussels')::date) AS dernier
                             FROM emails m WHERE m.facture_id = f.id AND m.modele IN ('rappel_doux', 'rappel_ferme') AND m.statut <> 'echec') r
         WHERE et.etat = 'en_retard'
         ORDER BY f.date_echeance, f.numero",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut lignes: Vec<Ligne> = lignes
        .into_iter()
        .map(|b| {
            let echeance = b.date_echeance.unwrap_or(b.date_facture);
            let prochain = prochain_rappel(echeance, b.dernier, premier, suivant);
            let modele = modele_du_rappel(b.nombre);
            Ligne {
                id: b.id,
                numero: b.numero.map(|n| n.to_string()).unwrap_or_default(),
                date: date_belge(b.date_facture),
                reference: b.reference,
                client: b.client,
                email: b.email,
                reste: format_euros(b.reste_cents),
                reste_cents: b.reste_cents,
                jours_retard: b.jours_retard,
                rappels: match (b.nombre, b.dernier) {
                    (0, _) | (_, None) => "Aucun".to_string(),
                    (1, Some(d)) => format!("1 rappel, le {}", date_belge(d)),
                    (n, Some(d)) => format!("{n} rappels, le dernier le {}", date_belge(d)),
                },
                prochain: date_belge(prochain),
                a_relancer: prochain <= aujourdhui,
                prochain_modele: if modele == "rappel_doux" { "Oubli de paiement ?" } else { "Rappel de paiement" },
            }
        })
        .collect();
    lignes.sort_by_key(|l| !l.a_relancer);
    Ok(lignes)
}

/// Ce que la fiche d'une facture en retard affiche : le dernier rappel et le prochain.
pub(crate) async fn resume_facture(db: &mut PgConnection, facture_id: i64) -> Result<Option<(String, bool)>, sqlx::Error> {
    Ok(factures_en_retard(&mut *db).await?.into_iter().find(|l| l.id == facture_id).map(|l| {
        let texte = if l.rappels == "Aucun" {
            format!("Aucun rappel envoyé. Prochain rappel proposé le {} (« {} »).", l.prochain, l.prochain_modele)
        } else {
            format!("Rappels : {}. Prochain rappel proposé le {} (« {} »).", l.rappels, l.prochain, l.prochain_modele)
        };
        (texte, l.a_relancer)
    }))
}

// ─── L'écran ────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "rappels.html")]
struct Page {
    a_relancer: Vec<Ligne>,
    autres: Vec<Ligne>,
    total_a_relancer: String,
    premier: i32,
    suivant: i32,
    envoi_actif: bool,
    message_ok: String,
    message_erreur: &'static str,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    envoyes: String,
    enregistres: String,
    echecs: String,
    sans_email: String,
    erreur: String,
}

impl Retour {
    fn message(&self) -> String {
        let n = |t: &str| t.parse::<u32>().unwrap_or(0);
        let mut morceaux = Vec::new();
        if n(&self.envoyes) > 0 {
            morceaux.push(format!("{} rappel(s) envoyé(s)", n(&self.envoyes)));
        }
        if n(&self.enregistres) > 0 {
            morceaux.push(format!("{} rappel(s) enregistré(s) en mode test (rien n'est parti)", n(&self.enregistres)));
        }
        if n(&self.echecs) > 0 {
            morceaux.push(format!("{} échec(s) : voir Réglages › E-mails", n(&self.echecs)));
        }
        if n(&self.sans_email) > 0 {
            morceaux.push(format!("{} facture(s) sans e-mail du client : rien envoyé", n(&self.sans_email)));
        }
        if morceaux.is_empty() { String::new() } else { format!("{}.", morceaux.join(" · ")) }
    }
}

async fn afficher(mut db: Base, Query(r): Query<Retour>) -> Result<Html<String>, Erreur> {
    let (premier, suivant) = delais(&mut *db).await?;
    let (a_relancer, autres): (Vec<Ligne>, Vec<Ligne>) = factures_en_retard(&mut *db).await?.into_iter().partition(|l| l.a_relancer);
    let page = Page {
        total_a_relancer: format_euros(a_relancer.iter().map(|l| l.reste_cents).sum()),
        a_relancer,
        autres,
        premier,
        suivant,
        envoi_actif: emails::envoi_actif(),
        message_ok: r.message(),
        message_erreur: if r.erreur == "rien" { "Coche au moins une facture à relancer." } else { "" },
    };
    Ok(Html(page.render()?))
}

/// Le rappel prérempli d'une facture : (modèle, saisie).
async fn rappel_prerempli(db: &mut PgConnection, id: i64) -> Result<(&'static str, Saisie), Erreur> {
    let nombre: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM emails WHERE facture_id = $1 AND modele IN ('rappel_doux', 'rappel_ferme') AND statut <> 'echec'",
    )
    .bind(id)
    .fetch_one(&mut *db)
    .await?;
    let modele = modele_du_rappel(nombre);
    let (variables, email) = emails::variables_facture(&mut *db, id).await?;
    let (objet, corps) = emails::modele(&mut *db, modele).await?;
    Ok((modele, Saisie { destinataires: email, copie: String::new(), objet: emails::remplir(&objet, &variables), corps: emails::remplir(&corps, &variables) }))
}

/// La facture, si elle peut recevoir un rappel (validée, pas une note de crédit, quelque chose à recevoir).
async fn facture_a_rappeler(db: &mut PgConnection, id: i64) -> Result<factures::Facture, Erreur> {
    let f = factures::charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    let reste: i64 = sqlx::query_scalar("SELECT reste_cents FROM factures_etats WHERE facture_id = $1").bind(id).fetch_one(&mut *db).await?;
    if f.numero.is_none() || f.genre == "note_credit" || reste <= 0 {
        return Err(Erreur::conflit("Un rappel se fait sur une facture validée qui n'est pas encore payée."));
    }
    Ok(f)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Selection {
    facture: Vec<i64>,
}

/// « Envoyer les rappels cochés » : chaque facture reçoit son rappel prérempli (doux ou ferme).
async fn envoyer_selection(mut db: Base, Form(s): Form<Selection>) -> Result<Redirect, Erreur> {
    if s.facture.is_empty() {
        return Ok(Redirect::to("/factures/rappels?erreur=rien"));
    }
    let (mut envoyes, mut enregistres, mut echecs, mut sans_email) = (0, 0, 0, 0);
    for id in s.facture {
        let Ok(f) = facture_a_rappeler(&mut *db, id).await else { continue };
        let (modele, saisie) = rappel_prerempli(&mut *db, id).await?;
        let Some(destinataires) = emails::lire_adresses(&saisie.destinataires).filter(|l| !l.is_empty()) else {
            sans_email += 1;
            continue;
        };
        match envoi::envoyer_piece(&mut *db, &f, modele, destinataires, Vec::new(), saisie.objet, saisie.corps).await? {
            Issue::Envoye => envoyes += 1,
            Issue::Enregistre => enregistres += 1,
            Issue::Echec => echecs += 1,
        }
    }
    Ok(Redirect::to(&format!("/factures/rappels?envoyes={envoyes}&enregistres={enregistres}&echecs={echecs}&sans_email={sans_email}")))
}

async fn page_rappel(db: &mut PgConnection, f: &factures::Facture, saisie: Saisie, erreurs: Vec<String>) -> Result<Html<String>, Erreur> {
    let titre = factures::titre(&f.genre, f.numero, f.date_facture);
    let page = envoi::Page {
        titre: format!("Envoyer un rappel - {titre}"),
        retour: format!("/factures/{}", f.id),
        action: format!("/factures/{}/rappel", f.id),
        nav: "rappels",
        saisie,
        pieces: vec![format!("{titre} (PDF, conditions générales comprises)")],
        avertissements: Vec::new(),
        erreurs,
        envoi_actif: emails::envoi_actif(),
        anciens: envoi::anciens(&mut *db, None, Some(f.id)).await?,
    };
    Ok(Html(page.render()?))
}

async fn formulaire(mut db: Base, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let f = facture_a_rappeler(&mut *db, id).await?;
    let (_, saisie) = rappel_prerempli(&mut *db, id).await?;
    page_rappel(&mut *db, &f, saisie, Vec::new()).await
}

async fn envoyer_un(mut db: Base, Path(id): Path<i64>, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let f = facture_a_rappeler(&mut *db, id).await?;
    let (destinataires, copie) = match envoi::verifier(&s) {
        Ok(v) => v,
        Err(erreurs) => return Ok(page_rappel(&mut *db, &f, s, erreurs).await?.into_response()),
    };
    let (modele, _) = rappel_prerempli(&mut *db, id).await?;
    let issue = envoi::envoyer_piece(&mut *db, &f, modele, destinataires, copie, s.objet.trim().to_string(), s.corps).await?;
    let suite = match issue {
        Issue::Envoye => "ok=rappel_envoye",
        Issue::Enregistre => "ok=email_enregistre",
        Issue::Echec => "erreur=email_echec",
    };
    Ok(Redirect::to(&format!("/factures/{id}?{suite}")).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Delais {
    premier: String,
    suivant: String,
}

async fn enregistrer_delais(mut db: Base, Form(d): Form<Delais>) -> Result<Redirect, Erreur> {
    let lire = |t: &str| t.trim().parse::<i32>().ok().filter(|n| (1..=365).contains(n));
    let (Some(premier), Some(suivant)) = (lire(&d.premier), lire(&d.suivant)) else {
        return Ok(Redirect::to("/parametres/emails?erreur=delais#rappels"));
    };
    sqlx::query("UPDATE reglages SET rappel_premier_jours = $1, rappel_suivant_jours = $2, updated_at = now() WHERE id = 1")
        .bind(premier)
        .bind(suivant)
        .execute(&mut *db)
        .await?;
    Ok(Redirect::to("/parametres/emails?ok=delais#rappels"))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/factures/rappels", get(afficher).post(envoyer_selection))
        .route("/factures/{id}/rappel", get(formulaire).post(envoyer_un))
        .route("/parametres/rappels", post(enregistrer_delais))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    #[test]
    fn le_premier_rappel_suit_l_echeance_les_suivants_le_precedent() {
        // L'exemple de la planche : échue le 22/09, premier rappel 14 jours plus tard → 06/10.
        assert_eq!(prochain_rappel(jour(2026, 9, 22), None, 14, 14), jour(2026, 10, 6));
        assert_eq!(prochain_rappel(jour(2026, 9, 22), None, 7, 14), jour(2026, 9, 29));
        assert_eq!(prochain_rappel(jour(2026, 9, 22), Some(jour(2026, 10, 6)), 7, 14), jour(2026, 10, 20));
    }

    #[test]
    fn doux_puis_ferme() {
        assert_eq!(modele_du_rappel(0), "rappel_doux");
        assert_eq!(modele_du_rappel(1), "rappel_ferme");
        assert_eq!(modele_du_rappel(5), "rappel_ferme");
    }

    #[test]
    fn le_message_apres_envoi() {
        let r = Retour { envoyes: "2".into(), sans_email: "1".into(), ..Default::default() };
        assert_eq!(r.message(), "2 rappel(s) envoyé(s) · 1 facture(s) sans e-mail du client : rien envoyé.");
        assert_eq!(Retour::default().message(), "");
    }
}
