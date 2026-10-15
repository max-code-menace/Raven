//! Éditeur de devis, onglet Visualisation (étape 5b-3) : aperçu du devis tel qu'il sera envoyé.
//! C'est une page HTML ; le vrai PDF arrive à l'étape 5c et reprendra cette mise en page.

use askama::Template;
use axum::{
    Router,
    extract::{Path, State},
    response::Html,
    routing::get,
};
use chrono::NaiveDate;
use sqlx::{FromRow, PgPool};

use crate::commun::{PAYS, libelle};
use crate::devis_lignes::{Encodage, charger_encodage, charger_entete};
use crate::fiches::{Genre, Options};
use crate::{AppState, Erreur};

/// Un nom et une adresse, prêts à afficher.
pub struct Bloc {
    pub nom: String,
    pub adresse: Vec<String>,
    pub tva: String,
}

pub struct Societe {
    pub bloc: Bloc,
    pub email: String,
    pub telephone: String,
    pub iban: String,
    pub logo: Option<i64>,
}

/// « Rue 12 », « 1000 Bruxelles », puis le pays s'il n'est pas la Belgique.
fn lignes_adresse(rue: &str, numero: &str, code_postal: &str, ville: &str, pays: &str) -> Vec<String> {
    let rue = format!("{rue} {numero}").trim().to_string();
    let ville = format!("{code_postal} {ville}").trim().to_string();
    let mut lignes: Vec<String> = [rue, ville].into_iter().filter(|s| !s.is_empty()).collect();
    if !lignes.is_empty() && pays != "BE" && !pays.is_empty() {
        lignes.push(libelle(&PAYS, pays));
    }
    lignes
}

type Adresse = (String, String, String, String, String, String, String);

fn depuis_adresse((nom, rue, numero, code_postal, ville, pays, tva): Adresse) -> Bloc {
    Bloc { nom, adresse: lignes_adresse(&rue, &numero, &code_postal, &ville, &pays), tva }
}

/// L'entreprise ou le contact choisi ; `facturation` demande son adresse de facturation
/// (qui est l'adresse normale quand « facturation identique » est coché sur sa fiche).
async fn charger_bloc(db: &PgPool, entreprise: Option<i64>, contact: Option<i64>, facturation: bool) -> Result<Option<Bloc>, sqlx::Error> {
    let champs = |nom: &str| {
        let colonne = |c: &str| {
            if facturation {
                format!("CASE WHEN facturation_identique THEN {c} ELSE COALESCE(facturation_{c}, '') END")
            } else {
                c.to_string()
            }
        };
        format!(
            "SELECT {nom}, {}, {}, {}, {}, {}, COALESCE(numero_tva, '')",
            colonne("rue"),
            colonne("numero"),
            colonne("code_postal"),
            colonne("ville"),
            colonne("pays")
        )
    };
    if let Some(id) = entreprise {
        let sql = format!("{} FROM entreprises WHERE id = $1", champs("nom"));
        let ligne: Option<Adresse> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(db).await?;
        return Ok(ligne.map(depuis_adresse));
    }
    if let Some(id) = contact {
        let sql = format!("{} FROM contacts WHERE id = $1", champs("trim(concat_ws(' ', prenom, nom))"));
        let ligne: Option<Adresse> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(db).await?;
        return Ok(ligne.map(depuis_adresse));
    }
    Ok(None)
}

async fn charger_societe(db: &PgPool) -> Result<Option<Societe>, sqlx::Error> {
    let ligne: Option<(String, String, String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT nom, rue, numero, code_postal, ville, pays, COALESCE(numero_tva, ''), email, telephone
         FROM societe WHERE id = 1",
    )
    .fetch_optional(db)
    .await?;
    let Some((nom, rue, numero, code_postal, ville, pays, tva, email, telephone)) = ligne else {
        return Ok(None);
    };
    let iban: Option<String> =
        sqlx::query_scalar("SELECT iban FROM societe_comptes_bancaires WHERE par_defaut").fetch_optional(db).await?;
    let logo: Option<i64> = sqlx::query_scalar("SELECT extract(epoch FROM updated_at)::bigint FROM societe_fichiers WHERE usage = 'logo'")
        .fetch_optional(db)
        .await?;
    Ok(Some(Societe {
        bloc: Bloc { nom, adresse: lignes_adresse(&rue, &numero, &code_postal, &ville, &pays), tva },
        email,
        telephone,
        iban: iban.unwrap_or_default(),
        logo,
    }))
}

#[derive(FromRow)]
struct Tete {
    date_devis: NaiveDate,
    reference: String,
    responsable: String,
    client_entreprise_id: Option<i64>,
    client_contact_id: Option<i64>,
    facturation_identique: bool,
    facturation_entreprise_id: Option<i64>,
    facturation_contact_id: Option<i64>,
    chantier_identique: bool,
    chantier_rue: String,
    chantier_numero: String,
    chantier_code_postal: String,
    chantier_ville: String,
    chantier_pays: String,
    conditions_paiement_id: Option<i64>,
    remarque_1: String,
    remarque_2: String,
}

#[derive(Template)]
#[template(path = "devis_apercu.html")]
struct Page {
    id: i64,
    numero: String,
    statut: &'static str,
    classe_statut: &'static str,
    onglet: &'static str,
    date: String,
    reference: String,
    responsable: String,
    societe: Option<Societe>,
    client: Option<Bloc>,
    /// Présent seulement si le devis est facturé à quelqu'un d'autre que le client.
    facturation: Option<Bloc>,
    chantier: Vec<String>,
    conditions: String,
    remarque_1: String,
    remarque_2: String,
    enc: Encodage,
}

async fn afficher(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&state.db, id).await?;
    let tete: Tete = sqlx::query_as(
        "SELECT date_devis, reference, responsable, client_entreprise_id, client_contact_id, facturation_identique,
                facturation_entreprise_id, facturation_contact_id, chantier_identique, chantier_rue, chantier_numero,
                chantier_code_postal, chantier_ville, chantier_pays, conditions_paiement_id, remarque_1, remarque_2
         FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let client = charger_bloc(&state.db, tete.client_entreprise_id, tete.client_contact_id, false).await?;
    let facturation = if tete.facturation_identique {
        None
    } else {
        charger_bloc(&state.db, tete.facturation_entreprise_id, tete.facturation_contact_id, true).await?
    };
    let chantier = if tete.chantier_identique {
        client.as_ref().map(|c| c.adresse.clone()).unwrap_or_default()
    } else {
        lignes_adresse(&tete.chantier_rue, &tete.chantier_numero, &tete.chantier_code_postal, &tete.chantier_ville, &tete.chantier_pays)
    };
    let options = Options::charger(&state.db, Genre::Entreprise).await?;
    let conditions = tete
        .conditions_paiement_id
        .and_then(|cid| options.conditions.iter().find(|(i, _)| *i == cid).map(|(_, t)| t.clone()))
        .unwrap_or_default();
    let enc = charger_encodage(&state.db, id, &entete, String::new()).await?;

    let page = Page {
        id,
        numero: entete.numero,
        statut: entete.statut,
        classe_statut: entete.classe_statut,
        onglet: "visualisation",
        date: tete.date_devis.format("%d/%m/%Y").to_string(),
        reference: tete.reference,
        responsable: tete.responsable,
        societe: charger_societe(&state.db).await?,
        client,
        facturation,
        chantier,
        conditions,
        remarque_1: tete.remarque_1,
        remarque_2: tete.remarque_2,
        enc,
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/apercu", get(afficher))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresses_sur_deux_lignes_et_pays_etranger() {
        assert_eq!(lignes_adresse("Rue Neuve", "12", "1000", "Bruxelles", "BE"), vec!["Rue Neuve 12", "1000 Bruxelles"]);
        assert_eq!(lignes_adresse("Rue Neuve", "", "", "", "BE"), vec!["Rue Neuve"]);
        assert_eq!(lignes_adresse("Rue de Paris", "3", "75001", "Paris", "FR"), vec!["Rue de Paris 3", "75001 Paris", "France"]);
        // Une adresse vide reste vide, même avec un pays étranger.
        assert!(lignes_adresse("", "", "", "", "FR").is_empty());
    }
}
