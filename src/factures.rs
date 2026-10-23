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
    extract::{Form, Path, Query},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};

use crate::base::Base;
use crate::calcul::{self, Ajustement};
use crate::commun::{Choix, echapper_like, encoder_url, format_euros};
use crate::composants::Enregistrement;
use crate::devis_editeur::{Personne, Referentiel};
use crate::facture_calcul;
use crate::fiches::choix_base;
use crate::paiements::{self, Solde};
use crate::{AppState, Erreur};

const PAR_PAGE: i64 = 20;
const LONGUEUR_MAX_REFERENCE: usize = 200;
const LONGUEUR_MAX_TEXTE: usize = 2_000;
const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_UNITE: usize = 20;

/// Valeur en base et libellé.
pub(crate) const GENRES: [(&str, &str); 6] = [
    ("simple", "Facture simple"),
    ("acompte", "Facture d'acompte"),
    ("avancement", "Facture d'avancement"),
    ("finale", "Facture finale"),
    ("recurrente", "Facture récurrente"),
    ("note_credit", "Note de crédit"),
];

/// Valeur, libellé, couleur du badge : les statuts d'envoi (en base) et ceux du paiement (calculés, étape 8-1).
pub(crate) const STATUTS: [(&str, &str, &str); 7] = paiements::ETATS;

pub(crate) fn libelle_genre(genre: &str) -> &'static str {
    GENRES.iter().find(|(v, _)| *v == genre).map(|(_, l)| *l).unwrap_or("Facture")
}

/// Le libellé et la couleur d'un état (« envoyee », « payee », « en_retard »…).
pub(crate) fn statut(valeur: &str) -> (&'static str, &'static str) {
    paiements::etat(valeur)
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// Le numéro tel qu'il est imprimé, comme dans Vertuoza : l'année puis le numéro (« 2026-125 ») ; la numérotation
/// repart à 1 chaque année. Les listes montrent le numéro seul.
pub(crate) fn numero_affiche(date: NaiveDate, numero: i64) -> String {
    format!("{}-{numero}", date.year())
}

/// « Facture n° 2026-125 », « Note de crédit n° 2026-6 », « Facture d'acompte (brouillon) ».
pub(crate) fn titre(genre: &str, numero: Option<i64>, date: NaiveDate) -> String {
    match (genre, numero) {
        ("note_credit", Some(n)) => format!("Note de crédit n° {}", numero_affiche(date, n)),
        (_, Some(n)) => format!("Facture n° {}", numero_affiche(date, n)),
        (g, None) => format!("{} (brouillon)", libelle_genre(g)),
    }
}

/// Le taux de TVA proposé par défaut : le taux normal (21 %) s'il existe, sinon le premier taux actif ; pour une
/// société en franchise, le taux « Franchise de TVA ».
pub(crate) async fn taux_par_defaut<'e>(db: impl sqlx::PgExecutor<'e>) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM taux_tva
         WHERE actif AND franchise = coalesce((SELECT regime_tva = 'franchise' FROM societe WHERE organisation_id = app_organisation()), false)
         ORDER BY taux_centiemes = 2100 DESC, taux_centiemes DESC, id LIMIT 1",
    )
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
    /// Au taux « Franchise de TVA ».
    pub(crate) franchise: bool,
}

pub(crate) async fn charger_lignes<'e>(db: impl sqlx::PgExecutor<'e>, facture_id: i64) -> Result<Vec<LigneBase>, sqlx::Error> {
    sqlx::query_as(
        "SELECT l.id, l.genre, l.description, l.quantite_e5, l.unite, l.prix_unitaire_e5, l.taux_tva_id, l.taux_centiemes,
                l.total_cents, COALESCE(t.autoliquidation, false) AS autoliquidation, COALESCE(t.franchise, false) AS franchise
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
    /// Une ligne en franchise de TVA : la mention légale doit figurer sur la facture.
    pub franchise: bool,
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
        franchise: lignes.iter().any(|l| l.genre == "poste" && l.franchise),
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
                // Sans titre avant elle, une ligne est simplement numérotée 1, 2, 3…
                if lot == 0 { rang.to_string() } else { format!("{lot}.{rang}") }
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
    /// Facture reprise de Vertuoza : son PDF d'origine fait foi.
    pub(crate) reprise: bool,
}

impl Facture {
    pub(crate) fn brouillon(&self) -> bool {
        self.statut == "brouillon"
    }

    /// Les lignes d'une facture simple, récurrente ou d'une note de crédit se saisissent à la main.
    pub(crate) fn lignes_libres(&self) -> bool {
        matches!(self.genre.as_str(), "simple" | "recurrente" | "note_credit")
    }

    /// Une facture saisie à la main (pas calculée depuis un chantier) peut servir de modèle à une récurrence.
    pub(crate) fn peut_recurrer(&self) -> bool {
        matches!(self.genre.as_str(), "simple" | "recurrente")
    }
}

pub(crate) async fn charger<'e>(db: impl sqlx::PgExecutor<'e>, id: i64) -> Result<Option<Facture>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, genre::text AS genre, numero, statut::text AS statut, date_facture, date_echeance, reference, responsable,
                client_entreprise_id, client_contact_id, conditions_paiement_id, taux_tva_id, remarque, chantier_id,
                avancement_id, facture_origine_id, vcs, reprise
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
    chantier: String,
    montant_ht_cents: i64,
    montant_ttc_cents: i64,
    #[sqlx(flatten)]
    solde: Solde,
}

pub struct LigneListe {
    pub id: i64,
    /// Brouillon : sans numéro, il se modifie et se supprime.
    pub brouillon: bool,
    /// Facture validée (pas une note de crédit) : le menu propose « Créditer ».
    pub peut_crediter: bool,
    /// Facture simple ou récurrente : le menu propose « Créer/voir la récurrence ».
    pub peut_recurrer: bool,
    pub numero: String,
    pub date: String,
    pub echeance: String,
    pub genre: &'static str,
    pub client: String,
    pub reference: String,
    /// Le chantier de la facture (sa référence), comme la colonne « Chantier/Installation » de Vertuoza.
    pub chantier: String,
    pub montant_ht: String,
    pub montant_ttc: String,
    pub statut: &'static str,
    pub classe_statut: &'static str,
    /// Sous la pastille : date du paiement, reste, retard ou échéance.
    pub detail_statut: String,
    /// Il reste quelque chose à encaisser : le menu propose « Ajouter le paiement ».
    pub a_recevoir: bool,
}

pub struct Liste {
    pub lignes: Vec<LigneListe>,
    /// La liste montre seulement les notes de crédit (entrée « Notes de crédit » du menu).
    pub notes_credit: bool,
    pub genre_choisi: &'static str,
    pub statut_choisi: &'static str,
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
    /// Reste à recevoir TTC des factures de la liste.
    pub total_a_recevoir: String,
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
    JOIN factures_etats et ON et.facture_id = f.id
    LEFT JOIN entreprises e ON e.id = f.client_entreprise_id
    LEFT JOIN contacts c ON c.id = f.client_contact_id
    LEFT JOIN chantiers ch ON ch.id = f.chantier_id
    LEFT JOIN devis dch ON dch.id = ch.devis_id
    WHERE ($1 = ''
           OR f.numero::text = $1
           OR f.reference ILIKE $2
           OR COALESCE(e.nom, concat_ws(' ', c.prenom, c.nom)) ILIKE $2)
      AND ($3 = '' OR f.genre::text = $3)
      AND ($4 = '' OR et.etat = $4)";

/// Une note de crédit diminue les totaux : elle compte en négatif.
const SIGNE: &str = "CASE WHEN f.genre = 'note_credit' THEN -1 ELSE 1 END";

async fn liste(mut db: Base, Query(r): Query<Recherche>, entetes: HeaderMap) -> Result<Html<String>, Erreur> {
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    let genre = GENRES.iter().map(|(v, _)| *v).find(|v| *v == r.genre.trim()).unwrap_or("");
    let statut_choisi = STATUTS.iter().map(|(v, _, _)| *v).find(|v| *v == r.statut.trim()).unwrap_or("");

    let (total, ht, tva, ttc, a_recevoir): (i64, i64, i64, i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*), COALESCE(sum({SIGNE} * f.montant_ht_cents), 0)::bigint,
                COALESCE(sum({SIGNE} * f.montant_tva_cents), 0)::bigint, COALESCE(sum({SIGNE} * f.montant_ttc_cents), 0)::bigint,
                COALESCE(sum(GREATEST(et.reste_cents, 0)), 0)::bigint
         {DEPUIS}"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(genre)
    .bind(statut_choisi)
    .fetch_one(&mut *db)
    .await?;
    let pages = ((total + PAR_PAGE - 1) / PAR_PAGE).max(1);
    let page: i64 = r.page.trim().parse().unwrap_or(1).clamp(1, pages);

    let lignes: Vec<LigneListeBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.id, f.genre::text AS genre, f.numero, f.date_facture, f.date_echeance,
                COALESCE(e.nom, NULLIF(trim(concat_ws(' ', c.prenom, c.nom)), ''), '') AS client,
                f.reference, COALESCE(NULLIF(dch.reference, ''), 'n° ' || ch.numero, '') AS chantier,
                {SIGNE} * f.montant_ht_cents AS montant_ht_cents, {SIGNE} * f.montant_ttc_cents AS montant_ttc_cents,
                et.paye_cents, et.credite_cents, et.reste_cents, et.dernier_paiement, et.etat, et.jours_retard
         {DEPUIS} ORDER BY f.numero IS NOT NULL, f.date_facture DESC, f.numero DESC NULLS FIRST, f.id DESC LIMIT $5 OFFSET $6"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(genre)
    .bind(statut_choisi)
    .bind(PAR_PAGE)
    .bind((page - 1) * PAR_PAGE)
    .fetch_all(&mut *db)
    .await?;

    let liste = Liste {
        lignes: lignes
            .into_iter()
            .map(|l| {
                let (libelle, classe) = statut(&l.solde.etat);
                LigneListe {
                    detail_statut: l.solde.detail(l.date_echeance),
                    a_recevoir: l.solde.a_recevoir(),
                    id: l.id,
                    brouillon: l.numero.is_none(),
                    peut_crediter: l.numero.is_some() && l.genre != "note_credit",
                    peut_recurrer: matches!(l.genre.as_str(), "simple" | "recurrente"),
                    // Le numéro avec l'année, comme sur le PDF et ce que le client cite (« 2026-125 »).
                    numero: match (l.genre.as_str(), l.numero) {
                        ("note_credit", Some(n)) => format!("NC {}", numero_affiche(l.date_facture, n)),
                        (_, Some(n)) => numero_affiche(l.date_facture, n),
                        (_, None) => "—".to_string(),
                    },
                    date: date_belge(l.date_facture),
                    echeance: l.date_echeance.map(date_belge).unwrap_or_default(),
                    genre: libelle_genre(&l.genre),
                    client: l.client,
                    reference: l.reference,
                    chantier: l.chantier,
                    montant_ht: format_euros(l.montant_ht_cents),
                    montant_ttc: format_euros(l.montant_ttc_cents),
                    statut: libelle,
                    classe_statut: classe,
                }
            })
            .collect(),
        requete: format!("q={}&genre={genre}&statut={statut_choisi}", encoder_url(&q)),
        notes_credit: genre == "note_credit",
        genre_choisi: genre,
        statut_choisi,
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
        total_a_recevoir: format_euros(a_recevoir),
    };
    let partiel = entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request");
    let html = if partiel { TableauListe { liste }.render()? } else { PageListe { liste }.render()? };
    Ok(Html(html))
}

/// « Nouvelle facture » : une facture simple en brouillon, sans numéro.
async fn nouvelle(mut db: Base) -> Result<Redirect, Erreur> {
    let taux = taux_par_defaut(&mut *db).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO factures (genre, date_facture, taux_tva_id) VALUES ('simple', (now() AT TIME ZONE 'Europe/Brussels')::date, $1) RETURNING id",
    )
    .bind(taux)
    .fetch_one(&mut *db)
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
    /// « 1 » : ouvre le panneau « Ajouter le paiement » (depuis la liste ou après une erreur de saisie).
    payer: String,
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
            "paiement" => "Le paiement partiel est enregistré.",
            "payee" => "Le paiement est enregistré : la facture est payée.",
            "paiement_retire" => "Le paiement est retiré.",
            "email_envoye" => "La pièce est envoyée par e-mail.",
            "peppol" => "La pièce est partie par Peppol.",
            "rappel_envoye" => "Le rappel de paiement est envoyé.",
            "email_enregistre" => "Mode test : l'e-mail est enregistré (Réglages › E-mails) mais n'est pas parti, car aucune clé Brevo n'est configurée. Marquez la pièce comme envoyée à la main si vous l'avez envoyée autrement.",
            _ => "",
        }
    }

    fn message_erreur(&self) -> &'static str {
        match self.erreur.as_str() {
            "peppol" => "L'envoi par Peppol a échoué : la raison est dans le bloc « Facturation électronique ».",
            "infos" => "Certaines informations n'ont pas été enregistrées (les autres l'ont été) : vérifiez la date, le client, les conditions, la référence (200 caractères) et la remarque (2 000 caractères).",
            "paiement_date" => "Le paiement n'est pas enregistré : la date est vide ou dans le futur.",
            "paiement_montant" => "Le paiement n'est pas enregistré : le montant doit être positif et ne pas dépasser le reste à recevoir.",
            "paiement_methode" => "Le paiement n'est pas enregistré : choisissez une méthode de paiement.",
            "paiement_remarque" => "Le paiement n'est pas enregistré : la remarque est trop longue (500 caractères au maximum).",
            "email_echec" => "L'e-mail n'a pas pu partir : le détail est dans Réglages › E-mails.",
            _ => "",
        }
    }
}

/// Le bloc « Montants » de la fiche : payé, crédité et reste à recevoir.
pub struct Montants {
    pub ttc: String,
    pub paye: String,
    pub credite: String,
    pub a_des_credits: bool,
    pub reste: String,
    /// La valeur proposée dans le champ « Montant » (« 1852,54 »).
    pub reste_saisie: String,
    pub a_recevoir: bool,
}

/// Une ligne de l'historique de la facture.
pub struct Evenement {
    pub quand: String,
    pub genre: &'static str,
    pub detail: String,
}

pub(crate) fn libelle_evenement(genre: &str) -> &'static str {
    match genre {
        "validation" => "Validation",
        "envoi" => "Envoi",
        "paiement" => "Paiement",
        "paiement_supprime" => "Paiement retiré",
        "email" => "E-mail",
        "rappel" => "Rappel",
        "client" => "Client",
        "peppol" => "Peppol",
        _ => "Événement",
    }
}

async fn historique(db: &mut PgConnection, id: i64) -> Result<Vec<Evenement>, sqlx::Error> {
    let lignes: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT genre, detail, to_char(created_at AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI')
         FROM facture_evenements WHERE facture_id = $1 ORDER BY created_at DESC, id DESC",
    )
    .bind(id)
    .fetch_all(&mut *db)
    .await?;
    Ok(lignes.into_iter().map(|(genre, detail, quand)| Evenement { quand, genre: libelle_evenement(&genre), detail }).collect())
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
    /// Facture simple ou récurrente : modèle possible d'une récurrence.
    peut_recurrer: bool,
    /// Reprise de Vertuoza.
    reprise: bool,
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
    /// État de paiement (étape 8-1).
    etat: String,
    detail_statut: String,
    jours_retard: i32,
    montants: Montants,
    paiements: Vec<paiements::LignePaiement>,
    methodes: Vec<Choix>,
    /// Le panneau « Ajouter le paiement » est ouvert.
    payer_ouvert: bool,
    aujourdhui: String,
    historique: Vec<Evenement>,
    /// L'adresse de la page client en ligne (vide pour un brouillon).
    lien_client: String,
    /// Facture en retard : (où en sont les rappels, un rappel est dû aujourd'hui).
    rappel: Option<(String, bool)>,
    /// L'archive légale (PDF et UBL tels qu'émis) d'une pièce validée.
    archive: Vec<crate::archive::Ligne>,
    /// Facturation électronique : l'adresse Peppol du client et ce qui empêche l'envoi (pièce validée seulement).
    peppol: Option<crate::peppol::Bloc>,
    /// Ce que fera « Valider et envoyer » sur un brouillon : par Peppol ou par e-mail, et pourquoi.
    envoi: Envoi,
    /// La note de la barre du bas d'un brouillon : les informations s'enregistrent toutes seules.
    enregistrement: Enregistrement,
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

async fn liens(db: &mut PgConnection, f: &Facture) -> Result<Vec<Lien>, sqlx::Error> {
    let mut liens = Vec::new();
    if let Some(chantier) = f.chantier_id {
        let numero: i64 = sqlx::query_scalar("SELECT numero FROM chantiers WHERE id = $1").bind(chantier).fetch_one(&mut *db).await?;
        liens.push(Lien { adresse: format!("/chantiers/{chantier}/finances"), texte: format!("{} n° {numero}", crate::profil::mot("Chantier")) });
    }
    if let (Some(chantier), Some(avancement)) = (f.chantier_id, f.avancement_id) {
        let (numero, reference): (i32, String) =
            sqlx::query_as("SELECT numero, reference FROM avancements WHERE id = $1").bind(avancement).fetch_one(&mut *db).await?;
        liens.push(Lien { adresse: format!("/chantiers/{chantier}/avancements/{avancement}"), texte: format!("Avancement n° {numero} ({reference})") });
    }
    if let Some(origine) = f.facture_origine_id {
        let (numero, date): (Option<i64>, NaiveDate) =
            sqlx::query_as("SELECT numero, date_facture FROM factures WHERE id = $1").bind(origine).fetch_one(&mut *db).await?;
        liens.push(Lien {
            adresse: format!("/factures/{origine}"),
            texte: format!("Note de crédit pour la facture n° {}", numero.map(|n| numero_affiche(date, n)).unwrap_or_default()),
        });
    }
    // Facture préparée par une récurrence, ou modèle d'une récurrence.
    let recurrence: Option<(i64, NaiveDate)> = sqlx::query_as(
        "SELECT r.modele_id, COALESCE(f.date_prevue, f.date_facture) FROM factures f JOIN recurrences r ON r.id = f.recurrence_id WHERE f.id = $1",
    )
    .bind(f.id)
    .fetch_optional(&mut *db)
    .await?;
    if let Some((modele, prevue)) = recurrence {
        liens.push(Lien { adresse: format!("/factures/{modele}/recurrence"), texte: format!("Facture récurrente prévue le {}", date_belge(prevue)) });
    }
    let modele: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM recurrences WHERE modele_id = $1)").bind(f.id).fetch_one(&mut *db).await?;
    if modele {
        liens.push(Lien { adresse: format!("/factures/{}/recurrence", f.id), texte: "Modèle d'une récurrence".to_string() });
    }
    let notes: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, numero FROM factures WHERE facture_origine_id = $1 ORDER BY id").bind(f.id).fetch_all(&mut *db).await?;
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
pub(crate) async fn nom_client(db: &mut PgConnection, entreprise: Option<i64>, contact: Option<i64>) -> Result<String, sqlx::Error> {
    let nom: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE((SELECT nom FROM entreprises WHERE id = $1), (SELECT trim(concat_ws(' ', prenom, nom)) FROM contacts WHERE id = $2))",
    )
    .bind(entreprise)
    .bind(contact)
    .fetch_one(&mut *db)
    .await?;
    Ok(nom.unwrap_or_default())
}

async fn afficher(mut db: Base, Path(id): Path<i64>, Query(retour): Query<Retour>) -> Result<Html<String>, Erreur> {
    let page = construire_page(&mut *db, id, &retour, Enregistrement::default()).await?;
    Ok(Html(page.render()?))
}

async fn construire_page(db: &mut PgConnection, id: i64, retour: &Retour, enregistrement: Enregistrement) -> Result<Page, Erreur> {
    let f = charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    let r = Referentiel::charger(&mut *db).await?;
    let lignes = charger_lignes(&mut *db, id).await?;
    let a_completer = if f.brouillon() { verifier(&mut *db, &f, &lignes).await? } else { Vec::new() };
    let solde = paiements::solde(&mut *db, id).await?;
    let (libelle, classe) = statut(&solde.etat);
    let ttc: i64 = sqlx::query_scalar("SELECT montant_ttc_cents FROM factures WHERE id = $1").bind(id).fetch_one(&mut *db).await?;
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await?;
    let montants = Montants {
        ttc: format_euros(ttc),
        paye: format_euros(solde.paye_cents),
        credite: format_euros(solde.credite_cents),
        a_des_credits: solde.credite_cents != 0,
        reste: format_euros(solde.reste_cents.max(0)),
        reste_saisie: crate::avancement_calcul::euros_a_saisir(solde.reste_cents.max(0)),
        a_recevoir: solde.a_recevoir(),
    };
    let methodes = paiements::methodes(&mut *db).await?;
    let peppol = crate::peppol::controler(&mut *db, &f).await?;
    let envoi = envoi_choisi(peppol.as_ref());
    let form = InfosForm::depuis(&f);
    let conditions_texte = f
        .conditions_paiement_id
        .and_then(|c| r.options.conditions.iter().find(|(i, _)| *i == c).map(|(_, t)| t.clone()))
        .unwrap_or_default();
    let page = Page {
        id,
        titre: titre(&f.genre, f.numero, f.date_facture),
        genre: libelle_genre(&f.genre),
        statut: libelle,
        classe_statut: classe,
        brouillon: f.brouillon(),
        peut_marquer_envoyee: f.statut == "a_envoyer",
        peut_crediter: f.numero.is_some() && f.genre != "note_credit",
        peut_recurrer: f.peut_recurrer(),
        reprise: f.reprise,
        numero: f.numero.map(|n| numero_affiche(f.date_facture, n)).unwrap_or_default(),
        date: date_belge(f.date_facture),
        echeance: f.date_echeance.map(date_belge).unwrap_or_default(),
        vcs: f.vcs.clone().unwrap_or_default(),
        liens: liens(&mut *db, &f).await?,
        clients: r.choix_personnes(&form.client),
        conditions: choix_base(&r.options.conditions, &form.conditions_paiement_id),
        taux_tva: choix_base(&r.options.taux_tva, &form.taux_tva_id),
        client: nom_client(&mut *db, f.client_entreprise_id, f.client_contact_id).await?,
        conditions_texte,
        a_completer,
        message_ok: retour.message_ok(),
        message_erreur: retour.message_erreur(),
        lignes: construire_lignes(id, f.brouillon() && f.lignes_libres(), &lignes, &crate::fiches::taux_courts(&r.options.taux_tva), String::new()),
        form,
        detail_statut: solde.detail(f.date_echeance),
        jours_retard: solde.jours_retard,
        etat: solde.etat.clone(),
        montants,
        paiements: paiements::lignes(&mut *db, id).await?,
        methodes: methodes.iter().enumerate().map(|(i, (v, l))| Choix { valeur: v.to_string(), libelle: l.clone(), choisi: i == 0 }).collect(),
        payer_ouvert: retour.payer == "1" && solde.a_recevoir(),
        aujourdhui: aujourdhui.to_string(),
        historique: historique(&mut *db, id).await?,
        lien_client: if f.numero.is_some() { crate::page_client::lien(&mut *db, crate::emails::Piece::Facture, id).await? } else { String::new() },
        rappel: if solde.etat == "en_retard" { crate::rappels::resume_facture(&mut *db, id).await? } else { None },
        archive: if f.numero.is_some() { crate::archive::lignes(&mut *db, &f).await? } else { Vec::new() },
        peppol,
        envoi,
        enregistrement,
    };
    Ok(page)
}

/// Enregistre l'en-tête d'un brouillon (date, référence, client, conditions…). Le formulaire s'envoie tout seul (htmx)
/// à chaque changement : chaque valeur valable est gardée, une valeur refusée garde la précédente (avec un message).
/// La réponse est la fiche entière ; htmx en reprend la barre du bas (note « Enregistré », bouton Valider), le bloc
/// « Avant de valider » et le bloc Peppol, qui dépendent du client et des conditions.
async fn enregistrer(mut db: Base, Path(id): Path<i64>, entetes: HeaderMap, Form(form): Form<InfosForm>) -> Result<Response, Erreur> {
    let f = charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Cette facture est validée : elle ne se modifie plus. Corrigez-la par une note de crédit."));
    }
    let r = Referentiel::charger(&mut *db).await?;
    let mut erreurs: Vec<String> = Vec::new();
    let date = NaiveDate::parse_from_str(form.date.trim(), "%Y-%m-%d").unwrap_or_else(|_| {
        erreurs.push("La date de la facture n'est pas une date.".to_string());
        f.date_facture
    });
    let client = match form.client.trim() {
        "" => None,
        texte => Personne::lire(texte).filter(|p| r.existe(*p)).or_else(|| {
            erreurs.push("Choisissez le client dans la liste.".to_string());
            Personne::depuis(f.client_entreprise_id, f.client_contact_id)
        }),
    };
    let mut identifiant = |texte: &str, liste: &[(i64, String)], avant: Option<i64>, message: &str| -> Option<i64> {
        match texte.trim() {
            "" => None,
            t => t.parse::<i64>().ok().filter(|i| liste.iter().any(|(x, _)| x == i)).or_else(|| {
                erreurs.push(message.to_string());
                avant
            }),
        }
    };
    let conditions =
        identifiant(&form.conditions_paiement_id, &r.options.conditions, f.conditions_paiement_id, "Choisissez les conditions de paiement dans la liste.");
    let taux = identifiant(&form.taux_tva_id, &r.options.taux_tva, f.taux_tva_id, "Choisissez le taux de TVA dans la liste.");
    let mut texte = |valeur: &str, max: usize, avant: &str, message: &str| -> String {
        let valeur = valeur.trim();
        if valeur.chars().count() > max {
            erreurs.push(message.to_string());
            avant.to_string()
        } else {
            valeur.to_string()
        }
    };
    let reference = texte(&form.reference, LONGUEUR_MAX_REFERENCE, &f.reference, "La référence dépasse 200 caractères.");
    let responsable = texte(&form.responsable, LONGUEUR_MAX_REFERENCE, &f.responsable, "Le responsable dépasse 200 caractères.");
    let remarque = texte(&form.remarque, LONGUEUR_MAX_TEXTE, &f.remarque, "La remarque dépasse 2 000 caractères.");
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
    .execute(&mut *db)
    .await?;
    if crate::composants::par_htmx(&entetes) {
        let enregistrement = Enregistrement::maintenant(&mut *db, erreurs).await?;
        let page = construire_page(&mut *db, id, &Retour::default(), enregistrement).await?;
        return Ok(Html(page.render()?).into_response());
    }
    let suite = if erreurs.is_empty() { "ok=infos" } else { "erreur=infos" };
    Ok(Redirect::to(&format!("/factures/{id}?{suite}")).into_response())
}

// ─── Lignes saisies à la main (facture simple, note de crédit) ──────────────

/// La facture, si ses lignes peuvent encore être modifiées à la main.
async fn facture_modifiable(db: &mut PgConnection, id: i64) -> Result<Facture, Erreur> {
    let f = charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Cette facture est validée : elle ne se modifie plus. Corrigez-la par une note de crédit."));
    }
    if !f.lignes_libres() {
        return Err(Erreur::conflit("Les lignes de cette facture sont calculées automatiquement : elles ne se modifient pas."));
    }
    Ok(f)
}

async fn rendre_lignes(db: &mut PgConnection, id: i64, message: String, seulement_maj: bool) -> Result<Html<String>, Erreur> {
    let lignes = charger_lignes(&mut *db, id).await?;
    let options = crate::fiches::Options::charger(&mut *db, crate::fiches::Genre::Entreprise).await?;
    let vue = construire_lignes(id, true, &lignes, &crate::fiches::taux_courts(&options.taux_tva), message);
    let html = if seulement_maj { LignesMaj { lignes: vue }.render()? } else { LignesSeules { lignes: vue }.render()? };
    Ok(Html(html))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Ajout {
    genre: String,
}

async fn ajouter_ligne(mut db: Base, Path(id): Path<i64>, Form(a): Form<Ajout>) -> Result<Html<String>, Erreur> {
    let f = facture_modifiable(&mut *db, id).await?;
    let genre = match a.genre.as_str() {
        "titre" => "titre",
        "texte" => "texte",
        _ => "poste",
    };
    let mut tx = db.begin().await?;
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
    rendre_lignes(&mut *db, id, String::new(), false).await
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
        return Err("Choisissez un taux de TVA.".to_string());
    }
    Ok((description, quantite, unite, prix, taux))
}

async fn modifier_ligne(
    mut db: Base,
    Path((id, ligne_id)): Path<(i64, i64)>,
    Form(s): Form<SaisieLigne>,
) -> Result<Html<String>, Erreur> {
    facture_modifiable(&mut *db, id).await?;
    let genre: Option<String> = sqlx::query_scalar("SELECT genre FROM facture_lignes WHERE id = $1 AND facture_id = $2")
        .bind(ligne_id)
        .bind(id)
        .fetch_optional(&mut *db)
        .await?;
    let genre = genre.ok_or_else(Erreur::introuvable)?;
    let options = crate::fiches::Options::charger(&mut *db, crate::fiches::Genre::Entreprise).await?;
    let message = match lire_ligne(&s, genre == "poste", &options.taux_tva) {
        Err(m) => m,
        Ok((description, quantite, unite, prix, taux)) => {
            let total = match (quantite, prix) {
                (Some(q), Some(p)) => calcul::total_ligne_cents(q, p),
                _ => 0,
            };
            let mut tx = db.begin().await?;
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
    rendre_lignes(&mut *db, id, message, true).await
}

async fn supprimer_ligne(mut db: Base, Path((id, ligne_id)): Path<(i64, i64)>) -> Result<Html<String>, Erreur> {
    facture_modifiable(&mut *db, id).await?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM facture_lignes WHERE id = $1 AND facture_id = $2").bind(ligne_id).bind(id).execute(&mut *tx).await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    rendre_lignes(&mut *db, id, String::new(), false).await
}

// ─── Valider, envoyer, supprimer ────────────────────────────────────────────

/// Ce qui manque pour valider le brouillon (liste vide : la facture peut être validée).
pub(crate) async fn verifier(db: &mut PgConnection, f: &Facture, lignes: &[LigneBase]) -> Result<Vec<String>, sqlx::Error> {
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
    // Le régime de TVA de la société décide des taux permis.
    if crate::societe::en_franchise(&mut *db).await? {
        if postes.iter().any(|l| !l.franchise) {
            manques.push("Des lignes au taux « Franchise de TVA » (votre société est en franchise de TVA)".to_string());
        }
    } else if postes.iter().any(|l| l.franchise) {
        manques.push("Des lignes sans le taux « Franchise de TVA », réservé aux sociétés en franchise (Réglages › Société)".to_string());
    }
    let t = totaux(lignes);
    if !postes.is_empty() && t.ttc_cents <= 0 {
        manques.push("Un total TTC positif".to_string());
    }
    if crate::mentions_tva::combine_autoliquidation_et_6(lignes) {
        manques.push("Pas d'autoliquidation et de TVA à 6 % sur la même facture (le 6 % exige un consommateur final, l'autoliquidation un assujetti qui dépose des déclarations périodiques)".to_string());
    }
    if let Some(origine) = f.facture_origine_id {
        // Une note de crédit ne rend jamais plus que ce que la facture n'a pas encore crédité.
        let reste = reste_a_crediter(&mut *db, origine, Some(f.id)).await?;
        if t.ttc_cents > reste {
            manques.push(format!("Un total TTC d'au plus {} (ce qui reste à créditer sur la facture)", format_euros(reste)));
        }
    }
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(&mut *db).await?;
    if f.date_facture > aujourdhui {
        manques.push("Une date qui n'est pas dans le futur".to_string());
    }
    if let Some(derniere) = derniere_date(&mut *db, f.genre == "note_credit").await?
        && f.date_facture < derniere
    {
        manques.push(format!("Une date au plus tôt le {} (date de la dernière pièce numérotée)", date_belge(derniere)));
    }
    // La numérotation est annuelle : une année déjà close (une pièce de l'année suivante existe) ne se rouvre pas.
    let compteur = if f.genre == "note_credit" { "note_credit" } else { "facture" };
    let annee_compteur: Option<i32> = sqlx::query_scalar("SELECT annee FROM compteurs WHERE organisation_id = app_organisation() AND nom = $1").bind(compteur).fetch_one(&mut *db).await?;
    if let Some(annee) = annee_compteur
        && f.date_facture.year() < annee
    {
        manques.push(format!("Une date en {annee} au plus tôt (la numérotation de {annee} est commencée)"));
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
pub(crate) async fn reste_a_crediter(db: &mut PgConnection, facture_id: i64, sauf: Option<i64>) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT f.montant_ttc_cents - COALESCE((SELECT sum(n.montant_ttc_cents) FROM factures n
                                                WHERE n.facture_origine_id = f.id AND n.numero IS NOT NULL
                                                  AND n.id IS DISTINCT FROM $2), 0)::bigint
         FROM factures f WHERE f.id = $1",
    )
    .bind(facture_id)
    .bind(sauf)
    .fetch_one(&mut *db)
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApresValidation {
    /// « envoyer » : l'envoyer juste après par le canal que Raven choisit (`envoi_choisi`) ; « email » : ouvrir
    /// l'envoi par e-mail juste après (comme l'envoi dans Vertuoza) ; « peppol » : l'envoyer par Peppol juste après.
    puis: String,
}

/// Ce que fait « Valider et envoyer » (23/10/2026, retour de Mahx : un seul bouton qui dit quoi faire ensuite).
pub struct Envoi {
    /// Par Peppol (sinon par e-mail : l'écran d'envoi s'ouvre, déjà rempli).
    pub peppol: bool,
    /// Le texte du bouton : « Valider et envoyer par Peppol » ou « Valider et envoyer par e-mail ».
    pub bouton: &'static str,
    /// La raison du choix, en clair.
    pub raison: String,
}

/// Raven choisit seul comment la pièce part : par Peppol quand elle le peut (client entreprise avec une adresse Peppol
/// valable, envoi configuré, organisation inscrite et vérifiée), sinon par e-mail, avec la raison.
pub(crate) fn envoi_choisi(bloc: Option<&crate::peppol::Bloc>) -> Envoi {
    let email = |raison: &str| Envoi { peppol: false, bouton: "Valider et envoyer par e-mail", raison: raison.to_string() };
    match bloc {
        Some(b) if b.pourra_partir() => {
            Envoi { peppol: true, bouton: "Valider et envoyer par Peppol", raison: format!("Par Peppol, à l'adresse {} du client.", b.client) }
        }
        Some(b) if b.particulier => email("Par e-mail : un particulier ne reçoit pas de facture par Peppol."),
        Some(b) if !b.prete() => email("Par e-mail : l'envoi par Peppol n'est pas encore possible (voir « Facturation électronique »)."),
        Some(b) if !b.envoi_actif => email("Par e-mail : l'envoi par Peppol n'est pas configuré sur ce serveur."),
        Some(_) => email("Par e-mail : votre entreprise n'est pas encore inscrite sur Peppol (Réglages › Peppol)."),
        None => email("Par e-mail."),
    }
}

async fn valider(mut db: Base, Path(id): Path<i64>, Form(apres): Form<ApresValidation>) -> Result<Redirect, Erreur> {
    let note_credit = {
        let f = charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
        if !f.brouillon() {
            return Err(Erreur::conflit("Cette facture est déjà validée."));
        }
        f.genre == "note_credit"
    };
    let compteur = if note_credit { "note_credit" } else { "facture" };
    let mut tx = db.begin().await?;
    // Verrouille le compteur d'abord : deux validations en même temps passent l'une après l'autre.
    sqlx::query("SELECT valeur FROM compteurs WHERE organisation_id = app_organisation() AND nom = $1 FOR UPDATE").bind(compteur).execute(&mut *tx).await?;
    sqlx::query("SELECT id FROM factures WHERE id = $1 FOR UPDATE").bind(id).execute(&mut *tx).await?;
    let f = charger(&mut *tx, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Cette facture est déjà validée."));
    }
    let lignes = charger_lignes(&mut *tx, id).await?;
    let manques = verifier(&mut *tx, &f, &lignes).await?;
    // La date de la dernière pièce, relue sous le verrou du compteur.
    let derniere = derniere_date(&mut *tx, note_credit).await?;
    if !manques.is_empty() || derniere.is_some_and(|d| f.date_facture < d) {
        return Err(Erreur::conflit("Cette facture ne peut pas encore être validée : complétez ce qui est indiqué sur la fiche."));
    }
    // Numérotation annuelle (réglage d'Ubique dans Vertuoza) : la première pièce d'une nouvelle année repart à 1.
    let annee = f.date_facture.year();
    let (numero, annee_compteur): (i64, Option<i32>) = sqlx::query_as(
        "UPDATE compteurs SET valeur = CASE WHEN annee IS NULL OR annee < $2 THEN 1 ELSE valeur + 1 END,
                              annee = GREATEST(COALESCE(annee, $2), $2)
         WHERE organisation_id = app_organisation() AND nom = $1 RETURNING valeur, annee",
    )
    .bind(compteur)
    .bind(annee)
    .fetch_one(&mut *tx)
    .await?;
    if annee_compteur != Some(annee) {
        return Err(Erreur::conflit("Cette facture ne peut pas encore être validée : complétez ce qui est indiqué sur la fiche."));
    }
    let (genre_condition, jours): (String, i32) = sqlx::query_as("SELECT \"type\"::text, jours FROM conditions_paiement WHERE id = $1")
        .bind(f.conditions_paiement_id)
        .fetch_one(&mut *tx)
        .await?;
    // Une note de crédit se rembourse : pas d'échéance ni de communication structurée à payer.
    let (echeance, vcs) = if note_credit {
        (None, None)
    } else {
        (Some(facture_calcul::echeance(f.date_facture, &genre_condition, jours)), Some(facture_calcul::vcs_annuelle(annee, numero)))
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
    paiements::noter(&mut tx, id, "validation", &format!("Validée : {}", titre(&f.genre, Some(numero), f.date_facture))).await?;
    tx.commit().await?;
    // Le PDF et l'UBL émis sont archivés tout de suite. Si cela échoue, la facture reste valide : l'archive se fera au
    // premier téléchargement, envoi ou affichage (même contenu, la facture ne change plus).
    if let Some(validee) = charger(&mut *db, id).await?
        && crate::archive::garantir(&mut *db, &validee).await.is_err()
    {
        eprintln!("Archive de la facture {id} : pas encore faite, elle se fera à la prochaine utilisation.");
    }
    // « Valider et envoyer » : le canal est choisi sur la pièce validée, pas d'après le formulaire.
    let puis = match (apres.puis.as_str(), charger(&mut *db, id).await?) {
        ("envoyer", Some(validee)) => {
            if envoi_choisi(crate::peppol::controler(&mut *db, &validee).await?.as_ref()).peppol { "peppol" } else { "email" }
        }
        (puis, _) => puis,
    };
    if puis == "email" {
        return Ok(Redirect::to(&format!("/factures/{id}/email")));
    }
    if puis == "peppol"
        && let Some(validee) = charger(&mut *db, id).await?
    {
        // La facture est validée quoi qu'il arrive ; un envoi refusé se lit dans le bloc « Facturation électronique ».
        return Ok(Redirect::to(&match crate::peppol::envoyer_facture(&mut db, &validee).await {
            Ok(true) => format!("/factures/{id}?ok=peppol#peppol"),
            Ok(false) => format!("/factures/{id}?erreur=peppol#peppol"),
            Err(_) => format!("/factures/{id}?ok=validee#peppol"),
        }));
    }
    Ok(Redirect::to(&format!("/factures/{id}?ok=validee")))
}

/// Marquer la facture comme envoyée à la main (envoyée autrement que par l'application).
async fn marquer_envoyee(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let mut tx = db.begin().await?;
    if !marquer_envoyee_dans(&mut tx, id, "Marquée comme envoyée (à la main)").await? {
        charger(&mut *tx, id).await?.ok_or_else(Erreur::introuvable)?;
        return Err(Erreur::conflit("Seule une facture validée « À envoyer » peut être marquée comme envoyée."));
    }
    tx.commit().await?;
    Ok(Redirect::to(&format!("/factures/{id}?ok=envoyee")))
}

/// Passe une facture « À envoyer » à « Envoyée » et le note dans l'historique. Faux si elle n'était pas « À envoyer ».
pub(crate) async fn marquer_envoyee_dans(tx: &mut Transaction<'_, Postgres>, id: i64, detail: &str) -> Result<bool, sqlx::Error> {
    let fait = sqlx::query("UPDATE factures SET statut = 'envoyee', envoyee_le = now(), updated_at = now() WHERE id = $1 AND statut = 'a_envoyer'")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    if fait.rows_affected() == 0 {
        return Ok(false);
    }
    paiements::noter(tx, id, "envoi", detail).await?;
    Ok(true)
}

/// Seul un brouillon (sans numéro) se supprime. La facture finale emporte l'avancement de solde créé pour elle.
async fn supprimer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let f = charger(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    if !f.brouillon() {
        return Err(Erreur::conflit("Une facture validée ne se supprime jamais : corrigez-la par une note de crédit."));
    }
    let mut tx = db.begin().await?;
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

// ─── Note de crédit ─────────────────────────────────────────────────────────

/// « Créditer » une facture validée : une note de crédit en brouillon qui reprend toutes ses lignes.
/// Pour un crédit partiel, on ajuste ou supprime des lignes avant de valider (jamais plus que ce qui reste à créditer).
async fn crediter(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT id FROM factures WHERE id = $1 FOR UPDATE").bind(id).execute(&mut *tx).await?;
    let f = charger(&mut *tx, id).await?.ok_or_else(Erreur::introuvable)?;
    let Some(numero) = f.numero.filter(|_| f.genre != "note_credit") else {
        return Err(Erreur::conflit("Seule une facture validée peut être créditée."));
    };
    let brouillon: Option<i64> = sqlx::query_scalar("SELECT id FROM factures WHERE facture_origine_id = $1 AND statut = 'brouillon'")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(note) = brouillon {
        return Ok(Redirect::to(&format!("/factures/{note}")));
    }
    if reste_a_crediter(&mut *tx, id, None).await? <= 0 {
        return Err(Erreur::conflit("Cette facture est déjà entièrement créditée."));
    }
    let note: i64 = sqlx::query_scalar(
        "INSERT INTO factures (genre, date_facture, reference, responsable, client_entreprise_id, client_contact_id,
                               conditions_paiement_id, taux_tva_id, chantier_id, facture_origine_id)
         SELECT 'note_credit', (now() AT TIME ZONE 'Europe/Brussels')::date, $2, responsable, client_entreprise_id, client_contact_id,
                conditions_paiement_id, taux_tva_id, chantier_id, id
         FROM factures WHERE id = $1 RETURNING id",
    )
    .bind(id)
    .bind(format!("Note de crédit pour la facture n° {}", numero_affiche(f.date_facture, numero)))
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO facture_lignes (facture_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5,
                                     taux_tva_id, taux_centiemes, total_cents)
         SELECT $1, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id, taux_centiemes, total_cents
         FROM facture_lignes WHERE facture_id = $2",
    )
    .bind(note)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    recalculer(&mut tx, note).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/factures/{note}?ok=credit")))
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
        .route("/factures/{id}/crediter", post(crediter))
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
            franchise: false,
        }
    }

    #[test]
    fn les_titres() {
        let jour = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        assert_eq!(titre("simple", Some(125), jour), "Facture n° 2026-125");
        assert_eq!(titre("acompte", Some(126), jour), "Facture n° 2026-126");
        assert_eq!(titre("note_credit", Some(6), jour), "Note de crédit n° 2026-6");
        assert_eq!(titre("finale", None, jour), "Facture finale (brouillon)");
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
        assert_eq!(numeros(&[ligne("poste", 1, Some(600)), ligne("poste", 1, Some(600))]), vec!["1", "2"]);
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
