//! Mentions légales de TVA imprimées sur les factures et les notes de crédit.
//!
//! Les textes sont figés (jamais saisis ni modifiables par l'utilisateur) : ils viennent de la loi, pas de l'artisan.
//! Source (recherche du 19/10/2026, à faire confirmer par le comptable) : réponse du ministre des Finances à la
//! question écrite n° 8-1 du Sénat (30/09/2024) pour le taux de 6 %, AR n° 1, article 20, § 3 pour l'autoliquidation.
//! Les deux textes doivent être copiés mot pour mot depuis Fisconetplus avant la mise en service (typographie
//! « 21 % » ou « 21% » à vérifier).
//!
//! Sans la mention complète, le client n'est pas présumé reconnaître qu'il remplit les conditions : l'artisan reste alors
//! redevable de la TVA, des intérêts et des amendes si le client ne les remplissait pas.

use crate::factures::LigneBase;

/// Autoliquidation pour des travaux immobiliers fournis à un assujetti tenu au dépôt de déclarations périodiques.
pub const AUTOLIQUIDATION: &str = "Autoliquidation : En l'absence de contestation par écrit, dans un délai d'un mois à compter de la réception de la facture, le client est présumé reconnaître qu'il est un assujetti tenu au dépôt de déclarations périodiques. Si cette condition n'est pas remplie, le client endossera, par rapport à cette condition, la responsabilité quant au paiement de la taxe, des intérêts et des amendes dus.";

/// Taux réduit de 6 % pour la rénovation d'une habitation de plus de dix ans, facturée à un consommateur final.
/// Remplace l'attestation du client depuis le 1er juillet 2022.
pub const TAUX_6_RENOVATION: &str = "Taux de TVA: En l'absence de contestation par écrit, dans un délai d'un mois à compter de la réception de la facture, le client est présumé reconnaître que (1) les travaux sont effectués à un bâtiment d'habitation dont la première occupation a eu lieu au cours d'une année civile qui précède d'au moins dix ans la date de la première facture relative à ces travaux, (2) qu'après l'exécution de ces travaux, l'habitation est utilisée, soit exclusivement soit à titre principal comme logement privé et (3) que ces travaux sont fournis et facturés à un consommateur final. Si au moins une de ces conditions n'est pas remplie, le taux normal de TVA de 21% sera applicable et le client endossera, par rapport à ces conditions, la responsabilité quant au paiement de la taxe, des intérêts et des amendes dus.";

/// Une ligne chiffrée au taux d'autoliquidation.
fn autoliquidee(l: &LigneBase) -> bool {
    l.genre == "poste" && l.autoliquidation
}

/// Une ligne chiffrée à 6 % (hors autoliquidation).
fn a_six_pour_cent(l: &LigneBase) -> bool {
    l.genre == "poste" && !l.autoliquidation && l.taux_centiemes == Some(600)
}

/// L'autoliquidation ne se combine jamais avec le taux de 6 % : le 6 % exige un consommateur final, l'autoliquidation
/// un assujetti qui dépose des déclarations périodiques. Une facture qui mélange les deux n'est pas valable.
pub(crate) fn combine_autoliquidation_et_6(lignes: &[LigneBase]) -> bool {
    lignes.iter().any(autoliquidee) && lignes.iter().any(a_six_pour_cent)
}

/// Les mentions à imprimer pour ces lignes, dans l'ordre : autoliquidation, puis taux de 6 %.
/// Pour le 6 %, seule la mention de la rénovation d'une habitation de plus de dix ans est connue : les autres régimes
/// à 6 % (pompes à chaleur dans un logement de moins de dix ans, démolition-reconstruction) ont leur propre mention,
/// à ajouter après validation par le comptable.
pub(crate) fn a_imprimer(lignes: &[LigneBase]) -> Vec<&'static str> {
    let mut mentions = Vec::new();
    if lignes.iter().any(autoliquidee) {
        mentions.push(AUTOLIQUIDATION);
    }
    if lignes.iter().any(a_six_pour_cent) {
        mentions.push(TAUX_6_RENOVATION);
    }
    mentions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(genre: &str, taux: Option<i32>, autoliquidation: bool) -> LigneBase {
        LigneBase {
            id: 1,
            genre: genre.to_string(),
            description: String::new(),
            quantite_e5: None,
            unite: String::new(),
            prix_unitaire_e5: None,
            taux_tva_id: None,
            taux_centiemes: taux,
            total_cents: 0,
            autoliquidation,
        }
    }

    #[test]
    fn rien_a_imprimer_a_21_pour_cent() {
        assert!(a_imprimer(&[ligne("poste", Some(2100), false)]).is_empty());
    }

    #[test]
    fn le_six_pour_cent_imprime_sa_mention() {
        let m = a_imprimer(&[ligne("titre", None, false), ligne("poste", Some(600), false), ligne("poste", Some(600), false)]);
        assert_eq!(m, vec![TAUX_6_RENOVATION]);
    }

    #[test]
    fn l_autoliquidation_imprime_sa_mention_complete() {
        let m = a_imprimer(&[ligne("poste", Some(0), true)]);
        assert_eq!(m, vec![AUTOLIQUIDATION]);
        assert!(AUTOLIQUIDATION.starts_with("Autoliquidation : En l'absence de contestation par écrit"));
    }

    #[test]
    fn un_titre_ou_un_texte_n_imprime_rien() {
        assert!(a_imprimer(&[ligne("titre", Some(600), false), ligne("texte", Some(0), true)]).is_empty());
    }

    #[test]
    fn autoliquidation_et_six_pour_cent_ne_se_combinent_pas() {
        assert!(combine_autoliquidation_et_6(&[ligne("poste", Some(0), true), ligne("poste", Some(600), false)]));
        assert!(!combine_autoliquidation_et_6(&[ligne("poste", Some(0), true), ligne("poste", Some(2100), false)]));
        assert!(!combine_autoliquidation_et_6(&[ligne("poste", Some(600), false), ligne("poste", Some(2100), false)]));
    }
}
