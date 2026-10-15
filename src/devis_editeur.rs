//! Éditeur de devis, onglet Informations (étape 5b-1).
//! Les onglets Encodage, Totaux et Visualisation arrivent aux étapes suivantes.

use askama::Template;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::Form;
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgPool};

use crate::commun::{Choix, PAYS, choix_fixes, vide_en_none};
use crate::devis::statut;
use crate::fiches::{Genre, Options, choix_base};
use crate::{AppState, Erreur};

// ─── Client : une entreprise ou une personne ────────────────────────────────

/// Dans les menus, une entreprise s'écrit « e12 » et un contact « c5 ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Personne {
    Entreprise(i64),
    Contact(i64),
}

impl Personne {
    fn lire(texte: &str) -> Option<Self> {
        let texte = texte.trim();
        let (genre, numero) = texte.split_at_checked(1)?;
        let id: i64 = numero.parse().ok().filter(|n| *n > 0)?;
        match genre {
            "e" => Some(Personne::Entreprise(id)),
            "c" => Some(Personne::Contact(id)),
            _ => None,
        }
    }

    fn code(self) -> String {
        match self {
            Personne::Entreprise(id) => format!("e{id}"),
            Personne::Contact(id) => format!("c{id}"),
        }
    }

    /// (entreprise_id, contact_id) tels qu'ils sont enregistrés en base.
    fn colonnes(personne: Option<Self>) -> (Option<i64>, Option<i64>) {
        match personne {
            Some(Personne::Entreprise(id)) => (Some(id), None),
            Some(Personne::Contact(id)) => (None, Some(id)),
            None => (None, None),
        }
    }

    fn depuis(entreprise: Option<i64>, contact: Option<i64>) -> Option<Self> {
        entreprise.map(Personne::Entreprise).or(contact.map(Personne::Contact))
    }
}

// ─── Ce que l'écran propose dans ses menus ──────────────────────────────────

struct Referentiel {
    options: Options,
    personnes: Vec<(Personne, String)>,
    contacts: Vec<(i64, String)>,
}

impl Referentiel {
    async fn charger(db: &PgPool) -> Result<Self, sqlx::Error> {
        // Pour une entreprise, `Options` ne charge que les conditions de paiement et les taux de TVA.
        let options = Options::charger(db, Genre::Entreprise).await?;
        let entreprises: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, nom FROM entreprises ORDER BY lower(nom), id").fetch_all(db).await?;
        let contacts: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, trim(concat_ws(' ', prenom, nom)) FROM contacts ORDER BY lower(nom), id",
        )
        .fetch_all(db)
        .await?;

        let mut personnes: Vec<(Personne, String)> = entreprises
            .into_iter()
            .map(|(id, nom)| (Personne::Entreprise(id), format!("{nom} (entreprise)")))
            .collect();
        personnes.extend(contacts.iter().map(|(id, nom)| (Personne::Contact(*id), format!("{nom} (contact)"))));
        Ok(Referentiel { options, personnes, contacts })
    }

    fn existe(&self, personne: Personne) -> bool {
        self.personnes.iter().any(|(p, _)| *p == personne)
    }

    fn choix_personnes(&self, actuel: &str) -> Vec<Choix> {
        let actuel = Personne::lire(actuel).map(Personne::code).unwrap_or_default();
        self.personnes
            .iter()
            .map(|(p, libelle)| Choix { valeur: p.code(), libelle: libelle.clone(), choisi: p.code() == actuel })
            .collect()
    }
}

// ─── Le devis en base, et dans le formulaire ────────────────────────────────

#[derive(FromRow)]
struct Devis {
    numero: i64,
    annee: i32,
    date_devis: NaiveDate,
    reference: String,
    responsable: String,
    statut: String,
    client_entreprise_id: Option<i64>,
    client_contact_id: Option<i64>,
    taux_tva_id: Option<i64>,
    conditions_paiement_id: Option<i64>,
    facturation_identique: bool,
    facturation_entreprise_id: Option<i64>,
    facturation_contact_id: Option<i64>,
    chantier_identique: bool,
    chantier_rue: String,
    chantier_numero: String,
    chantier_code_postal: String,
    chantier_ville: String,
    chantier_pays: String,
    architecte_contact_id: Option<i64>,
    gestionnaire_contact_id: Option<i64>,
    remarque_1: String,
    remarque_2: String,
}

async fn charger_devis(db: &PgPool, id: i64) -> Result<Option<Devis>, sqlx::Error> {
    sqlx::query_as(
        "SELECT numero, extract(year FROM date_devis)::int AS annee, date_devis, reference, responsable,
                statut::text AS statut, client_entreprise_id, client_contact_id, taux_tva_id,
                conditions_paiement_id, facturation_identique, facturation_entreprise_id,
                facturation_contact_id, chantier_identique, chantier_rue, chantier_numero,
                chantier_code_postal, chantier_ville, chantier_pays, architecte_contact_id,
                gestionnaire_contact_id, remarque_1, remarque_2
         FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Ce que le navigateur envoie : tout en texte. Une case décochée n'envoie rien.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DevisForm {
    reference: String,
    responsable: String,
    date_devis: String,
    client: String,
    taux_tva_id: String,
    conditions_paiement_id: String,
    facturation_identique: Option<String>,
    facturation_client: String,
    chantier_identique: Option<String>,
    chantier_rue: String,
    chantier_numero: String,
    chantier_code_postal: String,
    chantier_ville: String,
    chantier_pays: String,
    architecte_id: String,
    gestionnaire_id: String,
    remarque_1: String,
    remarque_2: String,
}

fn id_en_texte(id: Option<i64>) -> String {
    id.map(|n| n.to_string()).unwrap_or_default()
}

impl From<&Devis> for DevisForm {
    fn from(d: &Devis) -> Self {
        let code = |p: Option<Personne>| p.map(Personne::code).unwrap_or_default();
        DevisForm {
            reference: d.reference.clone(),
            responsable: d.responsable.clone(),
            date_devis: d.date_devis.to_string(),
            client: code(Personne::depuis(d.client_entreprise_id, d.client_contact_id)),
            taux_tva_id: id_en_texte(d.taux_tva_id),
            conditions_paiement_id: id_en_texte(d.conditions_paiement_id),
            facturation_identique: d.facturation_identique.then(|| "on".to_string()),
            facturation_client: code(Personne::depuis(d.facturation_entreprise_id, d.facturation_contact_id)),
            chantier_identique: d.chantier_identique.then(|| "on".to_string()),
            chantier_rue: d.chantier_rue.clone(),
            chantier_numero: d.chantier_numero.clone(),
            chantier_code_postal: d.chantier_code_postal.clone(),
            chantier_ville: d.chantier_ville.clone(),
            chantier_pays: d.chantier_pays.clone(),
            architecte_id: id_en_texte(d.architecte_contact_id),
            gestionnaire_id: id_en_texte(d.gestionnaire_contact_id),
            remarque_1: d.remarque_1.clone(),
            remarque_2: d.remarque_2.clone(),
        }
    }
}

/// Le formulaire vérifié et nettoyé, prêt à être enregistré.
struct Valeurs {
    reference: String,
    responsable: String,
    date_devis: NaiveDate,
    client: Personne,
    taux_tva_id: i64,
    conditions_paiement_id: i64,
    facturation_identique: bool,
    facturation_client: Option<Personne>,
    chantier_identique: bool,
    chantier_rue: String,
    chantier_numero: String,
    chantier_code_postal: String,
    chantier_ville: String,
    chantier_pays: String,
    architecte_id: Option<i64>,
    gestionnaire_id: Option<i64>,
    remarque_1: String,
    remarque_2: String,
}

fn valider(f: &DevisForm, r: &Referentiel) -> Result<Valeurs, Vec<String>> {
    let mut erreurs = Vec::new();

    let reference = f.reference.trim().to_string();
    if reference.is_empty() {
        erreurs.push("La référence est obligatoire.".to_string());
    }
    let responsable = f.responsable.trim().to_string();
    if responsable.is_empty() {
        erreurs.push("Le responsable est obligatoire.".to_string());
    }
    let date_devis = NaiveDate::parse_from_str(f.date_devis.trim(), "%Y-%m-%d").ok();
    if date_devis.is_none() {
        erreurs.push("La date du devis est obligatoire.".to_string());
    }

    let client = Personne::lire(&f.client).filter(|p| r.existe(*p));
    if client.is_none() {
        erreurs.push("Choisis le client du devis.".to_string());
    }

    let taux_tva_id = f.taux_tva_id.trim().parse::<i64>().ok().filter(|id| r.options.taux_tva.iter().any(|(i, _)| i == id));
    if taux_tva_id.is_none() {
        erreurs.push("Le taux de TVA par défaut est obligatoire.".to_string());
    }
    let conditions_paiement_id = f
        .conditions_paiement_id
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| r.options.conditions.iter().any(|(i, _)| i == id));
    if conditions_paiement_id.is_none() {
        erreurs.push("Les conditions de paiement sont obligatoires.".to_string());
    }

    // Facturé à quelqu'un d'autre : il faut dire à qui.
    let facturation_identique = f.facturation_identique.is_some();
    let facturation_client = if facturation_identique {
        None
    } else {
        let p = Personne::lire(&f.facturation_client).filter(|p| r.existe(*p));
        if p.is_none() {
            erreurs.push("Choisis le client de facturation, ou coche « identique au client ».".to_string());
        }
        p
    };

    let chantier_identique = f.chantier_identique.is_some();
    if !f.chantier_pays.is_empty() && !PAYS.iter().any(|(v, _)| *v == f.chantier_pays) {
        erreurs.push("Ce pays n'existe pas.".to_string());
    }

    // Architecte et gestionnaire : facultatifs, mais ils doivent exister.
    let mut contact = |texte: &str, nom: &str| -> Option<i64> {
        let id = vide_en_none(texte)?;
        match id.parse::<i64>().ok().filter(|n| r.contacts.iter().any(|(i, _)| i == n)) {
            Some(n) => Some(n),
            None => {
                erreurs.push(format!("{nom} n'existe pas."));
                None
            }
        }
    };
    let architecte_id = contact(&f.architecte_id, "Cet architecte");
    let gestionnaire_id = contact(&f.gestionnaire_id, "Ce gestionnaire");

    let (Some(date_devis), Some(client), Some(taux_tva_id), Some(conditions_paiement_id)) =
        (date_devis, client, taux_tva_id, conditions_paiement_id)
    else {
        return Err(erreurs);
    };
    if !erreurs.is_empty() {
        return Err(erreurs);
    }

    // Adresse identique à celle du client : on n'enregistre pas de seconde adresse.
    let chantier = |s: &str| if chantier_identique { String::new() } else { s.trim().to_string() };
    Ok(Valeurs {
        reference,
        responsable,
        date_devis,
        client,
        taux_tva_id,
        conditions_paiement_id,
        facturation_identique,
        facturation_client,
        chantier_identique,
        chantier_rue: chantier(&f.chantier_rue),
        chantier_numero: chantier(&f.chantier_numero),
        chantier_code_postal: chantier(&f.chantier_code_postal),
        chantier_ville: chantier(&f.chantier_ville),
        chantier_pays: if f.chantier_pays.is_empty() { "BE".to_string() } else { f.chantier_pays.clone() },
        architecte_id,
        gestionnaire_id,
        remarque_1: f.remarque_1.trim().to_string(),
        remarque_2: f.remarque_2.trim().to_string(),
    })
}

/// Renvoie `false` si le devis n'est plus un brouillon (ou n'existe plus) : rien n'est modifié.
async fn enregistrer_en_base(db: &PgPool, id: i64, v: &Valeurs) -> Result<bool, sqlx::Error> {
    let (client_entreprise, client_contact) = Personne::colonnes(Some(v.client));
    let (facturation_entreprise, facturation_contact) = Personne::colonnes(v.facturation_client);
    let res = sqlx::query(
        "UPDATE devis SET reference = $1, responsable = $2, date_devis = $3,
                client_entreprise_id = $4, client_contact_id = $5, taux_tva_id = $6,
                conditions_paiement_id = $7, facturation_identique = $8,
                facturation_entreprise_id = $9, facturation_contact_id = $10,
                chantier_identique = $11, chantier_rue = $12, chantier_numero = $13,
                chantier_code_postal = $14, chantier_ville = $15, chantier_pays = $16,
                architecte_contact_id = $17, gestionnaire_contact_id = $18,
                remarque_1 = $19, remarque_2 = $20, updated_at = now()
         WHERE id = $21 AND statut = 'brouillon'",
    )
    .bind(&v.reference)
    .bind(&v.responsable)
    .bind(v.date_devis)
    .bind(client_entreprise)
    .bind(client_contact)
    .bind(v.taux_tva_id)
    .bind(v.conditions_paiement_id)
    .bind(v.facturation_identique)
    .bind(facturation_entreprise)
    .bind(facturation_contact)
    .bind(v.chantier_identique)
    .bind(&v.chantier_rue)
    .bind(&v.chantier_numero)
    .bind(&v.chantier_code_postal)
    .bind(&v.chantier_ville)
    .bind(&v.chantier_pays)
    .bind(v.architecte_id)
    .bind(v.gestionnaire_id)
    .bind(&v.remarque_1)
    .bind(&v.remarque_2)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

// ─── Page ───────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "devis_editeur.html")]
struct Page {
    id: i64,
    numero: String,
    statut: &'static str,
    classe_statut: &'static str,
    onglet: &'static str,
    /// Seul un brouillon se modifie : un devis envoyé ou accepté est en lecture seule.
    modifiable: bool,
    enregistre: bool,
    erreurs: Vec<String>,
    clients: Vec<Choix>,
    clients_facturation: Vec<Choix>,
    taux_tva: Vec<Choix>,
    conditions: Vec<Choix>,
    pays: Vec<Choix>,
    architectes: Vec<Choix>,
    gestionnaires: Vec<Choix>,
    form: DevisForm,
}

fn construire_page(id: i64, d: &Devis, form: DevisForm, erreurs: Vec<String>, enregistre: bool, r: &Referentiel) -> Page {
    let (libelle, classe) = statut(&d.statut);
    let contacts: Vec<(i64, String)> = r.contacts.clone();
    let pays = if form.chantier_pays.is_empty() { "BE".to_string() } else { form.chantier_pays.clone() };
    Page {
        id,
        numero: format!("{}-{}", d.annee, d.numero),
        statut: libelle,
        classe_statut: classe,
        onglet: "informations",
        modifiable: d.statut == "brouillon",
        enregistre,
        erreurs,
        clients: r.choix_personnes(&form.client),
        clients_facturation: r.choix_personnes(&form.facturation_client),
        taux_tva: choix_base(&r.options.taux_tva, &form.taux_tva_id),
        conditions: choix_base(&r.options.conditions, &form.conditions_paiement_id),
        pays: choix_fixes(&PAYS, &[pays]),
        architectes: choix_base(&contacts, &form.architecte_id),
        gestionnaires: choix_base(&contacts, &form.gestionnaire_id),
        form,
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
}

async fn afficher(State(state): State<AppState>, Path(id): Path<i64>, Query(retour): Query<Retour>) -> Result<Html<String>, Erreur> {
    let devis = charger_devis(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
    let referentiel = Referentiel::charger(&state.db).await?;
    let form = DevisForm::from(&devis);
    let page = construire_page(id, &devis, form, Vec::new(), !retour.ok.is_empty(), &referentiel);
    Ok(Html(page.render()?))
}

async fn enregistrer(State(state): State<AppState>, Path(id): Path<i64>, Form(form): Form<DevisForm>) -> Result<Response, Erreur> {
    let devis = charger_devis(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
    if devis.statut != "brouillon" {
        return Err(Erreur::conflit("Ce devis n'est plus un brouillon : il ne peut plus être modifié."));
    }
    let referentiel = Referentiel::charger(&state.db).await?;
    match valider(&form, &referentiel) {
        Ok(valeurs) => {
            if enregistrer_en_base(&state.db, id, &valeurs).await? {
                Ok(Redirect::to(&format!("/devis/{id}?ok=1")).into_response())
            } else {
                Err(Erreur::conflit("Ce devis n'est plus un brouillon : il ne peut plus être modifié."))
            }
        }
        Err(erreurs) => {
            let page = construire_page(id, &devis, form, erreurs, false, &referentiel);
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}", get(afficher).post(enregistrer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personnes_lues_dans_les_menus() {
        assert_eq!(Personne::lire("e12"), Some(Personne::Entreprise(12)));
        assert_eq!(Personne::lire(" c5 "), Some(Personne::Contact(5)));
        assert_eq!(Personne::lire(""), None);
        assert_eq!(Personne::lire("x3"), None);
        assert_eq!(Personne::lire("e"), None);
        assert_eq!(Personne::lire("e0"), None);
        assert_eq!(Personne::lire("e-4"), None);
        assert_eq!(Personne::lire("é1"), None);
        assert_eq!(Personne::Entreprise(7).code(), "e7");
        assert_eq!(Personne::Contact(9).code(), "c9");
    }

    #[test]
    fn client_range_dans_la_bonne_colonne() {
        assert_eq!(Personne::colonnes(Some(Personne::Entreprise(3))), (Some(3), None));
        assert_eq!(Personne::colonnes(Some(Personne::Contact(4))), (None, Some(4)));
        assert_eq!(Personne::colonnes(None), (None, None));
        assert_eq!(Personne::depuis(Some(3), None), Some(Personne::Entreprise(3)));
        assert_eq!(Personne::depuis(None, Some(4)), Some(Personne::Contact(4)));
        assert_eq!(Personne::depuis(None, None), None);
    }
}
