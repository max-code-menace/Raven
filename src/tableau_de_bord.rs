//! Le tableau de bord (étape 9-5), d'après la planche TableauDeBord et les widgets de Vertuoza : des blocs « Mes devis »,
//! « Mes factures », « Mes chantiers », « Mes notes de crédit » (un nombre, un montant, et le lien vers la liste
//! filtrée), « À surveiller » (ce qui demande une action aujourd'hui) et « Ce mois-ci ».
//!
//! « À surveiller » remplace les notifications de Vertuoza : elle est recalculée à chaque affichage (pas de « lu /
//! non lu ») et disparaît quand l'action est faite.

use askama::Template;
use axum::{extract::State, response::Html};
use chrono::{Datelike, NaiveDate};
use sqlx::PgPool;

use crate::commun::format_euros;
use crate::{AppState, Erreur};

/// Un compteur : nombre et montant.
pub struct Compte {
    pub nombre: i64,
    pub montant: String,
}

fn compte((nombre, centimes): (i64, i64)) -> Compte {
    Compte { nombre, montant: format_euros(centimes) }
}

/// Une ligne de « À surveiller » : couleur du point (statut), texte, lien.
pub struct Alerte {
    pub couleur: &'static str,
    pub texte: String,
    pub lien: String,
    pub action: &'static str,
}

#[derive(Template)]
#[template(path = "home.html")]
struct Page {
    devis_attente: Compte,
    devis_a_envoyer: Compte,
    devis_brouillons: Compte,
    factures_retard: Compte,
    factures_a_envoyer: Compte,
    factures_a_encaisser: Compte,
    chantiers_en_cours: i64,
    chantiers_reste_a_facturer: String,
    avancements_brouillons: Compte,
    notes_brouillons: Compte,
    notes_annee: Compte,
    annee: i32,
    mois_texte: String,
    mois_facture: String,
    mois_encaisse: String,
    mois_devis_envoyes: i64,
    mois_devis_acceptes: i64,
    mois_conversion: String,
    alertes: Vec<Alerte>,
}

const MOIS: [&str; 12] = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];

async fn deux(db: &PgPool, sql: &'static str) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(sql).fetch_one(db).await
}

pub async fn afficher(State(state): State<AppState>) -> Result<Html<String>, Erreur> {
    let db = &state.db;
    let aujourdhui: NaiveDate = sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(db).await?;
    let debut_mois = aujourdhui.with_day(1).expect("le 1er existe");

    // Devis (sans les avenants, qui sont sur leur chantier).
    let devis_attente = deux(db, "SELECT count(*), COALESCE(sum(montant_ht_cents), 0)::bigint FROM devis WHERE chantier_id IS NULL AND statut IN ('envoye', 'envoi_en_cours', 'signe')").await?;
    let devis_a_envoyer = deux(db, "SELECT count(*), COALESCE(sum(montant_ht_cents), 0)::bigint FROM devis WHERE chantier_id IS NULL AND statut IN ('a_envoyer', 'echec_envoi')").await?;
    let devis_brouillons = deux(db, "SELECT count(*), COALESCE(sum(montant_ht_cents), 0)::bigint FROM devis WHERE chantier_id IS NULL AND statut = 'brouillon'").await?;
    // Factures.
    let factures_retard = deux(db, "SELECT count(*), COALESCE(sum(reste_cents), 0)::bigint FROM factures_etats WHERE etat = 'en_retard'").await?;
    let factures_a_envoyer = deux(db, "SELECT count(*), COALESCE(sum(montant_ttc_cents), 0)::bigint FROM factures WHERE statut = 'a_envoyer' AND genre <> 'note_credit'").await?;
    let factures_a_encaisser = deux(db, "SELECT count(*), COALESCE(sum(reste_cents), 0)::bigint FROM factures_etats WHERE reste_cents > 0").await?;
    // Chantiers.
    let (chantiers_en_cours, reste): (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(
                    d.montant_ht_cents
                    + COALESCE((SELECT sum(av.montant_ht_cents) FROM devis av WHERE av.chantier_id = ch.id AND av.statut = 'accepte'), 0)
                    - COALESCE((SELECT sum(CASE WHEN f.genre = 'note_credit' THEN -f.montant_ht_cents ELSE f.montant_ht_cents END)
                                FROM factures f WHERE f.chantier_id = ch.id AND f.numero IS NOT NULL), 0)), 0)::bigint
         FROM chantiers ch JOIN devis d ON d.id = ch.devis_id WHERE ch.statut = 'en_cours'",
    )
    .fetch_one(db)
    .await?;
    let avancements_brouillons = deux(db, "SELECT count(*), COALESCE(sum(montant_ht_cents), 0)::bigint FROM avancements WHERE statut = 'brouillon'").await?;
    // Notes de crédit.
    let notes_brouillons = deux(db, "SELECT count(*), COALESCE(sum(montant_ttc_cents), 0)::bigint FROM factures WHERE genre = 'note_credit' AND numero IS NULL").await?;
    let notes_annee: (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(montant_ttc_cents), 0)::bigint FROM factures
         WHERE genre = 'note_credit' AND numero IS NOT NULL AND extract(year FROM date_facture) = $1",
    )
    .bind(f64::from(aujourdhui.year()))
    .fetch_one(db)
    .await?;
    // Ce mois-ci.
    let mois_facture: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(CASE WHEN genre = 'note_credit' THEN -montant_ht_cents ELSE montant_ht_cents END), 0)::bigint
         FROM factures WHERE numero IS NOT NULL AND date_facture BETWEEN $1 AND $2",
    )
    .bind(debut_mois)
    .bind(aujourdhui)
    .fetch_one(db)
    .await?;
    let mois_encaisse: i64 = sqlx::query_scalar("SELECT COALESCE(sum(montant_cents), 0)::bigint FROM paiements WHERE date_paiement BETWEEN $1 AND $2")
        .bind(debut_mois)
        .bind(aujourdhui)
        .fetch_one(db)
        .await?;
    let (mois_devis_envoyes, mois_devis_acceptes): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE statut = 'accepte') FROM devis
         WHERE chantier_id IS NULL AND statut <> 'brouillon' AND date_devis BETWEEN $1 AND $2",
    )
    .bind(debut_mois)
    .bind(aujourdhui)
    .fetch_one(db)
    .await?;

    let page = Page {
        devis_attente: compte(devis_attente),
        devis_a_envoyer: compte(devis_a_envoyer),
        devis_brouillons: compte(devis_brouillons),
        factures_retard: compte(factures_retard),
        factures_a_envoyer: compte(factures_a_envoyer),
        factures_a_encaisser: compte(factures_a_encaisser),
        chantiers_en_cours,
        chantiers_reste_a_facturer: format_euros(reste),
        avancements_brouillons: compte(avancements_brouillons),
        notes_brouillons: compte(notes_brouillons),
        notes_annee: compte(notes_annee),
        annee: aujourdhui.year(),
        mois_texte: format!("{} {}", MOIS[aujourdhui.month0() as usize], aujourdhui.year()),
        mois_facture: format_euros(mois_facture),
        mois_encaisse: format_euros(mois_encaisse),
        mois_devis_envoyes,
        mois_devis_acceptes,
        mois_conversion: crate::statistiques::part(mois_devis_acceptes, mois_devis_envoyes),
        alertes: alertes(db).await?,
    };
    Ok(Html(page.render()?))
}

/// Ce qui demande une action aujourd'hui, le plus urgent d'abord.
async fn alertes(db: &PgPool) -> Result<Vec<Alerte>, sqlx::Error> {
    let mut alertes = Vec::new();
    let a_relancer: Vec<crate::rappels::Ligne> = crate::rappels::factures_en_retard(db).await?.into_iter().filter(|l| l.a_relancer).collect();
    if !a_relancer.is_empty() {
        let total: i64 = a_relancer.iter().map(|l| l.reste_cents).sum();
        alertes.push(Alerte {
            couleur: "#953A0A",
            texte: format!("{} facture(s) en retard à relancer aujourd'hui ({} à recevoir).", a_relancer.len(), format_euros(total)),
            lien: "/factures/rappels".to_string(),
            action: "Envoyer les rappels",
        });
    }
    let signes: Vec<(i64, i64, i32, String)> = sqlx::query_as(
        "SELECT id, numero, extract(year FROM date_devis)::int, signe_par FROM devis WHERE statut = 'signe' ORDER BY signe_le",
    )
    .fetch_all(db)
    .await?;
    for (id, numero, annee, par) in signes {
        alertes.push(Alerte {
            couleur: "#1C6B3C",
            texte: format!("Le devis {annee}-{numero} est signé en ligne{} : il reste à l'accepter.", if par.is_empty() { String::new() } else { format!(" par {par}") }),
            lien: format!("/devis/{id}"),
            action: "Accepter",
        });
    }
    let sans_reponse: Vec<(i64, i64, i32, i32)> = sqlx::query_as(
        "SELECT id, numero, extract(year FROM date_devis)::int, ((now() AT TIME ZONE 'Europe/Brussels')::date - (statut_date AT TIME ZONE 'Europe/Brussels')::date)
         FROM devis WHERE chantier_id IS NULL AND statut = 'envoye' AND statut_date < now() - interval '21 days' ORDER BY statut_date",
    )
    .fetch_all(db)
    .await?;
    for (id, numero, annee, jours) in sans_reponse {
        alertes.push(Alerte {
            couleur: "#7A5100",
            texte: format!("Le devis {annee}-{numero} est envoyé depuis {jours} jours, sans réponse."),
            lien: format!("/devis/{id}"),
            action: "Ouvrir",
        });
    }
    let a_envoyer: i64 = sqlx::query_scalar("SELECT count(*) FROM factures WHERE statut = 'a_envoyer'").fetch_one(db).await?;
    if a_envoyer > 0 {
        alertes.push(Alerte {
            couleur: "#7A5100",
            texte: format!("{a_envoyer} pièce(s) validée(s) pas encore envoyée(s) au client."),
            lien: "/factures?statut=a_envoyer".to_string(),
            action: "Voir",
        });
    }
    let brouillons: Vec<(i64, i64, i32, i64)> = sqlx::query_as(
        "SELECT a.id, ch.id, a.numero, ch.numero FROM avancements a JOIN chantiers ch ON ch.id = a.chantier_id WHERE a.statut = 'brouillon' ORDER BY a.id",
    )
    .fetch_all(db)
    .await?;
    for (id, chantier, numero, chantier_numero) in brouillons {
        alertes.push(Alerte {
            couleur: "#4A5058",
            texte: format!("L'avancement n° {numero} du chantier n° {chantier_numero} est resté en brouillon."),
            lien: format!("/chantiers/{chantier}/avancements/{id}"),
            action: "Reprendre",
        });
    }
    Ok(alertes)
}
