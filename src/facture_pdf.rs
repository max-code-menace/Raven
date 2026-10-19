//! Une facture ou une note de crédit en PDF (étape 7-3) : mise en page Typst `assets/facture.typ`,
//! conditions générales ajoutées à la suite (comme pour le devis). Un brouillon porte la mention « BROUILLON ».

use std::collections::HashMap;

use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use sqlx::PgPool;

use crate::calcul;
use crate::commun::format_euros;
use crate::devis_pdf::{Bloc, Societe, bloc_typst, charger_bloc, charger_societe, dictionnaire, lignes_adresse, litteral, option_litteral, tableau};
use crate::factures::{self, Facture, LigneBase};
use crate::fiches::{Genre, Options};
use crate::{AppState, Erreur, pdf};

const MODELE: &str = include_str!("../assets/facture.typ");

pub(crate) struct Donnees {
    /// Le type, comme titre (« Facture finale », « Facture », « Note de crédit »), comme dans Vertuoza.
    pub titre: String,
    /// « 2026-114 » (vide pour un brouillon).
    pub numero: String,
    /// Le devis de base du chantier (« 2026-13 ») et l'adresse du chantier.
    pub devis: String,
    pub adresse_chantier: String,
    pub brouillon: bool,
    pub note_credit: bool,
    pub date: String,
    pub echeance: String,
    pub reference: String,
    pub responsable: String,
    pub chantier: String,
    pub corrige: String,
    pub conditions: String,
    pub remarque: String,
    pub vcs: String,
    pub societe: Option<Societe>,
    pub client: Option<Bloc>,
    pub lignes: Vec<LigneBase>,
}

pub(crate) async fn charger_donnees(db: &PgPool, f: &Facture) -> Result<Donnees, Erreur> {
    let lignes = factures::charger_lignes(db, f.id).await?;
    let options = Options::charger(db, Genre::Entreprise).await?;
    let conditions = f
        .conditions_paiement_id
        .and_then(|c| options.conditions.iter().find(|(i, _)| *i == c).map(|(_, t)| t.clone()))
        .unwrap_or_default();
    let chantier: Option<String> = sqlx::query_scalar(
        "SELECT 'n° ' || ch.numero || CASE WHEN d.reference <> '' THEN ' · ' || d.reference ELSE '' END
         FROM chantiers ch JOIN devis d ON d.id = ch.devis_id WHERE ch.id = $1",
    )
    .bind(f.chantier_id)
    .fetch_optional(db)
    .await?;
    let corrige: Option<String> = sqlx::query_scalar(
        "SELECT 'Note de crédit pour la facture n° ' || numero || ' du ' || to_char(date_facture, 'DD/MM/YYYY') FROM factures WHERE id = $1",
    )
    .bind(f.facture_origine_id)
    .fetch_optional(db)
    .await?;
    // Le devis de base du chantier et l'adresse du chantier (sous la société, comme dans Vertuoza).
    let base: Option<(i32, i64, bool, String, String, String, String, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT extract(year FROM d.date_devis)::int, d.numero, d.chantier_identique, d.chantier_rue, d.chantier_numero,
                d.chantier_code_postal, d.chantier_ville, d.chantier_pays, d.client_entreprise_id, d.client_contact_id
         FROM chantiers ch JOIN devis d ON d.id = ch.devis_id WHERE ch.id = $1",
    )
    .bind(f.chantier_id)
    .fetch_optional(db)
    .await?;
    let (devis, adresse_chantier) = match base {
        None => (String::new(), String::new()),
        Some((annee, numero, identique, rue, num, cp, ville, pays, entreprise, contact)) => {
            let lignes = if identique {
                charger_bloc(db, entreprise, contact, false).await?.map(|b| b.adresse).unwrap_or_default()
            } else {
                lignes_adresse(&rue, &num, &cp, &ville, &pays)
            };
            (format!("{annee}-{numero}"), lignes.join(", "))
        }
    };
    let note_credit = f.genre == "note_credit";
    Ok(Donnees {
        titre: match f.genre.as_str() {
            "simple" | "recurrente" => "Facture".to_string(),
            g => factures::libelle_genre(g).to_string(),
        },
        numero: f.numero.map(|n| factures::numero_affiche(f.date_facture, n)).unwrap_or_default(),
        devis,
        adresse_chantier,
        brouillon: f.brouillon(),
        note_credit,
        date: f.date_facture.format("%d/%m/%Y").to_string(),
        echeance: f.date_echeance.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(),
        reference: f.reference.clone(),
        responsable: f.responsable.clone(),
        chantier: chantier.unwrap_or_default(),
        corrige: corrige.unwrap_or_default(),
        conditions: if note_credit { String::new() } else { conditions },
        remarque: f.remarque.clone(),
        vcs: f.vcs.clone().unwrap_or_default(),
        societe: charger_societe(db).await?,
        client: charger_bloc(db, f.client_entreprise_id, f.client_contact_id, true).await?,
        lignes,
    })
}

/// Le document complet : le modèle, puis l'appel qui lui donne les données. Renvoie aussi le logo.
pub(crate) fn source_typst(d: &Donnees) -> (String, HashMap<String, Vec<u8>>) {
    let mut fichiers = HashMap::new();
    let societe = match &d.societe {
        None => "none".to_string(),
        Some(s) => {
            let adresse: Vec<String> = s.bloc.adresse.iter().map(|l| litteral(l)).collect();
            let logo = s.logo.as_ref().map(|(nom, octets)| {
                fichiers.insert(nom.clone(), octets.clone());
                nom.as_str()
            });
            dictionnaire(&[
                ("nom", litteral(&s.bloc.nom)),
                ("adresse", tableau(&adresse)),
                ("telephone", litteral(&s.telephone)),
                ("email", litteral(&s.email)),
                ("tva", litteral(&s.bloc.tva)),
                ("iban", litteral(&s.iban)),
                ("logo", option_litteral(logo)),
            ])
        }
    };

    let numeros = factures::numeros(&d.lignes);
    let lignes: Vec<String> = d
        .lignes
        .iter()
        .zip(numeros)
        .map(|(l, numero)| {
            if l.genre == "poste" {
                dictionnaire(&[
                    ("type", litteral("poste")),
                    ("numero", litteral(&numero)),
                    ("description", litteral(&l.description)),
                    ("quantite", litteral(&l.quantite_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default())),
                    ("unite", litteral(&l.unite)),
                    ("prix_unitaire", litteral(&l.prix_unitaire_e5.map(calcul::prix_pour_affichage).unwrap_or_default())),
                    ("tva", litteral(&l.taux_centiemes.map(calcul::taux_en_texte).unwrap_or_default())),
                    ("total", litteral(&format_euros(l.total_cents))),
                ])
            } else {
                dictionnaire(&[("type", litteral(&l.genre)), ("numero", litteral(&numero)), ("description", litteral(&l.description))])
            }
        })
        .collect();

    let totaux = factures::vue_totaux(&d.lignes);
    let tva: Vec<String> = totaux
        .tva
        .iter()
        .map(|v| dictionnaire(&[("taux", litteral(&v.taux)), ("base", litteral(&v.base)), ("montant", litteral(&v.montant))]))
        .collect();
    let totaux_typst = dictionnaire(&[("ht", litteral(&totaux.ht)), ("tva", tableau(&tva)), ("ttc", litteral(&totaux.ttc))]);

    // Le bloc de paiement : jamais sur une note de crédit (elle se rembourse).
    let paiement = if d.note_credit {
        "none".to_string()
    } else {
        let iban = d.societe.as_ref().map(|s| s.iban.clone()).unwrap_or_default();
        dictionnaire(&[("echeance", litteral(&d.echeance)), ("iban", litteral(&iban)), ("vcs", litteral(&d.vcs))])
    };

    let donnees = dictionnaire(&[
        ("titre", litteral(&d.titre)),
        ("numero", litteral(&d.numero)),
        ("devis", litteral(&d.devis)),
        ("adresse_chantier", litteral(&d.adresse_chantier)),
        ("brouillon", d.brouillon.to_string()),
        ("date", litteral(&d.date)),
        ("echeance", litteral(&d.echeance)),
        ("reference", litteral(&d.reference)),
        ("responsable", litteral(&d.responsable)),
        ("chantier", litteral(&d.chantier)),
        ("corrige", litteral(&d.corrige)),
        ("conditions", litteral(&d.conditions)),
        ("remarque", litteral(&d.remarque)),
        ("societe", societe),
        ("client", d.client.as_ref().map(bloc_typst).unwrap_or_else(|| "none".to_string())),
        ("lignes", tableau(&lignes)),
        ("totaux", totaux_typst),
        ("libelle_total", litteral(if d.note_credit { "Total TTC crédité" } else { "Total TTC à payer" })),
        ("mentions", tableau(&crate::mentions_tva::a_imprimer(&d.lignes).iter().map(|m| litteral(m)).collect::<Vec<_>>())),
        ("paiement", paiement),
    ]);
    (format!("{MODELE}\n#facture({donnees})\n"), fichiers)
}

/// Le PDF de la pièce (nom de fichier, octets), conditions générales comprises.
pub(crate) async fn pdf_de_la_facture(db: &PgPool, f: &Facture) -> Result<(String, Vec<u8>), Erreur> {
    let donnees = charger_donnees(db, f).await?;
    let nom = match (f.genre.as_str(), f.numero) {
        ("note_credit", Some(n)) => format!("Note-de-credit-{n}.pdf"),
        (_, Some(n)) => format!("Facture-{n}.pdf"),
        (_, None) => format!("Facture-brouillon-{}.pdf", f.id),
    };
    let (texte, fichiers) = source_typst(&donnees);
    let avait_un_logo = !fichiers.is_empty();
    let id = f.id;
    let essai = {
        let texte = texte.clone();
        tokio::task::spawn_blocking(move || pdf::generer(texte, fichiers)).await
    };
    let mut octets = match essai {
        Ok(Ok(octets)) => octets,
        // Un logo abîmé ne doit pas empêcher de facturer : on réessaie sans lui.
        Ok(Err(erreur)) if avait_un_logo => {
            eprintln!("PDF de la facture {id} : {erreur} ; nouvel essai sans le logo");
            let (texte, _) = source_typst(&Donnees { societe: donnees.societe.map(|s| Societe { logo: None, ..s }), ..donnees });
            tokio::task::spawn_blocking(move || pdf::generer(texte, HashMap::new()))
                .await
                .map_err(|e| Erreur::pdf(&e.to_string()))?
                .map_err(|e| Erreur::pdf(&e))?
        }
        Ok(Err(erreur)) => return Err(Erreur::pdf(&erreur)),
        Err(e) => return Err(Erreur::pdf(&e.to_string())),
    };
    let conditions: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT contenu FROM societe_fichiers WHERE usage = 'conditions_generales'").fetch_optional(db).await?;
    if let Some(conditions) = conditions {
        let base = octets.clone();
        match tokio::task::spawn_blocking(move || pdf::fusionner(&base, &conditions)).await {
            Ok(Ok(fusion)) => octets = fusion,
            Ok(Err(erreur)) => eprintln!("PDF de la facture {id} : conditions générales ignorées ({erreur})"),
            Err(e) => eprintln!("PDF de la facture {id} : conditions générales ignorées ({e})"),
        }
    }
    Ok((nom, octets))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParametresPdf {
    telecharger: String,
}

async fn servir(State(state): State<AppState>, Path(id): Path<i64>, Query(o): Query<ParametresPdf>) -> Result<Response, Erreur> {
    let f = factures::charger(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
    let (nom, octets) = pdf_de_la_facture(&state.db, &f).await?;
    let disposition = if o.telecharger.is_empty() { "inline" } else { "attachment" };
    let mut reponse = octets.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    // Le nom ne contient que des lettres sans accent, des chiffres et des tirets : il est sûr dans un en-tête.
    entetes.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("{disposition}; filename=\"{nom}\"")).map_err(|_| Erreur::pdf("nom de fichier"))?,
    );
    entetes.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(reponse)
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/factures/{id}/pdf", get(servir))
}

/// Les pages de la pièce en images PNG, pour vérifier la mise en page à l'œil (voir `tests/notes_credit.rs`).
pub async fn pages_en_png(db: &PgPool, id: i64) -> Result<Vec<Vec<u8>>, String> {
    let f = factures::charger(db, id).await.map_err(|e| e.to_string())?.ok_or("facture introuvable")?;
    let donnees = charger_donnees(db, &f).await.map_err(|_| "données illisibles".to_string())?;
    let (texte, fichiers) = source_typst(&donnees);
    pdf::pages_en_png(texte, fichiers, 1.6)
}
