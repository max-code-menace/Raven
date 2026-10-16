//! Calculs des factures, en entiers : communication structurée (VCS), date d'échéance,
//! et répartition d'un montant entre les taux de TVA (acompte, remise d'un devis).

use chrono::{Datelike, Days, NaiveDate};

/// Communication structurée belge d'une facture : les 10 chiffres du numéro, puis 2 chiffres de contrôle
/// (le reste de la division par 97 ; 97 quand ce reste vaut 0). 125 → « +++000/0000/12528+++ ».
pub fn vcs(numero: i64) -> String {
    let base = numero.rem_euclid(10_000_000_000);
    let controle = match base % 97 {
        0 => 97,
        r => r,
    };
    let chiffres = format!("{base:010}{controle:02}");
    format!("+++{}/{}/{}+++", &chiffres[0..3], &chiffres[3..7], &chiffres[7..12])
}

/// Vérifie une communication structurée (avec ou sans les +++ et les /).
pub fn vcs_valide(texte: &str) -> bool {
    let chiffres: String = texte.chars().filter(char::is_ascii_digit).collect();
    if chiffres.len() != 12 {
        return false;
    }
    let (base, controle) = chiffres.split_at(10);
    let (Ok(base), Ok(controle)) = (base.parse::<i64>(), controle.parse::<i64>()) else {
        return false;
    };
    let attendu = match base % 97 {
        0 => 97,
        r => r,
    };
    controle == attendu
}

/// La date d'échéance d'après les conditions de paiement :
/// - « X jours après date de facturation » : date de la facture + X jours ;
/// - « X jours fin du mois » : dernier jour du mois de la facture + X jours ;
/// - « comptant » : la date de la facture.
pub fn echeance(date_facture: NaiveDate, genre_condition: &str, jours: i32) -> NaiveDate {
    let jours = Days::new(u64::try_from(jours.max(0)).unwrap_or(0));
    match genre_condition {
        "apres_facturation" => date_facture + jours,
        "fin_de_mois" => fin_du_mois(date_facture) + jours,
        _ => date_facture,
    }
}

fn fin_du_mois(d: NaiveDate) -> NaiveDate {
    let (annee, mois) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
    NaiveDate::from_ymd_opt(annee, mois, 1).and_then(|premier| premier.pred_opt()).unwrap_or(d)
}

/// Répartit `total` entre des bases (une par taux de TVA) au prorata, sans perdre un centime.
/// Quand la somme des bases n'est pas positive (rien à quoi se rapporter), tout va à la première base.
pub fn repartir(total: i64, bases: &[i64]) -> Vec<i64> {
    let somme: i64 = bases.iter().sum();
    if bases.is_empty() {
        return Vec::new();
    }
    if somme <= 0 || bases.iter().any(|b| *b < 0) {
        let mut parts = vec![0; bases.len()];
        parts[0] = total;
        return parts;
    }
    if total >= 0 {
        crate::calcul::repartir(total, bases, somme)
    } else {
        crate::calcul::repartir(-total, bases, somme).into_iter().map(|p| -p).collect()
    }
}

/// Additionne des montants par clé (un taux de TVA), en gardant l'ordre d'arrivée des clés.
pub fn par_cle(montants: impl IntoIterator<Item = (i64, i64)>) -> Vec<(i64, i64)> {
    let mut sommes: Vec<(i64, i64)> = Vec::new();
    for (cle, montant) in montants {
        match sommes.iter_mut().find(|(c, _)| *c == cle) {
            Some((_, s)) => *s += montant,
            None => sommes.push((cle, montant)),
        }
    }
    sommes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(a: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, j).unwrap()
    }

    #[test]
    fn communication_structuree() {
        // 125 mod 97 = 28
        assert_eq!(vcs(125), "+++000/0000/12528+++");
        // un multiple de 97 prend 97 comme contrôle
        assert_eq!(vcs(97), "+++000/0000/09797+++");
        assert_eq!(vcs(1_234_567_890), "+++123/4567/89002+++");
        assert!(vcs_valide(&vcs(125)) && vcs_valide(&vcs(97)) && vcs_valide("123456789002"));
        assert!(!vcs_valide("+++000/0000/12527+++") && !vcs_valide("123") && !vcs_valide(""));
    }

    #[test]
    fn dates_d_echeance() {
        let d = date(2026, 10, 16);
        assert_eq!(echeance(d, "apres_facturation", 30), date(2026, 11, 15));
        assert_eq!(echeance(d, "fin_de_mois", 30), date(2026, 11, 30));
        assert_eq!(echeance(date(2026, 12, 5), "fin_de_mois", 10), date(2027, 1, 10));
        assert_eq!(echeance(date(2028, 2, 3), "fin_de_mois", 0), date(2028, 2, 29));
        assert_eq!(echeance(d, "comptant", 0), d);
    }

    #[test]
    fn repartition_sans_perte() {
        assert_eq!(repartir(100, &[1, 1, 1]).iter().sum::<i64>(), 100);
        assert_eq!(repartir(-100, &[1, 1, 1]).iter().sum::<i64>(), -100);
        assert_eq!(repartir(1_000, &[3_000, 1_000]), vec![750, 250]);
        assert_eq!(repartir(50, &[0, 0]), vec![50, 0]);
        assert_eq!(repartir(50, &[100, -20]), vec![50, 0]);
        assert!(repartir(5, &[]).is_empty());
    }

    #[test]
    fn sommes_par_taux() {
        assert_eq!(par_cle([(3, 10), (1, 5), (3, 1)]), vec![(3, 11), (1, 5)]);
    }
}
