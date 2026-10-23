//! Composants partagés du design de Raven (refonte, étape 2) : ce que reçoivent les macros de `templates/_ui.html`
//! (bouton, pastille de statut, champ, tableau, bandeau, onglets, tuile chiffrée, totaux).
//! Rien n'est calculé ici : les mots des statuts viennent des listes existantes (devis, paiements, chantiers), les montants
//! de `calcul::Totaux` (déjà arrondis au centime) et sont écrits par `commun::format_euros`.
//! La page `/composants` les montre tous, avec des données fictives.

use std::borrow::Borrow;

use askama::Template;
use axum::{Router, response::Html, routing::get};

use crate::{AppState, Erreur, calcul, commun::format_euros};

// ─── Pastilles de statut ────────────────────────────────────────────────────

/// Une pastille : le mot exact du statut et sa variante. Une règle pour tous les statuts (audit du design du 23/10/2026) :
/// jaune (`rv-pill--todo`) = à l'utilisateur de jouer, contour encre (neutre) = on attend quelqu'un d'autre,
/// vert (`--done`) = fini, rouge (`--late`, `--refused`) = problème, gris (`--draft`) = brouillon.
#[derive(Debug, PartialEq, Eq)]
pub struct Pastille {
    pub mot: String,
    pub classe: &'static str,
}

const BROUILLON: &str = "rv-pill--draft";
const A_FAIRE: &str = "rv-pill--todo";
const NEUTRE: &str = "";
const ACQUIS: &str = "rv-pill--done";
const A_CORRIGER: &str = "rv-pill--late";
const REFUSE: &str = "rv-pill--refused";

fn pastille(mot: &str, classe: &'static str) -> Pastille {
    // Le rouge est toujours doublé d'un « ! » devant le mot (design system, StatusPill).
    let mot = if classe == A_CORRIGER { format!("! {mot}") } else { mot.to_string() };
    Pastille { mot, classe }
}

/// Statut d'un devis (codes de `devis::STATUTS`).
pub fn pastille_devis(code: &str) -> Pastille {
    let classe = match code {
        "brouillon" => BROUILLON,
        "accepte" => ACQUIS,
        "refuse" => REFUSE,
        "echec_envoi" => A_CORRIGER,
        // À envoyer : c'est à l'utilisateur ; signé en ligne : il doit encore l'accepter.
        "a_envoyer" | "signe" => A_FAIRE,
        "envoi_en_cours" | "envoye" => NEUTRE,
        _ => BROUILLON,
    };
    pastille(crate::devis::statut(code).0, classe)
}

/// État d'une facture ou d'une note de crédit, tel que calculé par la vue `factures_etats` (codes de `paiements::ETATS`).
pub fn pastille_facture(code: &str) -> Pastille {
    let classe = match code {
        "brouillon" => BROUILLON,
        "payee" => ACQUIS,
        "en_retard" => A_CORRIGER,
        "creditee" => REFUSE,
        "a_envoyer" => A_FAIRE,
        "envoyee" | "partielle" => NEUTRE,
        _ => BROUILLON,
    };
    pastille(crate::paiements::etat(code).0, classe)
}

/// Statut d'un chantier ou d'un projet (codes de `chantiers::STATUTS`).
pub fn pastille_chantier(code: &str) -> Pastille {
    let classe = match code {
        "termine" => ACQUIS,
        "en_cours" => NEUTRE,
        _ => BROUILLON,
    };
    pastille(crate::chantiers::statut(code).0, classe)
}

// ─── Boutons ────────────────────────────────────────────────────────────────

/// « acc sm » → « rv-btn rv-btn--acc rv-btn--sm ». Genres : acc (l'action principale, une par écran), ink (geste qui
/// engage), ghost (discret), sm (36 px), icon (carré sans libellé). Un mot inconnu est ignoré.
pub fn classes_bouton(genre: impl AsRef<str>) -> String {
    let mut classes = String::from("rv-btn");
    for mot in genre.as_ref().split_whitespace() {
        if matches!(mot, "acc" | "ink" | "ghost" | "sm" | "icon") {
            classes.push_str(" rv-btn--");
            classes.push_str(mot);
        }
    }
    classes
}

// ─── Tableaux ───────────────────────────────────────────────────────────────

pub struct Colonne {
    pub titre: String,
    /// Colonne de nombres : alignée à droite, en mono.
    pub nombre: bool,
    /// Titre lu par les lecteurs d'écran seulement (colonne des boutons d'une ligne).
    pub cache: bool,
}

/// « N°|Client|Montant TTC:n|Statut|Actions:cache » → cinq colonnes, la troisième de nombres, la dernière sans titre visible.
pub fn colonnes(spec: impl AsRef<str>) -> Vec<Colonne> {
    spec.as_ref()
        .split('|')
        .map(|c| {
            if let Some(titre) = c.strip_suffix(":n") {
                Colonne { titre: titre.to_string(), nombre: true, cache: false }
            } else if let Some(titre) = c.strip_suffix(":cache") {
                Colonne { titre: titre.to_string(), nombre: false, cache: true }
            } else {
                Colonne { titre: c.to_string(), nombre: false, cache: false }
            }
        })
        .collect()
}

/// Un montant en centimes écrit à la belge (« 4 431,84 € »), qu'on le donne par valeur ou par référence.
pub fn euros(centimes: impl Borrow<i64>) -> String {
    format_euros(*centimes.borrow())
}

// ─── Totaux ─────────────────────────────────────────────────────────────────

pub struct LigneTotal {
    pub intitule: String,
    pub montant: String,
    /// Ligne de détail (base et TVA d'un taux, sous-total, ajustement) : en retrait.
    pub detail: bool,
}

/// Le récapitulatif d'un devis ou d'une facture, dans l'ordre du design system : (sous-total et ajustement s'il y en a un),
/// Total HT, base et TVA de chaque taux, puis Total TTC.
pub struct Recap {
    pub lignes: Vec<LigneTotal>,
    pub ttc: String,
}

/// Met en forme des totaux déjà calculés par `calcul::totaux` ; aucun montant n'est recalculé.
pub fn recap(t: &calcul::Totaux, libelle_ajustement: &str) -> Recap {
    let ligne = |intitule: String, cents: i64, detail: bool| LigneTotal { intitule, montant: format_euros(cents), detail };
    let mut lignes = Vec::new();
    if t.ajustement_cents != 0 {
        lignes.push(ligne("Sous-total HT".into(), t.sous_total_cents, true));
        let libelle = if libelle_ajustement.trim().is_empty() { "Ajustement" } else { libelle_ajustement.trim() };
        lignes.push(ligne(libelle.into(), t.ajustement_cents, true));
    }
    lignes.push(ligne("Total HT".into(), t.ht_cents, false));
    for taux in &t.tva {
        let texte = calcul::taux_en_texte(taux.taux_centiemes);
        lignes.push(ligne(format!("Base {texte}"), taux.base_cents, true));
        lignes.push(ligne(format!("TVA {texte}"), taux.tva_cents, true));
    }
    Recap { lignes, ttc: format_euros(t.ttc_cents) }
}

// ─── Enregistrement automatique ─────────────────────────────────────────────

/// Ce que dit la barre du bas d'un formulaire qui s'enregistre tout seul (macro `ui::enregistrement`) : à l'ouverture,
/// que tout s'enregistre automatiquement ; ensuite l'heure du dernier enregistrement, ou les valeurs refusées.
#[derive(Debug, Default)]
pub struct Enregistrement {
    /// « 14:32 » (heure de Bruxelles) ; vide tant que rien n'a été enregistré depuis l'ouverture de la page.
    pub heure: String,
    /// Les valeurs refusées : leur champ garde sa valeur précédente, les autres champs sont enregistrés.
    pub erreurs: Vec<String>,
}

impl Enregistrement {
    /// Juste après un enregistrement : l'heure de Bruxelles et ce qui a été refusé.
    pub async fn maintenant(db: &mut sqlx::PgConnection, erreurs: Vec<String>) -> Result<Self, sqlx::Error> {
        let heure: String =
            sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'Europe/Brussels', 'HH24:MI')").fetch_one(&mut *db).await?;
        Ok(Enregistrement { heure, erreurs })
    }
}

/// La demande vient de l'enregistrement automatique (htmx), pas d'un formulaire envoyé en entier.
pub fn par_htmx(entetes: &axum::http::HeaderMap) -> bool {
    entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request")
}

/// La réponse à l'enregistrement automatique d'un petit formulaire dans un bloc (une page en a plusieurs : Réglages ›
/// E-mails) : sa note seule, que htmx met à la place de l'ancienne (`id`).
#[derive(Template)]
#[template(path = "_enregistrement.html")]
pub struct NoteSeule {
    pub id: String,
    pub enregistrement: Enregistrement,
}

impl NoteSeule {
    pub async fn rendre(db: &mut sqlx::PgConnection, id: String, erreurs: Vec<String>) -> Result<Html<String>, Erreur> {
        let enregistrement = Enregistrement::maintenant(db, erreurs).await?;
        Ok(Html(NoteSeule { id, enregistrement }.render()?))
    }
}

// ─── Page de démonstration ──────────────────────────────────────────────────

pub struct ExempleFacture {
    pub numero: &'static str,
    pub client: &'static str,
    pub libelle: &'static str,
    pub echeance: &'static str,
    pub ttc_cents: i64,
    pub etat: &'static str,
}

#[derive(Template)]
#[template(path = "composants.html")]
struct Demo {
    devis: Vec<Pastille>,
    factures: Vec<Pastille>,
    chantiers: Vec<Pastille>,
    exemples: Vec<ExempleFacture>,
    recap: Recap,
    recap_remise: Recap,
    choix_tva: Vec<crate::commun::Choix>,
}

async fn demo() -> Result<Html<String>, Erreur> {
    fn codes(liste: &[(&'static str, &'static str, &'static str)]) -> Vec<&'static str> {
        liste.iter().map(|(c, _, _)| *c).collect()
    }
    // Des lignes fictives : 6 % et 21 %, avec et sans remise, passées par le vrai calcul des totaux.
    let lignes = [(1_240_000, 600), (183_500, 2100)];
    let page = Demo {
        devis: codes(&crate::devis::STATUTS).into_iter().map(pastille_devis).collect(),
        factures: codes(&crate::paiements::ETATS).into_iter().map(pastille_facture).collect(),
        chantiers: codes(&crate::chantiers::STATUTS).into_iter().map(pastille_chantier).collect(),
        exemples: vec![
            ExempleFacture { numero: "2026-45", client: "Claes Immo SA", libelle: "Entretien espaces verts", echeance: "30/10/2026", ttc_cents: 300_000, etat: "envoyee" },
            ExempleFacture { numero: "2026-40", client: "Van Damme Construct SRL", libelle: "Rénovation toiture, solde", echeance: "27/09/2026", ttc_cents: 736_475, etat: "en_retard" },
            ExempleFacture { numero: "2026-37", client: "Maes Jean-Pierre", libelle: "Cuisine, acompte", echeance: "12/07/2026", ttc_cents: 254_400, etat: "payee" },
        ],
        recap: recap(&calcul::totaux(&lignes, calcul::Ajustement::default()), ""),
        recap_remise: recap(&calcul::totaux(&lignes, calcul::Ajustement::depuis_base("montant", -50_000)), "Remise fidélité"),
        choix_tva: crate::commun::choix_fixes(&[("600", "6 %"), ("1200", "12 %"), ("2100", "21 %")], &["2100".to_string()]),
    };
    Ok(Html(page.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/composants", get(demo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaque_statut_existant_a_sa_pastille_avec_le_mot_du_code() {
        for (code, mot, _) in crate::devis::STATUTS {
            assert!(pastille_devis(code).mot.ends_with(mot), "{code}");
        }
        for (code, mot, _) in crate::paiements::ETATS {
            assert!(pastille_facture(code).mot.ends_with(mot), "{code}");
        }
        for (code, mot, _) in crate::chantiers::STATUTS {
            assert!(pastille_chantier(code).mot.ends_with(mot), "{code}");
        }
        assert_eq!(pastille_facture("en_retard"), Pastille { mot: "! En retard".into(), classe: "rv-pill--late" });
        assert_eq!(pastille_facture("payee").classe, "rv-pill--done");
        assert_eq!(pastille_devis("brouillon").classe, "rv-pill--draft");
        assert_eq!(pastille_devis("refuse").classe, "rv-pill--refused");
        // À envoyer (à faire) et Envoyé (on attend le client) ne se ressemblent plus.
        assert_eq!(pastille_devis("a_envoyer").classe, "rv-pill--todo");
        assert_eq!(pastille_devis("envoye").classe, "");
        assert_eq!(pastille_facture("a_envoyer").classe, "rv-pill--todo");
        assert_eq!(pastille_devis("envoye").classe, "");
        assert_eq!(pastille_devis("inconnu").mot, "Inconnu");
    }

    #[test]
    fn classes_des_boutons() {
        assert_eq!(classes_bouton(""), "rv-btn");
        assert_eq!(classes_bouton("acc sm"), "rv-btn rv-btn--acc rv-btn--sm");
        assert_eq!(classes_bouton("ink n'importe"), "rv-btn rv-btn--ink");
    }

    #[test]
    fn colonnes_et_montants() {
        let c = colonnes("N°|Montant TTC:n|Actions:cache");
        assert_eq!((c[0].titre.as_str(), c[0].nombre, c[1].titre.as_str(), c[1].nombre), ("N°", false, "Montant TTC", true));
        assert!(c[2].cache && c[2].titre == "Actions");
        assert_eq!(euros(443_184), "4\u{a0}431,84\u{a0}€");
        assert_eq!(euros(&-7), "-0,07\u{a0}€");
    }

    #[test]
    fn le_recapitulatif_reprend_les_totaux_sans_rien_recalculer() {
        let t = calcul::totaux(&[(100_000, 600), (50_000, 2100)], calcul::Ajustement::default());
        let r = recap(&t, "");
        let intitules: Vec<&str> = r.lignes.iter().map(|l| l.intitule.as_str()).collect();
        assert_eq!(intitules, ["Total HT", "Base 6 %", "TVA 6 %", "Base 21 %", "TVA 21 %"]);
        assert_eq!(r.lignes[0].montant, format_euros(t.ht_cents));
        assert_eq!(r.ttc, format_euros(t.ttc_cents));
        let avec = recap(&calcul::totaux(&[(100_000, 600)], calcul::Ajustement::depuis_base("montant", -10_000)), "Remise");
        assert_eq!(avec.lignes[0].intitule, "Sous-total HT");
        assert_eq!(avec.lignes[1].intitule, "Remise");
    }
}
