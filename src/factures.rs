//! Factures (étape 7) : liste, fiche, lignes, validation et numérotation.
//!
//! Une facture naît en brouillon, sans numéro : elle se modifie et se supprime librement.
//! « Valider » lui donne le numéro suivant (compteur incrémenté dans la même transaction), sa date d'échéance
//! et sa communication structurée : elle est alors comptabilisée, ne se modifie plus et ne se supprime jamais
//! (la base le refuse aussi). Une erreur se corrige par une note de crédit.
//! Les lignes d'une facture simple et d'une note de crédit se saisissent à la main ; celles d'une facture
//! d'acompte, d'avancement ou finale sont calculées depuis le chantier (voir `factures_chantier`).

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::HeaderMap,
    response::{Html, Redirect},
    routing::{get, post},
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::calcul::{self, Ajustement};
use crate::commun::{Choix, echapper_like, encoder_url, format_euros};
use crate::devis_editeur::{Personne, Referentiel};
use crate::facture_calcul;
use crate::fiches::choix_base;
use crate::{AppState, Erreur};

const PAR_PAGE: i64 = 20;
const LONGUEUR_MAX_REFERENCE: usize = 200;
const LONGUEUR_MAX_TEXTE: usize = 2_000;
const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_UNITE: usize = 20;

/// Valeur en base et libellé.
pub(crate) const GENRES: [(&str, &str); 5] = [
    ("simple", "Facture simple"),
    ("acompte", "Facture d'acompte"),
    ("avancement", "Facture d'avancement"),
    ("finale", "Facture finale"),
    ("note_credit", "Note de crédit"),
];

/// Valeur en base, libellé, couleur du badge.
pub(crate) const STATUTS: [(&str, &str, &str); 3] =
    [("brouillon", "Brouillon", "badge-ghost"), ("a_envoyer", "À envoyer", "badge-warning"), ("envoyee", "Envoyée", "badge-info")];

pub(crate) fn libelle_genre(genre: &str) -> &'static str {
    GENRES.iter().find(|(v, _)| *v == genre).map(|(_, l)| *l).unwrap_or("Facture")
}

pub(crate) fn statut(valeur: &str) -> (&'static str, &'static str) {
    STATUTS.iter().find(|(v, _, _)| *v == valeur).map(|(_, l, c)| (*l, *c)).unwrap_or(("Inconnu", "badge-ghost"))
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// « Facture N° 125 », « Note de crédit N° 6 », « Facture d'acompte (brouillon) ».
pub(crate) fn titre(genre: &str, numero: Option<i64>) -> String {
    match (genre, numero) {
        ("note_credit", Some(n)) => format!("Note de crédit N° {n}"),
        (_, Some(n)) => format!("Facture N° {n}"),
        (g, None) => format!("{} (brouillon)", libelle_genre(g)),
    }
}

/// Le taux de TVA proposé par défaut : le taux normal (21 %) s'il existe, sinon le premier taux actif.
pub(crate) async fn taux_par_defaut<'e>(db: impl sqlx::PgExecutor<'e>) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE actif ORDER BY taux_centiemes = 2100 DESC, taux_centiemes DESC, id LIMIT 1")
        .fetch_optional(db)
        .await
}

// ─── Lignes et totaux ───────────────────────────────────────────────────────

#[derive(FromRow, Clone)]
pub(crate) struct LigneBase {
    pub(crate) id: i64,
    pub(crate) genre: String,
    pub(crate) description: String,
    pub(crate) quantite_e5: Option<i64>,
    pub(crate) unite: String,
    pub(crate) prix_unitaire_e5: Option<i64>,
    pub(crate) taux_tva_id: Option<i64>,
    pub(crate) taux_centiemes: Option<i32>,
    pub(crate) total_cents: i64,
    pub(crate) autoliquidation: bool,
}

pub(crate) async fn charger_lignes<'e>(db: impl sqlx::PgExecutor<'e>, facture_id: i64) -> Result<Vec<LigneBase>, sqlx::Error> {
    sqlx::query_as(
        "SELECT l.id, l.genre, l.description, l.quantite_e5, l.unite, l.prix_unitaire_e5, l.taux_tva_id, l.taux_centiemes,
                l.total_cents, COALESCE(t.autoliquidation, false) AS autoliquidation
         FROM facture_lignes l LEFT JOIN taux_tva t ON t.id = l.taux_tva_id
         WHERE l.facture_id = $1 ORDER BY l.position",
    )
    .bind(facture_id)
    .fetch_all(db)
    .await
}

/// Totaux d'une facture : la TVA se calcule une fois par taux, sur la somme des bases (comme pour le devis).
pub(crate) fn totaux(lignes: &[LigneBase]) -> calcul::Totaux {
    let comptees: Vec<(i64, i32)> =
        lignes.iter().filter(|l| l.genre == "poste").filter_map(|l| Some((l.total_cents, l.taux_centiemes?))).collect();
    calcul::totaux(&comptees, Ajustement::Aucun)
}

/// Recalcule et enregistre les totaux de la facture depuis ses lignes.
pub(crate) async fn recalculer(tx: &mut Transaction<'_, Postgres>, facture_id: i64) -> Result<calcul::Totaux, sqlx::Error> {
    let lignes = charger_lignes(&mut **tx, facture_id).await?;
    let t = totaux(&lignes);
    sqlx::query(
        "UPDATE factures SET montant_ht_cents = $1, montant_tva_cents = $2, montant_ttc_cents = $3, updated_at = now() WHERE id = $4",
    )
    .bind(t.ht_cents)
    .bind(t.ttc_cents - t.ht_cents)
    .bind(t.ttc_cents)
    .bind(facture_id)
    .execute(&mut **tx)
    .await?;
    Ok(t)
}

/// Une ligne prête à être ajoutée à une facture (créée depuis un chantier ou copiée d'une autre facture).
pub(crate) struct NouvelleLigne {
    pub(crate) genre: &'static str,
    pub(crate) description: String,
    pub(crate) quantite_e5: Option<i64>,
    pub(crate) unite: String,
    pub(crate) prix_unitaire_e5: Option<i64>,
    pub(crate) taux_tva_id: Option<i64>,
    pub(crate) total_cents: i64,
}

impl NouvelleLigne {
    pub(crate) fn titre(description: impl Into<String>) -> Self {
        NouvelleLigne {
            genre: "titre",
            description: description.into(),
            quantite_e5: None,
            unite: String::new(),
            prix_unitaire_e5: None,
            taux_tva_id: None,
            total_cents: 0,
        }
    }

    /// Un poste au forfait : 1 × le montant.
    pub(crate) fn forfait(description: impl Into<String>, taux_tva_id: i64, total_cents: i64) -> Self {
        NouvelleLigne {
            genre: "poste",
            description: description.into(),
            quantite_e5: Some(100_000),
            unite: "forfait".to_string(),
            prix_unitaire_e5: Some(total_cents * 1_000),
            taux_tva_id: Some(taux_tva_id),
            total_cents,
        }
    }
}

/// Ajoute des lignes à la fin d'une facture brouillon (le taux est copié depuis la table des taux).
pub(crate) async fn ajouter_lignes(tx: &mut Transaction<'_, Postgres>, facture_id: i64, lignes: &[NouvelleLigne]) -> Result<(), sqlx::Error> {
    let mut position: i32 = sqlx::query_scalar("SELECT COALESCE(max(position), 0) FROM facture_lignes WHERE facture_id = $1")
        .bind(facture_id)
        .fetch_one(&mut **tx)
        .await?;
    for l in lignes {
        position += 1;
        sqlx::query(
            "INSERT INTO facture_lignes (facture_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5,
                                         taux_tva_id, taux_centiemes, total_cents)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, (SELECT taux_centiemes FROM taux_tva WHERE id = $8), $9)",
        )
        .bind(facture_id)
        .bind(position)
        .bind(l.genre)
        .bind(&l.description)
        .bind(l.quantite_e5)
        .bind(&l.unite)
        .bind(l.prix_unitaire_e5)
        .bind(l.taux_tva_id)
        .bind(l.total_cents)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

// ─── Ce que l'écran affiche ─────────────────────────────────────────────────

pub struct LigneVue {
    pub id: i64,
    pub est_poste: bool,
    pub est_titre: bool,
    pub numero: String,
    pub description: String,
    pub quantite: String,
    pub unite: String,
    /// La valeur du champ de saisie (« 19,99 »).
    pub prix_unitaire: String,
    pub prix_affiche: String,
    pub taux: Vec<Choix>,
    pub taux_texte: String,
    pub total: String,
}

pub struct TvaVue {
    pub taux: String,
    pub base: String,
    pub montant: String,
}

pub struct TotauxVue {
    pub ht: String,
    pub tva: Vec<TvaVue>,
    pub ttc: String,
    /// Une ligne au taux d'autoliquidation : la mention légale doit figurer sur la facture.
    pub autoliquidation: bool,
}

pub struct Lignes {
    pub id: i64,
    /// Lignes saisies à la main (facture simple, note de crédit) et facture en brouillon.
    pub modifiables: bool,
    pub lignes: Vec<LigneVue>,
    pub totaux: TotauxVue,
    pub message: String,
}

pub(crate) fn vue_totaux(lignes: &[LigneBase]) -> TotauxVue {
    let t = totaux(lignes);
    TotauxVue {
        ht: format_euros(t.ht_cents),
        tva: t
            .tva
            .iter()
            .map(|v| TvaVue { taux: calcul::taux_en_texte(v.taux_centiemes), base: format_euros(v.base_cents), montant: format_euros(v.tva_cents) })
            .collect(),
        ttc: format_euros(t.ttc_cents),
        autoliquidation: lignes.iter().any(|l| l.genre == "poste" && l.autoliquidation),
    }
}

/// Numérotation comme dans le devis : un titre ouvre un lot (1.0), les lignes suivantes en font partie (1.1…).
pub(crate) fn numeros(lignes: &[LigneBase]) -> Vec<String> {
    let (mut lot, mut rang) = (0, 0);
    lignes
        .iter()
        .map(|l| {
            if l.genre == "titre" {
                lot += 1;
                rang = 0;
                format!("{lot}.0")
            } else {
                rang += 1;
                format!("{lot}.{rang}")
            }
        })
        .collect()
}

fn construire_lignes(id: i64, modifiables: bool, lignes: &[LigneBase], taux: &[(i64, String)], message: String) -> Lignes {
    let numeros = numeros(lignes);
    Lignes {
        id,
        modifiables,
        lignes: lignes
            .iter()
            .zip(numeros)
            .map(|(l, numero)| LigneVue {
                id: l.id,
                est_poste: l.genre == "poste",
                est_titre: l.genre == "titre",
                numero,
                description: l.description.clone(),
                quantite: l.quantite_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
                unite: l.unite.clone(),
                prix_unitaire: l.prix_unitaire_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
                prix_affiche: l.prix_unitaire_e5.map(calcul::prix_pour_affichage).unwrap_or_default(),
                taux: taux.iter().map(|(tid, lib)| Choix { valeur: tid.to_string(), libelle: lib.clone(), choisi: Some(*tid) == l.taux_tva_id }).collect(),
                taux_texte: l.taux_centiemes.map(calcul::taux_en_texte).unwrap_or_default(),
                total: if l.genre == "poste" { format_euros(l.total_cents) } else { String::new() },
            })
            .collect(),
        totaux: vue_totaux(lignes),
        message,
    }
}

// ─── La facture en base ─────────────────────────────────────────────────────

#[derive(FromRow)]
pub(crate) struct Facture {
    pub(crate) id: i64,
    pub(crate) genre: String,
    pub(crate) numero: Option<i64>,
    pub(crate) statut: String,
    pub(crate) date_facture: NaiveDate,
    pub(crate) date_echeance: Option<NaiveDate>,
    pub(crate) reference: String,
    pub(crate) responsable: String,
    pub(crate) client_entreprise_id: Option<i64>,
    pub(crate) client_contact_id: Option<i64>,
    pub(crate) conditions_paiement_id: Option<i64>,
    pub(crate) taux_tva_id: Option<i64>,
    pub(crate) remarque: String,
    pub(crate) chantier_id: Option<i64>,
    pub(crate) avancement_id: Option<i64>,
    pub(crate) facture_origine_id: Option<i64>,
    pub(crate) vcs: Option<String>,
    pub(crate) montant_ttc_cents: i64,
}

impl Facture {
    pub(crate) fn brouillon(&self) -> bool {
        self.statut == "brouillon"
    }

    /// Les lignes d'une facture simple ou d'une note de crédit se saisissent à la main.
    pub(crate) fn lignes_libres(&self) -> bool {
        matches!(self.genre.as_str(), "simple" | "note_credit")
    }
}

pub(crate) async fn charger<'e>(db: impl sqlx::PgExecutor<'e>, id: i64) -> Result<Option<Facture>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, genre::text AS genre, numero, statut::text AS statut, date_facture, date_echeance, reference, responsable,
                client_entreprise_id, client_contact_id, conditions_paiement_id, taux_tva_id, remarque, chantier_id,
                avancement_id, facture_origine_id, vcs, montant_ttc_cents
         FROM factures WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

// ─── Liste ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Recherche {
    q: String,
    genre: String,
    statut: String,
    page: String,
}

#[derive(FromRow)]
struct LigneListeBase {
    id: i64,
    genre: String,
    numero: Option<i64>,
    date_facture: NaiveDate,
    date_echeance: Option<NaiveDate>,
    client: String,
    reference: String,
    montant_ht_cents: i64,
    montant_ttc_cents: i64,
    statut: String,
}

pub struct LigneListe {
    pub id: i64,
    pub numero: String,
    pub date: String,
    pub echeance: String,
    pub genre: &'static str,
    pub client: String,
    pub reference: String,
    pub montant_ht: String,
    pub montant_ttc: String,
    pub statut: &'static str,
    pub classe_statut: &'static str,
}

pub struct Liste {
    pub lignes: Vec<LigneListe>,
    pub q: String,
    pub genres: Vec<Choix>,
    pub statuts: Vec<Choix>,
    pub requete: String,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
    pub total_ht: String,
    pub total_tva: String,
    pub total_ttc: String,
}

#[derive(Template)]
#[template(path = "factures_liste.html")]
struct PageListe {
    liste: Liste,
}

#[derive(Template)]
#[template(path = "_factures_tableau.html")]
struct TableauListe {
    liste: Liste,
}

const DEPUIS: &str = "FROM factures f
    LEFT JOIN entreprises e ON e.id = f.client_entreprise_id
    LEFT JOIN contacts c ON c.id = f.client_contact_id
    WHERE ($1 = ''
           OR f.numero::text = $1
           OR f.reference ILIKE $2
           OR COALESCE(e.nom, concat_ws(' ', c.prenom, c.nom)) ILIKE $2)
      AND ($3 = '' OR f.genre::text = $3)
      AND ($4 = '' OR f.statut::text = $4)";

/// Une note de crédit diminue les totaux : elle compte en négatif.
const SIGNE: &str = "CASE WHEN f.genre = 'note_credit' THEN -1 ELSE 1 END";

async fn liste(State(state): State<AppState>, Query(r): Query<Recherche>, entetes: HeaderMap) -> Result<Html<String>, Erreur> {
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    let genre = GENRES.iter().map(|(v, _)| *v).find(|v| *v == r.genre.trim()).unwrap_or("");
    let statut_choisi = STATUTS.iter().map(|(v, _, _)| *v).find(|v| *v == r.statut.trim()).unwrap_or("");

    let (total, ht, tva, ttc): (i64, i64, i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*), COALESCE(sum({SIGNE} * f.montant_ht_cents), 0)::bigint,
                COALESCE(sum({SIGNE} * f.montant_tva_cents), 0)::bigint, COALESCE(sum({SIGNE} * f.montant_ttc_cents), 0)::bigint
         {DEPUIS}"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(genre)
    .bind(statut_choisi)
    .fetch_one(&state.db)
    .await?;
    let pages = ((total + PAR_PAGE - 1) / PAR_PAGE).max(1);
    let page: i64 = r.page.trim().parse().unwrap_or(1).clamp(1, pages);

    let lignes: Vec<LigneListeBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.id, f.genre::text AS genre, f.numero, f.date_facture, f.date_echeance,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '') AS client,
                f.reference, {SIGNE} * f.montant_ht_cents AS montant_ht_cents, {SIGNE} * f.montant_ttc_cents AS montant_ttc_cents,
                f.statut::text AS statut
         {DEPUIS} ORDER BY f.numero IS NOT NULL, f.date_facture DESC, f.numero DESC NULLS FIRST, f.id DESC LIMIT $5 OFFSET $6"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(genre)
    .bind(statut_choisi)
    .bind(PAR_PAGE)
    .bind((page - 1) * PAR_PAGE)
    .fetch_all(&state.db)
    .await?;

    let liste = Liste {
        lignes: lignes
            .into_iter()
            .map(|l| {
                let (libelle, classe) = statut(&l.statut);
                LigneListe {
                    id: l.id,
                    numero: match (l.genre.as_str(), l.numero) {
                        ("note_credit", Some(n)) => format!("NC {n}"),
                        (_, Some(n)) => n.to_string(),
                        (_, None) => "—".to_string(),
                    },
                    date: date_belge(l.date_facture),
                    echeance: l.date_echeance.map(date_belge).unwrap_or_default(),
                    genre: libelle_genre(&l.genre),
                    client: l.client,
                    reference: l.reference,
                    montant_ht: format_euros(l.montant_ht_cents),
                    montant_ttc: format_euros(l.montant_ttc_cents),
                    statut: libelle,
                    classe_statut: classe,
                }
            })
            .collect(),
        requete: format!("q={}&genre={genre}&statut={statut_choisi}", encoder_url(&q)),
        q,
        genres: GENRES.iter().map(|(v, l)| Choix { valeur: (*v).to_string(), libelle: (*l).to_string(), choisi: *v == genre }).collect(),
        statuts: STATUTS
            .iter()
            .map(|(v, l, _)| Choix { valeur: (*v).to_string(), libelle: (*l).to_string(), choisi: *v == statut_choisi })
            .collect(),
        page,
        pages,
        total,
        total_ht: format_euros(ht),
        total_tva: format_euros(tva),
        total_ttc: format_euros(ttc),
    };
    let partiel = entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request");
    let html = if partiel { TableauListe { liste }.render()? } else { PageListe { liste }.render()? };
    Ok(Html(html))
}

/// « Nouvelle facture » : une facture simple en brouillon, sans numéro.
async fn nouvelle(State(state): State<AppState>) -> Result<Redirect, Erreur> {
    let taux = taux_par_defaut(&state.db).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO factures (genre, date_facture, taux_tva_id) VALUES ('simple', (now() AT TIME ZONE 'Europe/Brussels')::date, $1) RETURNING id",
    )
    .bind(taux)
    .fetch_one(&state.db)
    .await?;
    Ok(Redirect::to(&format!("/factures/{id}?ok=creee")))
}

// ─── Fiche ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct InfosForm {
    date: String,
    reference: String,
    responsable: String,
    client: String,
    conditions_paiement_id: String,
    taux_tva_id: String,
    remarque: String,
}

impl InfosForm {
    fn depuis(f: &Facture) -> Self {
        InfosForm {
            date: f.date_facture.to_string(),
            reference: f.reference.clone(),
            responsable: f.responsable.clone(),
            client: Personne::depuis(f.client_entreprise_id, f.client_contact_id).map(Personne::code).unwrap_or_default(),
            conditions_paiement_id: f.conditions_paiement_id.map(|i| i.to_string()).unwrap_or_default(),
            taux_tva_id: f.taux_tva_id.map(|i| i.to_string()).unwrap_or_default(),
            remarque: f.remarque.clone(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

impl Retour {
    /// Les messages sont fixes : rien de ce qui est dans l'adresse n'est affiché tel quel.
    fn message_ok(&self) -> &'static str {
        match self.ok.as_str() {
            "creee" => "La facture est créée en brouillon : elle n'a pas encore de numéro.",
            "infos" => "Les informations sont enregistrées.",
            "validee" => "La facture est validée : elle a reçu son numéro et ne se modifie plus.",
            "envoyee" => "La facture est marquée comme envoyée.",
            "credit" => "La note de crédit est créée en brouillon : ajuste ses lignes puis valide-la.",
            _ => "",
        }
    }

    fn message_erreur(&self) -> &'static str {
        match self.erreur.as_str() {
            "infos" => "Les informations n'ont pas été enregistrées : vérifie la date, le client, la référence (200 caractères) et la remarque (2 000 caractères).",
            _ => "",
        }
    }
}

/// Ce que l'écran montre du chantier, de l'avancement ou de la facture corrigée.
pub struct Lien {
    pub adresse: String,
    pub texte: String,
}

#[derive(Template)]
#[template(path = "facture.html")]
struct Page {
    id: i64,
    titre: String,
    genre: &'static str,
    statut: &'static str,
    classe_statut: &'static str,
    brouillon: bool,
    peut_marquer_envoyee: bool,
    /// Une facture numérotée (pas une note de crédit) peut être créditée.
    peut_crediter: bool,
    numero: String,
    date: String,
    echeance: String,
    vcs: String,
    liens: Vec<Lien>,
    form: InfosForm,
    clients: Vec<Choix>,
    conditions: Vec<Choix>,
    taux_tva: Vec<Choix>,
    client: String,
    conditions_texte: String,
    /// Ce qui manque avant de pouvoir valider.
    a_completer: Vec<String>,
    message_ok: &'static str,
    message_erreur: &'static str,
    lignes: Lignes,
}

#[derive(Template)]
#[template(path = "_facture_lignes.html")]
struct LignesSeules {
    lignes: Lignes,
}

#[derive(Template)]
#[template(path = "_facture_lignes_maj.html")]
struct LignesMaj {
    lignes: Lignes,
}

async fn liens(db: &PgPool, f: &Facture) -> Result<Vec<Lien>, sqlx::Error> {
    let mut liens = Vec::new();
    if let Some(chantier) = f.chantier_id {
        let numero: i64 = sqlx::query_scalar("SELECT numero FROM chantiers WHERE id = $1").bind(chantier).fetch_one(db).await?;
        liens.push(Lien { adresse: format!("/chantiers/{chantier}/finances"), texte: format!("Chantier n° {numero}") });
    }
    if let (Some(chantier), Some(avancement)) = (f.chantier_id, f.avancement_id) {
        let (numero, reference): (i32, String) =
            sqlx::query_as("SELECT numero, reference FROM avancements WHERE id = $1").bind(avancement).fetch_one(db).await?;
        liens.push(Lien { adresse: format!("/chantiers/{chantier}/avancements/{avancement}"), texte: format!("Avancement n° {numero} ({reference})") });
    }
    if let Some(origine) = f.facture_origine_id {
        let numero: Option<i64> = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(origine).fetch_one(db).await?;
        liens.push(Lien {
            adresse: format!("/factures/{origine}"),
            texte: format!("Note de crédit pour la facture n° {}", numero.map(|n| n.to_string()).unwrap_or_default()),
        });
    }
    let notes: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, numero FROM factures WHERE facture_origine_id = $1 ORDER BY id").bind(f.id).fetch_all(db).await?;
    for (id, numero) in notes {
        liens.push(Lien {
            adresse: format!("/factures/{id}"),
            texte: match numero {
                Some(n) => format!("Créditée par la note de crédit n° {n}"),
                None => "Note de crédit en brouillon".to_string(),
            },
        });
    }
    Ok(liens)
}

/// Le nom du client, tel qu'il est affiché.
pub(crate) async fn nom_client(db: &PgPool, entreprise: Option<i64>, contact: Option<i64>) -> Result<String, sqlx::Error> {
    let nom: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE((SELECT nom FROM entreprises WHERE id = $1), (SELECT trim(concat_ws(' ', prenom, nom)) FROM contacts WHERE id = $2))",
    )
    .bind(entreprise)
    .bind(contact)
    .fetch_one(db)
    .await?;
    Ok(nom.unwrap_or_default())
}

async fn afficher(State(state): State<AppState>, Path(id): Path<i64>, Query(retour): Query<Retour>) -> Result<Html<String>, Erreur> {
    let f = charger(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
    let r = Referentiel::charger(&state.db).await?;
    let lignes = charger_lignes(&state.db, id).await?;
    let a_completer = if f.brouillon() { verifier(&state.db, &f, &lignes).await? } else { Vec::new() };
    let (libelle, classe) = statut(&f.statut);
    let form = InfosForm::depuis(&f);
    let conditions_texte = f
        .conditions_paiement_id
        .and_then(|c| r.options.conditions.iter().find(|(i, _)| *i == c).map(|(_, t)| t.clone()))
        .unwrap_or_default();
    let page = Page {
        id,
        titre: titre(&f.genre, f.numero),
        genre: libelle_genre(&f.genre),
        statut: libelle,
        classe_statut: classe,
        brouillon: f.brouillon(),
        peut_marquer_envoyee: f.statut == "a_envoyer",
        peut_crediter: f.numero.is_some() && f.genre != "note_credit",
        numero: f.numero.map(|n| n.to_string()).unwrap_or_default(),
        date: date_belge(f.date_facture),
        echeance: f.date_echeance.map(date_belge).unwrap_or_default(),
        vcs: f.vcs.clone().unwrap_or_default(),
        liens: liens(&state.db, &f).await?,
        clients: r.choix_personnes(&form.client),
        conditions: choix_base(&r.options.conditions, &form.conditions_paiement_id),
        taux_tva: choix_base(&r.options.taux_tva, &form.taux_tva_id),
        client: nom_client(&state.db, f.client_entreprise_id, f.client_contact_id).await?,
        conditions_texte,
        a_completer,
        message_ok: retour.message_ok(),
        message_erreur: retour.message_erreur(),
        lignes: construire_lignes(id, f.brouillon() && f.lignes_libres(), &lignes, &r.options.taux_tva, String::new()),
        form,
    };
    Ok(Html(page.render()?))
}

/// Enregistre l'en-tête d'un brouillon (date, référence, client, conditions…).
async fn enregistrer(State(state): State<AppState>, Path(id): Path<i64>, Form(form): Form<InfosForm>) -> Result<Redirect, Erreur> {
    let f = charger(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Cette facture est validée : elle ne se modifie plus. Corrige-la par une note de crédit."));
    }
    let r = Referentiel::charger(&state.db).await?;
    let erreur = Ok(Redirect::to(&format!("/factures/{id}?erreur=infos")));
    let Ok(date) = NaiveDate::parse_from_str(form.date.trim(), "%Y-%m-%d") else { return erreur };
    let client = match form.client.trim() {
        "" => None,
        texte => match Personne::lire(texte).filter(|p| r.existe(*p)) {
            Some(p) => Some(p),
            None => return erreur,
        },
    };
    let identifiant = |texte: &str, liste: &[(i64, String)]| -> Result<Option<i64>, ()> {
        match texte.trim() {
            "" => Ok(None),
            t => t.parse::<i64>().ok().filter(|i| liste.iter().any(|(x, _)| x == i)).map(Some).ok_or(()),
        }
    };
    let (Ok(conditions), Ok(taux)) =
        (identifiant(&form.conditions_paiement_id, &r.options.conditions), identifiant(&form.taux_tva_id, &r.options.taux_tva))
    else {
        return erreur;
    };
    let reference = form.reference.trim();
    let responsable = form.responsable.trim();
    let remarque = form.remarque.trim();
    if reference.chars().count() > LONGUEUR_MAX_REFERENCE
        || responsable.chars().count() > LONGUEUR_MAX_REFERENCE
        || remarque.chars().count() > LONGUEUR_MAX_TEXTE
    {
        return erreur;
    }
    let (entreprise, contact) = Personne::colonnes(client);
    sqlx::query(
        "UPDATE factures SET date_facture = $1, reference = $2, responsable = $3, client_entreprise_id = $4, client_contact_id = $5,
                conditions_paiement_id = $6, taux_tva_id = $7, remarque = $8, updated_at = now()
         WHERE id = $9 AND statut = 'brouillon'",
    )
    .bind(date)
    .bind(reference)
    .bind(responsable)
    .bind(entreprise)
    .bind(contact)
    .bind(conditions)
    .bind(taux)
    .bind(remarque)
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(Redirect::to(&format!("/factures/{id}?ok=infos")))
}

// ─── Lignes saisies à la main (facture simple, note de crédit) ──────────────

/// La facture, si ses lignes peuvent encore être modifiées à la main.
async fn facture_modifiable(db: &PgPool, id: i64) -> Result<Facture, Erreur> {
    let f = charger(db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Cette facture est validée : elle ne se modifie plus. Corrige-la par une note de crédit."));
    }
    if !f.lignes_libres() {
        return Err(Erreur::conflit("Les lignes de cette facture sont calculées depuis le chantier."));
    }
    Ok(f)
}

async fn rendre_lignes(db: &PgPool, id: i64, message: String, seulement_maj: bool) -> Result<Html<String>, Erreur> {
    let lignes = charger_lignes(db, id).await?;
    let options = crate::fiches::Options::charger(db, crate::fiches::Genre::Entreprise).await?;
    let vue = construire_lignes(id, true, &lignes, &options.taux_tva, message);
    let html = if seulement_maj { LignesMaj { lignes: vue }.render()? } else { LignesSeules { lignes: vue }.render()? };
    Ok(Html(html))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Ajout {
    genre: String,
}

async fn ajouter_ligne(State(state): State<AppState>, Path(id): Path<i64>, Form(a): Form<Ajout>) -> Result<Html<String>, Erreur> {
    let f = facture_modifiable(&state.db, id).await?;
    let genre = match a.genre.as_str() {
        "titre" => "titre",
        "texte" => "texte",
        _ => "poste",
    };
    let mut tx = state.db.begin().await?;
    let taux = match (genre, f.taux_tva_id) {
        ("poste", Some(t)) => Some(t),
        ("poste", None) => taux_par_defaut(&mut *tx).await?,
        _ => None,
    };
    ajouter_lignes(
        &mut tx,
        id,
        &[NouvelleLigne {
            genre,
            description: String::new(),
            quantite_e5: None,
            unite: String::new(),
            prix_unitaire_e5: None,
            taux_tva_id: taux,
            total_cents: 0,
        }],
    )
    .await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    rendre_lignes(&state.db, id, String::new(), false).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaisieLigne {
    description: String,
    quantite: String,
    unite: String,
    prix_unitaire: String,
    taux_tva_id: String,
}

/// Lit une ligne saisie. Erreur : un message simple pour la personne.
fn lire_ligne(s: &SaisieLigne, est_poste: bool, taux_connus: &[(i64, String)]) -> Result<(String, Option<i64>, String, Option<i64>, Option<i64>), String> {
    let description = s.description.trim().to_string();
    if description.chars().count() > LONGUEUR_MAX_DESCRIPTION {
        return Err("La description est trop longue (5 000 caractères au maximum).".to_string());
    }
    if !est_poste {
        return Ok((description, None, String::new(), None, None));
    }
    let nombre = |texte: &str, quoi: &str| -> Result<Option<i64>, String> {
        match texte.trim() {
            "" => Ok(None),
            t => calcul::lire_decimal_e5(t).map(Some).ok_or_else(|| format!("{quoi} : « {t} » n'est pas un nombre.")),
        }
    };
    let quantite = nombre(&s.quantite, "Quantité")?;
    let prix = nombre(&s.prix_unitaire, "Prix unitaire")?;
    let unite = s.unite.trim().to_string();
    if unite.chars().count() > LONGUEUR_MAX_UNITE {
        return Err("L'unité est trop longue (20 caractères au maximum).".to_string());
    }
    let taux = s.taux_tva_id.trim().parse::<i64>().ok().filter(|t| taux_connus.iter().any(|(i, _)| i == t));
    if taux.is_none() {
        return Err("Choisis un taux de TVA.".to_string());
    }
    Ok((description, quantite, unite, prix, taux))
}

async fn modifier_ligne(
    State(state): State<AppState>,
    Path((id, ligne_id)): Path<(i64, i64)>,
    Form(s): Form<SaisieLigne>,
) -> Result<Html<String>, Erreur> {
    facture_modifiable(&state.db, id).await?;
    let genre: Option<String> = sqlx::query_scalar("SELECT genre FROM facture_lignes WHERE id = $1 AND facture_id = $2")
        .bind(ligne_id)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let genre = genre.ok_or_else(Erreur::introuvable)?;
    let options = crate::fiches::Options::charger(&state.db, crate::fiches::Genre::Entreprise).await?;
    let message = match lire_ligne(&s, genre == "poste", &options.taux_tva) {
        Err(m) => m,
        Ok((description, quantite, unite, prix, taux)) => {
            let total = match (quantite, prix) {
                (Some(q), Some(p)) => calcul::total_ligne_cents(q, p),
                _ => 0,
            };
            let mut tx = state.db.begin().await?;
            sqlx::query(
                "UPDATE facture_lignes SET description = $1, quantite_e5 = $2, unite = $3, prix_unitaire_e5 = $4, taux_tva_id = $5,
                        taux_centiemes = (SELECT taux_centiemes FROM taux_tva WHERE id = $5), total_cents = $6
                 WHERE id = $7 AND facture_id = $8",
            )
            .bind(&description)
            .bind(quantite)
            .bind(&unite)
            .bind(prix)
            .bind(taux)
            .bind(total)
            .bind(ligne_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            recalculer(&mut tx, id).await?;
            tx.commit().await?;
            String::new()
        }
    };
    rendre_lignes(&state.db, id, message, true).await
}

async fn supprimer_ligne(State(state): State<AppState>, Path((id, ligne_id)): Path<(i64, i64)>) -> Result<Html<String>, Erreur> {
    facture_modifiable(&state.db, id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM facture_lignes WHERE id = $1 AND facture_id = $2").bind(ligne_id).bind(id).execute(&mut *tx).await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    rendre_lignes(&state.db, id, String::new(), false).await
}

// ─── Valider, envoyer, supprimer ────────────────────────────────────────────

/// Ce qui manque pour valider le brouillon (liste vide : la facture peut être validée).
pub(crate) async fn verifier(db: &PgPool, f: &Facture, lignes: &[LigneBase]) -> Result<Vec<String>, sqlx::Error> {
    let mut manques = Vec::new();
    if f.client_entreprise_id.is_none() && f.client_contact_id.is_none() {
        manques.push("Le client".to_string());
    }
    if f.conditions_paiement_id.is_none() {
        manques.push("Les conditions de paiement".to_string());
    }
    let postes: Vec<&LigneBase> = lignes.iter().filter(|l| l.genre == "poste").collect();
    if postes.is_empty() {
        manques.push("Au moins une ligne chiffrée".to_string());
    }
    if postes.iter().any(|l| l.description.trim().is_empty()) {
        manques.push("La description de chaque ligne chiffrée".to_string());
    }
    let t = totaux(lignes);
    if !postes.is_empty() && t.ttc_cents <= 0 {
        manques.push("Un total TTC positif".to_string());
    }
    if let Some(origine) = f.facture_origine_id {
        // Une note de crédit ne rend jamais plus que ce que la facture n'a pas encore crédité.
        let reste = reste_a_crediter(db, origine, Some(f.id)).await?;
        if t.ttc_cents > reste {
            manques.push(format!("Un total TTC d'au plus {} (ce qui reste à créditer sur la facture)", format_euros(reste)));
        }
    }
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(db).await?;
    if f.date_facture > aujourdhui {
        manques.push("Une date qui n'est pas dans le futur".to_string());
    }
    if let Some(derniere) = derniere_date(db, f.genre == "note_credit").await?
        && f.date_facture < derniere
    {
        manques.push(format!("Une date au plus tôt le {} (date de la dernière pièce numérotée)", date_belge(derniere)));
    }
    Ok(manques)
}

/// La date de la dernière facture (ou note de crédit) numérotée : les numéros suivent l'ordre des dates.
async fn derniere_date<'e>(db: impl sqlx::PgExecutor<'e>, note_credit: bool) -> Result<Option<NaiveDate>, sqlx::Error> {
    sqlx::query_scalar("SELECT max(date_facture) FROM factures WHERE numero IS NOT NULL AND (genre = 'note_credit') = $1")
        .bind(note_credit)
        .fetch_one(db)
        .await
}

/// Le TTC de la facture moins ses notes de crédit validées (sans compter la note `sauf`).
pub(crate) async fn reste_a_crediter(db: &PgPool, facture_id: i64, sauf: Option<i64>) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT f.montant_ttc_cents - COALESCE((SELECT sum(n.montant_ttc_cents) FROM factures n
                                                WHERE n.facture_origine_id = f.id AND n.numero IS NOT NULL
                                                  AND n.id IS DISTINCT FROM $2), 0)::bigint
         FROM factures f WHERE f.id = $1",
    )
    .bind(facture_id)
    .bind(sauf)
    .fetch_one(db)
    .await
}

async fn valider(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let note_credit = {
        let f = charger(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
        if !f.brouillon() {
            return Err(Erreur::conflit("Cette facture est déjà validée."));
        }
        f.genre == "note_credit"
    };
    let compteur = if note_credit { "note_credit" } else { "facture" };
    let mut tx = state.db.begin().await?;
    // Verrouille le compteur d'abord : deux validations en même temps passent l'une après l'autre.
    sqlx::query("SELECT valeur FROM compteurs WHERE nom = $1 FOR UPDATE").bind(compteur).execute(&mut *tx).await?;
    sqlx::query("SELECT id FROM factures WHERE id = $1 FOR UPDATE").bind(id).execute(&mut *tx).await?;
    let f = charger(&mut *tx, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Cette facture est déjà validée."));
    }
    let lignes = charger_lignes(&mut *tx, id).await?;
    let manques = verifier(&state.db, &f, &lignes).await?;
    // La date de la dernière pièce, relue sous le verrou du compteur.
    let derniere = derniere_date(&mut *tx, note_credit).await?;
    if !manques.is_empty() || derniere.is_some_and(|d| f.date_facture < d) {
        return Err(Erreur::conflit("Cette facture ne peut pas encore être validée : complète ce qui est indiqué sur la fiche."));
    }
    let numero: i64 = sqlx::query_scalar("UPDATE compteurs SET valeur = valeur + 1 WHERE nom = $1 RETURNING valeur")
        .bind(compteur)
        .fetch_one(&mut *tx)
        .await?;
    let (genre_condition, jours): (String, i32) = sqlx::query_as("SELECT \"type\"::text, jours FROM conditions_paiement WHERE id = $1")
        .bind(f.conditions_paiement_id)
        .fetch_one(&mut *tx)
        .await?;
    // Une note de crédit se rembourse : pas d'échéance ni de communication structurée à payer.
    let (echeance, vcs) = if note_credit {
        (None, None)
    } else {
        (Some(facture_calcul::echeance(f.date_facture, &genre_condition, jours)), Some(facture_calcul::vcs(numero)))
    };
    recalculer(&mut tx, id).await?;
    sqlx::query(
        "UPDATE factures SET numero = $1, statut = 'a_envoyer', date_echeance = $2, vcs = $3, validee_le = now(), updated_at = now()
         WHERE id = $4 AND statut = 'brouillon'",
    )
    .bind(numero)
    .bind(echeance)
    .bind(vcs)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/factures/{id}?ok=validee")))
}

/// En attendant l'envoi par e-mail (étape 8), on marque la facture comme envoyée à la main.
async fn marquer_envoyee(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let fait = sqlx::query("UPDATE factures SET statut = 'envoyee', envoyee_le = now(), updated_at = now() WHERE id = $1 AND statut = 'a_envoyer'")
        .bind(id)
        .execute(&state.db)
        .await?;
    if fait.rows_affected() == 0 {
        charger(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
        return Err(Erreur::conflit("Seule une facture validée « À envoyer » peut être marquée comme envoyée."));
    }
    Ok(Redirect::to(&format!("/factures/{id}?ok=envoyee")))
}

/// Seul un brouillon (sans numéro) se supprime. La facture finale emporte l'avancement de solde créé pour elle.
async fn supprimer(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let f = charger(&state.db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Une facture validée ne se supprime jamais : corrige-la par une note de crédit."));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM factures WHERE id = $1 AND statut = 'brouillon'").bind(id).execute(&mut *tx).await?;
    if let (Some(avancement), "finale") = (f.avancement_id, f.genre.as_str()) {
        sqlx::query("DELETE FROM avancements WHERE id = $1").bind(avancement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let retour = match f.chantier_id {
        Some(chantier) => format!("/chantiers/{chantier}/finances"),
        None => "/factures".to_string(),
    };
    Ok(Redirect::to(&retour))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/factures", get(liste).post(nouvelle))
        .route("/factures/{id}", get(afficher).post(enregistrer))
        .route("/factures/{id}/lignes", post(ajouter_ligne))
        .route("/factures/{id}/lignes/{ligne}", post(modifier_ligne))
        .route("/factures/{id}/lignes/{ligne}/supprimer", post(supprimer_ligne))
        .route("/factures/{id}/valider", post(valider))
        .route("/factures/{id}/envoyee", post(marquer_envoyee))
        .route("/factures/{id}/supprimer", post(supprimer))
}


#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(genre: &str, total: i64, taux: Option<i32>) -> LigneBase {
        LigneBase {
            id: 0,
            genre: genre.to_string(),
            description: "x".to_string(),
            quantite_e5: None,
            unite: String::new(),
            prix_unitaire_e5: None,
            taux_tva_id: taux.map(|_| 1),
            taux_centiemes: taux,
            total_cents: total,
            autoliquidation: false,
        }
    }

    #[test]
    fn les_titres() {
        assert_eq!(titre("simple", Some(125)), "Facture N° 125");
        assert_eq!(titre("acompte", Some(126)), "Facture N° 126");
        assert_eq!(titre("note_credit", Some(6)), "Note de crédit N° 6");
        assert_eq!(titre("finale", None), "Facture finale (brouillon)");
    }

    #[test]
    fn totaux_par_taux_et_titres_ignores() {
        // l'exemple réel de la facture finale : 2 969,30 € à 6 % → TVA 178,16 €, TTC 3 147,46 €
        let lignes = [ligne("titre", 0, None), ligne("poste", 296_930, Some(600))];
        let t = totaux(&lignes);
        assert_eq!((t.ht_cents, t.ttc_cents - t.ht_cents, t.ttc_cents), (296_930, 17_816, 314_746));
    }

    #[test]
    fn numerotation_des_lignes() {
        let lignes = [ligne("titre", 0, None), ligne("poste", 1, Some(600)), ligne("texte", 0, None), ligne("titre", 0, None), ligne("poste", 1, Some(600))];
        assert_eq!(numeros(&lignes), vec!["1.0", "1.1", "1.2", "2.0", "2.1"]);
    }

    #[test]
    fn chaque_genre_et_statut_a_un_libelle() {
        for (g, _) in GENRES {
            assert_ne!(libelle_genre(g), "Facture", "{g}");
        }
        assert_eq!(statut("a_envoyer").0, "À envoyer");
        assert_eq!(statut("x").0, "Inconnu");
    }
}
