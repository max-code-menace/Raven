//! Éditeur de devis, onglet Totaux (étape 5b-3) : sous-total, ajustement (remise ou majoration),
//! TVA par taux et total TTC. Tous les montants sont calculés par le serveur.

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;

use sqlx::{Connection as _, PgConnection};
use crate::base::Base;
use crate::calcul::{self, Ajustement};
use crate::commun::format_euros;
use crate::composants::Enregistrement;
use crate::devis_statut::{Barre, charger_barre};
use crate::devis_lignes::{Entete, RentabiliteVue, TotauxVue, charger_encodage, charger_entete, recalculer};
use crate::{AppState, Erreur};

const MESSAGE_NON_MODIFIABLE: &str = "Ce devis n'est plus un brouillon : il ne peut plus être modifié.";
const LONGUEUR_MAX_LIBELLE: usize = 100;

/// Ce que le navigateur envoie. `valeur` est toujours positive : c'est `sens` qui dit remise ou majoration.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct AjustementForm {
    libelle: String,
    sens: String,
    mode: String,
    valeur: String,
}

impl AjustementForm {
    /// Le formulaire tel qu'il se présente quand l'ajustement enregistré est `a`.
    fn depuis(a: Ajustement, libelle: &str) -> Self {
        let (sens, mode, valeur) = match a {
            Ajustement::Aucun => ("remise", "pourcentage", String::new()),
            // centièmes de pour cent → e5 : × 1000 ; centimes → e5 : × 1000 aussi
            Ajustement::Pourcentage(v) => (if v < 0 { "remise" } else { "majoration" }, "pourcentage", calcul::ecrire_decimal_e5(v.abs() * 1000)),
            Ajustement::Montant(c) => (if c < 0 { "remise" } else { "majoration" }, "montant", calcul::ecrire_decimal_e5(c.abs() * 1000)),
        };
        AjustementForm { libelle: libelle.to_string(), sens: sens.to_string(), mode: mode.to_string(), valeur }
    }
}

/// Un ajustement vérifié : prêt à être enregistré.
struct Valide {
    libelle: String,
    mode: &'static str,
    valeur: i64,
}

/// `sous_total_cents` : somme des lignes, pour refuser une remise plus grande que le devis.
fn valider(f: &AjustementForm, sous_total_cents: i64) -> Result<Valide, String> {
    let libelle = f.libelle.trim().to_string();
    if libelle.chars().count() > LONGUEUR_MAX_LIBELLE {
        return Err(format!("Le libellé est trop long ({LONGUEUR_MAX_LIBELLE} caractères au maximum)."));
    }
    let remise = match f.sens.as_str() {
        "remise" => true,
        "majoration" => false,
        _ => return Err("Choisissez remise ou majoration.".to_string()),
    };
    let signe = if remise { -1 } else { 1 };
    let e5 = calcul::lire_decimal_e5(f.valeur.trim())
        .filter(|v| *v > 0)
        .ok_or("La valeur doit être un nombre supérieur à 0 (exemple : 5 ou 100,50).")?;
    // Au plus 2 décimales : un pourcentage au centième près, un montant au centime près.
    if e5 % 1000 != 0 {
        return Err("La valeur peut avoir 2 décimales au maximum.".to_string());
    }
    let valeur = e5 / 1000;
    match f.mode.as_str() {
        "pourcentage" => {
            if remise && valeur > 10_000 {
                return Err("Une remise ne peut pas dépasser 100 %.".to_string());
            }
            if valeur > 100_000 {
                return Err("Une majoration ne peut pas dépasser 1 000 %.".to_string());
            }
            Ok(Valide { libelle, mode: "pourcentage", valeur: signe * valeur })
        }
        "montant" => {
            if remise && valeur > sous_total_cents {
                return Err(format!("La remise dépasse le sous-total des lignes ({}).", format_euros(sous_total_cents)));
            }
            Ok(Valide { libelle, mode: "montant", valeur: signe * valeur })
        }
        _ => Err("Choisissez pourcentage ou montant.".to_string()),
    }
}

/// Le mode et la valeur enregistrés d'un ajustement.
fn colonnes(a: Ajustement) -> (&'static str, i64) {
    match a {
        Ajustement::Aucun => ("aucun", 0),
        Ajustement::Pourcentage(v) => ("pourcentage", v),
        Ajustement::Montant(c) => ("montant", c),
    }
}

/// Lit le formulaire pour l'enregistrement automatique : une valeur vide retire l'ajustement ; un libellé ou un
/// ajustement refusé garde celui d'avant (`avant`, `libelle_avant`) et donne un message.
fn lire(f: &AjustementForm, avant: Ajustement, libelle_avant: &str, sous_total_cents: i64) -> (Valide, Vec<String>) {
    let mut erreurs = Vec::new();
    let libelle = if f.libelle.trim().chars().count() > LONGUEUR_MAX_LIBELLE {
        erreurs.push(format!("Le libellé est trop long ({LONGUEUR_MAX_LIBELLE} caractères au maximum)."));
        libelle_avant.to_string()
    } else {
        f.libelle.trim().to_string()
    };
    let (mode, valeur) = if f.valeur.trim().is_empty() {
        ("aucun", 0)
    } else {
        match valider(&AjustementForm { libelle: String::new(), ..f.clone() }, sous_total_cents) {
            Ok(v) => (v.mode, v.valeur),
            Err(e) => {
                erreurs.push(e);
                colonnes(avant)
            }
        }
    };
    (Valide { libelle, mode, valeur }, erreurs)
}

// ─── Page ───────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "devis_totaux.html")]
struct Page {
    id: i64,
    onglet: &'static str,
    barre: Barre,
    modifiable: bool,
    a_un_ajustement: bool,
    erreur: String,
    form: AjustementForm,
    totaux: TotauxVue,
    /// Présent seulement quand l'option « prix d'achat, marge et frais généraux » est allumée.
    rentabilite: Option<RentabiliteVue>,
    /// La note de la barre du bas : l'ajustement s'enregistre tout seul.
    enregistrement: Enregistrement,
}

async fn construire(db: &mut PgConnection, id: i64, entete: Entete, form: Option<AjustementForm>, erreur: String) -> Result<Page, Erreur> {
    let enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    let form = form.unwrap_or_else(|| AjustementForm::depuis(entete.ajustement, &entete.ajustement_libelle));
    Ok(Page {
        id,
        onglet: "totaux",
        barre: charger_barre(&mut *db, id).await?,
        modifiable: entete.brouillon,
        a_un_ajustement: entete.ajustement != Ajustement::Aucun,
        erreur,
        form,
        totaux: enc.totaux,
        rentabilite: enc.rentabilite,
        enregistrement: Enregistrement::default(),
    })
}

async fn afficher(mut db: Base, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    Ok(Html(construire(&mut *db, id, entete, None, String::new()).await?.render()?))
}

/// Enregistre l'ajustement. Le formulaire s'envoie tout seul (htmx) à chaque changement : la réponse est la page
/// entière, dont htmx reprend la note de la barre du bas, les totaux et la rentabilité.
async fn enregistrer(mut db: Base, Path(id): Path<i64>, entetes: HeaderMap, Form(f): Form<AjustementForm>) -> Result<Response, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    if !entete.brouillon {
        return Err(Erreur::conflit(MESSAGE_NON_MODIFIABLE));
    }
    let sous_total = sous_total_cents(&mut *db, id).await?;
    let (v, erreurs) = lire(&f, entete.ajustement, &entete.ajustement_libelle, sous_total);
    changer_ajustement(&mut *db, id, &v.libelle, v.mode, v.valeur).await?;
    let entete = charger_entete(&mut *db, id).await?;
    if crate::composants::par_htmx(&entetes) {
        let mut page = construire(&mut *db, id, entete, Some(f), String::new()).await?;
        page.enregistrement = Enregistrement::maintenant(&mut *db, erreurs).await?;
        return Ok(Html(page.render()?).into_response());
    }
    if erreurs.is_empty() {
        return Ok(Redirect::to(&format!("/devis/{id}/totaux")).into_response());
    }
    let page = construire(&mut *db, id, entete, Some(f), erreurs.join(" ")).await?;
    Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
}

async fn retirer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    if !entete.brouillon {
        return Err(Erreur::conflit(MESSAGE_NON_MODIFIABLE));
    }
    changer_ajustement(&mut *db, id, "", "aucun", 0).await?;
    Ok(Redirect::to(&format!("/devis/{id}/totaux")))
}

async fn sous_total_cents(db: &mut PgConnection, id: i64) -> Result<i64, Erreur> {
    // Les lignes sans ajustement : on demande les totaux à un devis « vu sans ajustement ».
    let mut entete = charger_entete(&mut *db, id).await?;
    entete.ajustement = Ajustement::Aucun;
    let enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    Ok(enc.totaux.sous_total_cents)
}

/// Enregistre l'ajustement et les nouveaux totaux dans la même transaction.
async fn changer_ajustement(db: &mut PgConnection, id: i64, libelle: &str, mode: &str, valeur: i64) -> Result<(), Erreur> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let res = sqlx::query(
        "UPDATE devis SET ajustement_libelle = $1, ajustement_mode = $2, ajustement_valeur = $3, updated_at = now()
         WHERE id = $4 AND statut = 'brouillon'",
    )
    .bind(libelle)
    .bind(mode)
    .bind(valeur)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(Erreur::conflit(MESSAGE_NON_MODIFIABLE));
    }
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    Ok(())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devis/{id}/totaux", get(afficher))
        .route("/devis/{id}/ajustement", post(enregistrer))
        .route("/devis/{id}/ajustement/retirer", post(retirer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formulaire(sens: &str, mode: &str, valeur: &str) -> AjustementForm {
        AjustementForm { libelle: "Geste commercial".into(), sens: sens.into(), mode: mode.into(), valeur: valeur.into() }
    }

    #[test]
    fn remise_et_majoration_valides() {
        let v = valider(&formulaire("remise", "pourcentage", "5"), 100_000).unwrap();
        assert_eq!((v.mode, v.valeur), ("pourcentage", -500));
        let v = valider(&formulaire("majoration", "pourcentage", "2,5"), 100_000).unwrap();
        assert_eq!((v.mode, v.valeur), ("pourcentage", 250));
        let v = valider(&formulaire("remise", "montant", "100,50"), 100_000).unwrap();
        assert_eq!((v.mode, v.valeur), ("montant", -10_050));
        let v = valider(&formulaire("majoration", "montant", "1 000"), 0).unwrap();
        assert_eq!((v.mode, v.valeur), ("montant", 100_000));
    }

    #[test]
    fn ajustements_refuses() {
        let s = 100_000;
        for (sens, mode, valeur) in [
            ("remise", "pourcentage", "100,01"),
            ("remise", "pourcentage", "0"),
            ("remise", "pourcentage", "-5"),
            ("remise", "pourcentage", "abc"),
            ("remise", "pourcentage", ""),
            ("remise", "pourcentage", "5,123"),
            ("majoration", "pourcentage", "1000,01"),
            ("remise", "montant", "1000,01"),
            ("remise", "montant", "0,001"),
            ("autre", "montant", "5"),
            ("remise", "autre", "5"),
        ] {
            assert!(valider(&formulaire(sens, mode, valeur), s).is_err(), "{sens} {mode} {valeur}");
        }
        // Une remise égale au sous-total est permise ; au-delà non.
        assert!(valider(&formulaire("remise", "montant", "1000"), s).is_ok());
        assert!(valider(&formulaire("remise", "pourcentage", "100"), s).is_ok());
        let mut f = formulaire("remise", "montant", "5");
        f.libelle = "x".repeat(101);
        assert!(valider(&f, s).is_err());
    }

    #[test]
    fn le_formulaire_se_relit_depuis_l_ajustement_enregistre() {
        let f = AjustementForm::depuis(Ajustement::Pourcentage(-500), "Fidélité");
        assert_eq!((f.sens.as_str(), f.mode.as_str(), f.valeur.as_str(), f.libelle.as_str()), ("remise", "pourcentage", "5", "Fidélité"));
        let f = AjustementForm::depuis(Ajustement::Montant(1_250), "");
        assert_eq!((f.sens.as_str(), f.mode.as_str(), f.valeur.as_str()), ("majoration", "montant", "12,5"));
        let f = AjustementForm::depuis(Ajustement::Aucun, "");
        assert_eq!((f.sens.as_str(), f.valeur.as_str()), ("remise", ""));
    }
}
