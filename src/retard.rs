//! Règles légales belges du retard de paiement, pour les rappels envoyés au nom de l'artisan.
//!
//! - **Particuliers** (livre XIX du Code de droit économique, loi du 4 mai 2023) : le premier rappel est gratuit, sur
//!   support durable, avec quatre mentions (montant dû et montant de l'indemnité ; nom et numéro d'entreprise du
//!   créancier ; description et date d'exigibilité ; délai de 14 jours). Le délai de 14 jours calendrier court à partir
//!   du lendemain de l'envoi d'un e-mail. L'indemnité n'existe que si les conditions générales la prévoient, selon un
//!   barème plafonné ; les intérêts sont plafonnés au taux de la loi du 2 août 2002.
//! - **Entreprises** (loi du 2 août 2002) : intérêts de plein droit au taux semestriel, plus une indemnité forfaitaire de 40 €.
//!
//! Ces règles viennent d'une recherche du 19/10/2026 (sources en partie secondaires) : à faire valider par un avocat
//! avant la mise en service (voir `docs/conformite.md`). Le calendrier des jours ouvrables du courrier postal (3e jour
//! ouvrable, samedi compris) n'est pas géré : l'application n'envoie que des e-mails.

use chrono::{Duration, NaiveDate};
use sqlx::PgPool;

use crate::commun::format_euros;
use crate::factures::{self, Facture};

/// Le délai laissé au consommateur, en jours calendrier.
pub const DELAI_JOURS: i64 = 14;
/// Indemnité forfaitaire entre entreprises (loi du 2 août 2002).
pub const INDEMNITE_ENTREPRISE_CENTS: i64 = 4_000;
/// Plafond de l'indemnité envers un consommateur (art. XIX.4 du Code de droit économique).
const PLAFOND_INDEMNITE_CENTS: i64 = 200_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Regime {
    Particulier,
    Entreprise,
}

/// Taux d'intérêt de retard des transactions commerciales (loi du 2 août 2002, art. 5), en centièmes de pour cent par an,
/// avec le premier jour où il s'applique. 1er semestre 2026 : fiche officielle du SPF Finances. 2e semestre 2026 : calculé
/// (taux BCE de 2,40 % + 8 points, arrondi au demi-point supérieur = 10,50 %) ; l'avis du Moniteur n'a pas été retrouvé.
/// À compléter à chaque semestre ; au-delà de la dernière période connue, le taux n'est plus affiché.
const TAUX_COMMERCIAL: [((i32, u32, u32), i32); 2] = [((2026, 1, 1), 1050), ((2026, 7, 1), 1050)];
const FIN_DES_TAUX: (i32, u32, u32) = (2027, 1, 1);

fn jour(d: (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.0, d.1, d.2).expect("date de la table des taux")
}

/// Le taux d'intérêt de retard commercial en vigueur ce jour-là, s'il est connu.
pub fn taux_commercial_centiemes(j: NaiveDate) -> Option<i32> {
    if j >= jour(FIN_DES_TAUX) {
        return None;
    }
    TAUX_COMMERCIAL.iter().rev().find(|(debut, _)| j >= jour(*debut)).map(|(_, taux)| *taux)
}

/// L'indemnité de retard maximale envers un consommateur pour un montant dû (art. XIX.4) : 20 € jusqu'à 150 € ;
/// 30 € + 10 % de la tranche de 150,01 à 500 € ; 65 € + 5 % de la tranche au-delà de 500 € ; 2 000 € au plus.
pub fn indemnite_particulier_cents(du_cents: i64) -> i64 {
    if du_cents <= 0 {
        0
    } else if du_cents <= 15_000 {
        2_000
    } else if du_cents <= 50_000 {
        3_000 + (du_cents - 15_000 + 5) / 10
    } else {
        (6_500 + (du_cents - 50_000 + 10) / 20).min(PLAFOND_INDEMNITE_CENTS)
    }
}

/// Le premier jour du délai : le lendemain de l'envoi de l'e-mail.
pub fn debut_du_delai(envoi: NaiveDate) -> NaiveDate {
    envoi + Duration::days(1)
}

/// Le dernier jour du délai (inclus) : 14 jours calendrier à partir du lendemain de l'envoi.
pub fn fin_du_delai(envoi: NaiveDate) -> NaiveDate {
    envoi + Duration::days(DELAI_JOURS)
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// 1050 → « 10,50 ».
fn pourcent(centiemes: i32) -> String {
    format!("{},{:02}", centiemes / 100, centiemes % 100)
}

pub(crate) struct Reglage {
    pub indemnite_particuliers: bool,
    pub interets_particuliers_centiemes: Option<i32>,
    pub pme: bool,
}

pub(crate) async fn reglage(db: &PgPool) -> Result<Reglage, sqlx::Error> {
    let (indemnite, interets, pme): (bool, Option<i32>, bool) =
        sqlx::query_as("SELECT rappel_indemnite_particuliers, rappel_interets_particuliers, rappel_pme FROM reglages WHERE id = 1").fetch_one(db).await?;
    Ok(Reglage { indemnite_particuliers: indemnite, interets_particuliers_centiemes: interets, pme })
}

/// Ce que le bloc légal d'un rappel dit de la facture et de son créancier.
pub(crate) struct Contexte {
    pub regime: Regime,
    pub societe_nom: String,
    pub societe_numero: String,
    pub societe_email: String,
    pub societe_telephone: String,
    pub numero_facture: String,
    pub date_facture: NaiveDate,
    pub echeance: Option<NaiveDate>,
    /// Ce que la facture facture (sa référence, sinon « travaux »).
    pub objet: String,
    pub reste_cents: i64,
    /// C'est le premier rappel de cette facture.
    pub premier: bool,
}

pub(crate) async fn contexte(db: &PgPool, f: &Facture) -> Result<Contexte, sqlx::Error> {
    // Un client qui a un numéro de TVA est une entreprise ; sans numéro, un particulier.
    let tva_client: Option<String> = match (f.client_entreprise_id, f.client_contact_id) {
        (Some(e), _) => sqlx::query_scalar("SELECT numero_tva FROM entreprises WHERE id = $1").bind(e).fetch_one(db).await?,
        (_, Some(c)) => sqlx::query_scalar("SELECT numero_tva FROM contacts WHERE id = $1").bind(c).fetch_one(db).await?,
        _ => None,
    };
    let regime = if tva_client.is_some_and(|t| !t.trim().is_empty()) { Regime::Entreprise } else { Regime::Particulier };
    let societe: Option<(String, Option<String>, String, String)> =
        sqlx::query_as("SELECT nom, numero_tva, email, telephone FROM societe WHERE id = 1").fetch_optional(db).await?;
    let (societe_nom, societe_numero, societe_email, societe_telephone) = societe.map(|(n, t, e, p)| (n, t.unwrap_or_default(), e, p)).unwrap_or_default();
    let (reste_cents, deja): (i64, i64) = sqlx::query_as(
        "SELECT GREATEST(et.reste_cents, 0), (SELECT count(*) FROM emails m WHERE m.facture_id = f.id
                AND m.modele IN ('rappel_doux', 'rappel_ferme') AND m.statut <> 'echec')
         FROM factures f JOIN factures_etats et ON et.facture_id = f.id WHERE f.id = $1",
    )
    .bind(f.id)
    .fetch_one(db)
    .await?;
    Ok(Contexte {
        regime,
        societe_nom,
        societe_numero,
        societe_email,
        societe_telephone,
        numero_facture: f.numero.map(|n| factures::numero_affiche(f.date_facture, n)).unwrap_or_default(),
        date_facture: f.date_facture,
        echeance: f.date_echeance,
        objet: if f.reference.trim().is_empty() { "travaux".to_string() } else { f.reference.trim().to_string() },
        reste_cents,
        premier: deja == 0,
    })
}

/// Ce qui manque à la société pour que le rappel soit légal (liste vide : on peut envoyer).
pub(crate) fn manques(c: &Contexte) -> Vec<String> {
    let mut m = Vec::new();
    if c.societe_nom.trim().is_empty() {
        m.push("Le nom de la société (Réglages › Société)".to_string());
    }
    if c.societe_numero.trim().is_empty() {
        m.push("Le numéro d'entreprise (TVA) de la société (Réglages › Société) : mention obligatoire d'un rappel".to_string());
    }
    if c.societe_email.trim().is_empty() && c.societe_telephone.trim().is_empty() {
        m.push("Un e-mail ou un téléphone de la société (Réglages › Société), pour contester la facture ou demander les pièces".to_string());
    }
    m
}

/// Le bloc légal ajouté à la fin du rappel : figé, jamais dans la zone de texte modifiable. `envoi` est le jour d'envoi.
pub(crate) fn bloc_legal(c: &Contexte, r: &Reglage, envoi: NaiveDate) -> String {
    let mut l: Vec<String> = Vec::new();
    let echeance = c.echeance.map(date_belge).unwrap_or_else(|| date_belge(c.date_facture));
    let creancier = format!("Créancier : {}, numéro d'entreprise {}.", c.societe_nom.trim(), c.societe_numero.trim());
    let facture = format!("Facture : n° {} du {}, relative à {}, exigible le {}.", c.numero_facture, date_belge(c.date_facture), c.objet, echeance);
    let du = format!("Montant restant dû : {} TVA comprise.", format_euros(c.reste_cents));
    match c.regime {
        Regime::Particulier => {
            l.push("Informations légales sur ce rappel (livre XIX du Code de droit économique)".to_string());
            l.push(creancier);
            l.push(facture);
            l.push(du);
            let delai = format!(
                "Vous disposez de 14 jours calendrier à compter du lendemain de l'envoi de cet e-mail, soit jusqu'au {} inclus, pour payer ce montant.",
                date_belge(fin_du_delai(envoi))
            );
            l.push(format!("{} {delai}", if c.premier { "Ce premier rappel est gratuit." } else { "Ce rappel est gratuit." }));
            let indemnite = if r.indemnite_particuliers { indemnite_particulier_cents(c.reste_cents) } else { 0 };
            l.push(if indemnite > 0 {
                format!("Montant de l'indemnité de retard prévue par nos conditions générales : {}.", format_euros(indemnite))
            } else {
                "Montant de l'indemnité de retard : aucune.".to_string()
            });
            if let Some(taux) = r.interets_particuliers_centiemes.filter(|t| *t > 0) {
                // Les intérêts ne dépassent jamais le taux de la loi du 2 août 2002.
                let taux = taux_commercial_centiemes(envoi).map(|plafond| taux.min(plafond)).unwrap_or(taux);
                let depuis = if r.pme { debut_du_delai(envoi) } else { fin_du_delai(envoi) + Duration::days(1) };
                l.push(format!(
                    "À défaut de paiement dans ce délai, des intérêts de retard au taux de {} % l'an pourront vous être réclamés à compter du {}, conformément à nos conditions générales.",
                    pourcent(taux),
                    date_belge(depuis)
                ));
            } else if indemnite > 0 {
                l.push("À défaut de paiement dans ce délai, l'indemnité ci-dessus pourra vous être réclamée, conformément à nos conditions générales.".to_string());
            }
        }
        Regime::Entreprise => {
            l.push("Informations sur ce rappel (transaction commerciale)".to_string());
            l.push(creancier);
            l.push(facture);
            l.push(du);
            let taux = match taux_commercial_centiemes(envoi) {
                Some(t) => format!("au taux de {} % l'an (taux légal en vigueur)", pourcent(t)),
                None => "au taux légal en vigueur".to_string(),
            };
            l.push(format!(
                "Conformément à la loi du 2 août 2002 concernant la lutte contre le retard de paiement dans les transactions commerciales, des intérêts de retard {taux} et une indemnité forfaitaire de {} pour frais de recouvrement sont dus de plein droit à défaut de paiement, sans préjudice d'une indemnisation raisonnable des autres frais de recouvrement.",
                format_euros(INDEMNITE_ENTREPRISE_CENTS)
            ));
        }
    }
    let mut contact = Vec::new();
    if !c.societe_email.trim().is_empty() {
        contact.push(c.societe_email.trim().to_string());
    }
    if !c.societe_telephone.trim().is_empty() {
        contact.push(c.societe_telephone.trim().to_string());
    }
    l.push(format!("Pour contester cette facture ou recevoir les pièces justificatives, contactez-nous : {}.", contact.join(" · ")));
    l.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    #[test]
    fn le_bareme_de_l_indemnite() {
        assert_eq!(indemnite_particulier_cents(0), 0);
        assert_eq!(indemnite_particulier_cents(10_000), 2_000, "100 € → 20 €");
        assert_eq!(indemnite_particulier_cents(15_000), 2_000, "150 € → 20 €");
        assert_eq!(indemnite_particulier_cents(15_001), 3_000, "150,01 € → 30 €");
        assert_eq!(indemnite_particulier_cents(30_000), 4_500, "300 € → 30 € + 10 % de 150 €");
        assert_eq!(indemnite_particulier_cents(50_000), 6_500, "500 € → 30 € + 10 % de 350 € = 65 €");
        assert_eq!(indemnite_particulier_cents(50_100), 6_505, "500,01 € et plus → 65 € + 5 % de la tranche");
        assert_eq!(indemnite_particulier_cents(100_000), 9_000, "1 000 € → 65 € + 5 % de 500 €");
        assert_eq!(indemnite_particulier_cents(5_000_000), 200_000, "plafonnée à 2 000 €");
    }

    #[test]
    fn le_delai_de_14_jours_court_des_le_lendemain() {
        assert_eq!(debut_du_delai(d(2026, 10, 19)), d(2026, 10, 20));
        assert_eq!(fin_du_delai(d(2026, 10, 19)), d(2026, 11, 2));
        assert_eq!(fin_du_delai(d(2026, 12, 25)), d(2027, 1, 8), "à cheval sur deux années");
    }

    #[test]
    fn le_taux_commercial_par_semestre() {
        assert_eq!(taux_commercial_centiemes(d(2026, 3, 1)), Some(1050));
        assert_eq!(taux_commercial_centiemes(d(2026, 10, 19)), Some(1050));
        assert_eq!(taux_commercial_centiemes(d(2025, 12, 31)), None, "avant la table");
        assert_eq!(taux_commercial_centiemes(d(2027, 1, 1)), None, "après la table : à compléter");
        assert_eq!(pourcent(1050), "10,50");
        assert_eq!(pourcent(450), "4,50");
    }

    fn contexte_de_test(regime: Regime) -> Contexte {
        Contexte {
            regime,
            societe_nom: "Ubique SRL".into(),
            societe_numero: "BE0726407561".into(),
            societe_email: "info@ubique.be".into(),
            societe_telephone: "010 00 00 00".into(),
            numero_facture: "2026-125".into(),
            date_facture: d(2026, 9, 1),
            echeance: Some(d(2026, 9, 11)),
            objet: "Toiture".into(),
            reste_cents: 30_000,
            premier: true,
        }
    }

    #[test]
    fn le_rappel_au_particulier_porte_les_quatre_mentions_et_rien_de_plus_par_defaut() {
        let r = Reglage { indemnite_particuliers: false, interets_particuliers_centiemes: None, pme: false };
        let b = bloc_legal(&contexte_de_test(Regime::Particulier), &r, d(2026, 10, 19));
        for attendu in [
            "Montant restant dû : 300,00\u{a0}€ TVA comprise.",
            "Montant de l'indemnité de retard : aucune.",
            "Créancier : Ubique SRL, numéro d'entreprise BE0726407561.",
            "Facture : n° 2026-125 du 01/09/2026, relative à Toiture, exigible le 11/09/2026.",
            "Ce premier rappel est gratuit.",
            "14 jours calendrier à compter du lendemain de l'envoi de cet e-mail, soit jusqu'au 02/11/2026 inclus",
            "contactez-nous : info@ubique.be · 010 00 00 00",
        ] {
            assert!(b.contains(attendu), "manque « {attendu} » dans :\n{b}");
        }
        assert!(!b.contains("intérêts"), "aucun intérêt réclamé par défaut");
    }

    #[test]
    fn l_indemnite_et_les_interets_ne_figurent_que_si_les_conditions_generales_les_prevoient() {
        let r = Reglage { indemnite_particuliers: true, interets_particuliers_centiemes: Some(800), pme: false };
        let b = bloc_legal(&contexte_de_test(Regime::Particulier), &r, d(2026, 10, 19));
        assert!(b.contains("Montant de l'indemnité de retard prévue par nos conditions générales : 45,00\u{a0}€."), "{b}");
        // Pas une PME : les intérêts courent après l'expiration du délai (le 03/11/2026).
        assert!(b.contains("au taux de 8,00 % l'an") && b.contains("à compter du 03/11/2026"), "{b}");
        // Une PME : les intérêts courent dès le début du délai (le lendemain de l'envoi).
        let pme = Reglage { pme: true, ..r };
        let b = bloc_legal(&contexte_de_test(Regime::Particulier), &pme, d(2026, 10, 19));
        assert!(b.contains("à compter du 20/10/2026"), "{b}");
    }

    #[test]
    fn les_interets_envers_un_particulier_ne_depassent_pas_le_taux_legal() {
        let r = Reglage { indemnite_particuliers: false, interets_particuliers_centiemes: Some(2500), pme: false };
        let b = bloc_legal(&contexte_de_test(Regime::Particulier), &r, d(2026, 10, 19));
        assert!(b.contains("au taux de 10,50 % l'an"), "plafonné à 10,50 % : {b}");
    }

    #[test]
    fn le_rappel_a_une_entreprise_cite_la_loi_de_2002() {
        let r = Reglage { indemnite_particuliers: true, interets_particuliers_centiemes: Some(800), pme: true };
        let b = bloc_legal(&contexte_de_test(Regime::Entreprise), &r, d(2026, 10, 19));
        assert!(b.contains("loi du 2 août 2002") && b.contains("10,50 % l'an") && b.contains("40,00\u{a0}€"), "{b}");
        assert!(!b.contains("livre XIX") && !b.contains("gratuit"), "pas de règles de consommation : {b}");
        let b = bloc_legal(&contexte_de_test(Regime::Entreprise), &r, d(2027, 2, 1));
        assert!(b.contains("au taux légal en vigueur") && !b.contains("10,50"), "taux inconnu : pas de chiffre inventé : {b}");
    }

    #[test]
    fn ce_qui_manque_a_la_societe() {
        let mut c = contexte_de_test(Regime::Particulier);
        assert!(manques(&c).is_empty());
        c.societe_numero = " ".into();
        c.societe_email = String::new();
        c.societe_telephone = String::new();
        assert_eq!(manques(&c).len(), 2);
    }
}
