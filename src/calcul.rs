//! Calculs d'un devis. Tout se fait côté serveur, en nombres entiers (jamais de virgule flottante).
//!
//! Quantités et prix unitaires : entiers en 1/100 000 (« e5 »). Totaux : centimes.

const MAX_UNITES: i64 = 10_000_000;

/// Dernier arrondi : au plus proche, la moitié s'éloigne de zéro (0,5 → 1 ; −0,5 → −1).
fn diviser_arrondi(n: i128, d: i128) -> i64 {
    let moitie = d / 2;
    let q = if n >= 0 { (n + moitie) / d } else { -((-n + moitie) / d) };
    q as i64
}

/// Quantité × prix unitaire, en centimes.
pub fn total_ligne_cents(quantite_e5: i64, prix_unitaire_e5: i64) -> i64 {
    // (q / 1e5) × (p / 1e5) euros = q × p / 1e10 euros = q × p / 1e8 centimes
    diviser_arrondi(i128::from(quantite_e5) * i128::from(prix_unitaire_e5), 100_000_000)
}

/// TVA d'une base en centimes ; le taux est en centièmes de pour cent (600 = 6 %).
pub fn tva_cents(base_cents: i64, taux_centiemes: i32) -> i64 {
    diviser_arrondi(i128::from(base_cents) * i128::from(taux_centiemes), 10_000)
}

#[derive(Debug, PartialEq, Eq)]
pub struct TvaParTaux {
    pub taux_centiemes: i32,
    pub base_cents: i64,
    pub tva_cents: i64,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct Totaux {
    pub ht_cents: i64,
    pub tva: Vec<TvaParTaux>,
    pub ttc_cents: i64,
}

/// `lignes` : (total HT de la ligne en centimes, taux de TVA de la ligne).
/// La TVA se calcule une fois par taux, sur la somme des bases (et non ligne par ligne).
pub fn totaux(lignes: &[(i64, i32)]) -> Totaux {
    let mut par_taux: Vec<(i32, i64)> = Vec::new();
    for &(total, taux) in lignes {
        match par_taux.iter_mut().find(|(t, _)| *t == taux) {
            Some((_, base)) => *base += total,
            None => par_taux.push((taux, total)),
        }
    }
    par_taux.sort_by_key(|(t, _)| *t);
    let tva: Vec<TvaParTaux> = par_taux
        .into_iter()
        .map(|(taux_centiemes, base_cents)| TvaParTaux { taux_centiemes, base_cents, tva_cents: tva_cents(base_cents, taux_centiemes) })
        .collect();
    let ht_cents = tva.iter().map(|t| t.base_cents).sum();
    let ttc_cents = ht_cents + tva.iter().map(|t| t.tva_cents).sum::<i64>();
    Totaux { ht_cents, tva, ttc_cents }
}

/// « 2,5 », « 1 150.50 », « -3780,36 » → entier en 1/100 000. `None` si ce n'est pas un nombre
/// (ou plus de 5 décimales, ou un nombre démesuré).
pub fn lire_decimal_e5(texte: &str) -> Option<i64> {
    let t: String = texte.chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}').collect();
    let t = t.replace(',', ".");
    let (negatif, t) = match t.strip_prefix('-') {
        Some(reste) => (true, reste),
        None => (false, t.as_str()),
    };
    let (entier, decimales) = t.split_once('.').unwrap_or((t, ""));
    let chiffres = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if (entier.is_empty() && decimales.is_empty()) || !chiffres(entier) || !chiffres(decimales) || decimales.len() > 5 {
        return None;
    }
    let e: i64 = if entier.is_empty() { 0 } else { entier.parse().ok()? };
    if e > MAX_UNITES {
        return None;
    }
    let d: i64 = if decimales.is_empty() { 0 } else { format!("{decimales:0<5}").parse().ok()? };
    let valeur = e * 100_000 + d;
    Some(if negatif { -valeur } else { valeur })
}

/// 250000 → « 2,5 » ; 100000 → « 1 » ; −378036000 → « -3780,36 ».
pub fn ecrire_decimal_e5(valeur: i64) -> String {
    let signe = if valeur < 0 { "-" } else { "" };
    let absolu = valeur.unsigned_abs();
    let (entier, decimales) = (absolu / 100_000, absolu % 100_000);
    if decimales == 0 {
        return format!("{signe}{entier}");
    }
    let d = format!("{decimales:05}");
    format!("{signe}{entier},{}", d.trim_end_matches('0'))
}

/// 600 → « 6 % » ; 2100 → « 21 % » ; 550 → « 5,5 % ».
pub fn taux_en_texte(taux_centiemes: i32) -> String {
    let (entier, reste) = (taux_centiemes / 100, taux_centiemes % 100);
    match reste {
        0 => format!("{entier} %"),
        r if r % 10 == 0 => format!("{entier},{} %", r / 10),
        r => format!("{entier},{r:02} %"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_d_une_ligne() {
        // 16 h à 50 € = 800 €
        assert_eq!(total_ligne_cents(1_600_000, 5_000_000), 80_000);
        // 2,5 × 19,99 € = 49,975 € → 49,98 € (la moitié monte)
        assert_eq!(total_ligne_cents(250_000, 1_999_000), 4_998);
        // et pour un montant négatif (avenant en moins), la moitié s'éloigne de zéro aussi
        assert_eq!(total_ligne_cents(250_000, -1_999_000), -4_998);
        assert_eq!(total_ligne_cents(100_000, -378_036_000), -378_036);
        // 5 décimales : 0,33333 × 3 = 0,99999 → 1,00 €
        assert_eq!(total_ligne_cents(33_333, 300_000), 100);
        assert_eq!(total_ligne_cents(0, 5_000_000), 0);
    }

    #[test]
    fn tva_d_une_base() {
        // Exemple vérifié sur un vrai chantier : 6 603,77 € HT à 6 % → 396,23 € → 7 000,00 € TTC
        assert_eq!(tva_cents(660_377, 600), 39_623);
        assert_eq!(660_377 + tva_cents(660_377, 600), 700_000);
        // Facture finale : 2 969,30 € HT à 6 % → 178,16 €
        assert_eq!(tva_cents(296_930, 600), 17_816);
        assert_eq!(tva_cents(100_000, 2_100), 21_000);
        assert_eq!(tva_cents(100_000, 0), 0);
        assert_eq!(tva_cents(-100_000, 600), -6_000);
    }

    #[test]
    fn totaux_regroupes_par_taux() {
        let t = totaux(&[(10_000, 600), (20_000, 2_100), (5_000, 600)]);
        assert_eq!(t.ht_cents, 35_000);
        assert_eq!(
            t.tva,
            vec![
                TvaParTaux { taux_centiemes: 600, base_cents: 15_000, tva_cents: 900 },
                TvaParTaux { taux_centiemes: 2_100, base_cents: 20_000, tva_cents: 4_200 },
            ]
        );
        assert_eq!(t.ttc_cents, 40_100);
        assert_eq!(totaux(&[]), Totaux::default());
    }

    #[test]
    fn la_tva_se_calcule_sur_la_base_du_taux_pas_ligne_par_ligne() {
        // Trois lignes de 0,25 € à 6 % : ligne par ligne chacune donnerait 0,02 (total 0,06) ;
        // sur la base de 0,75 € on obtient 0,05 €.
        let t = totaux(&[(25, 600), (25, 600), (25, 600)]);
        assert_eq!(t.tva[0].tva_cents, 5);
        assert_eq!(t.ttc_cents, 80);
    }

    #[test]
    fn nombres_lus_depuis_le_formulaire() {
        assert_eq!(lire_decimal_e5("2,5"), Some(250_000));
        assert_eq!(lire_decimal_e5("2.5"), Some(250_000));
        assert_eq!(lire_decimal_e5("1 150,50"), Some(115_050_000));
        assert_eq!(lire_decimal_e5("1\u{a0}150"), Some(115_000_000));
        assert_eq!(lire_decimal_e5("-3780,36"), Some(-378_036_000));
        assert_eq!(lire_decimal_e5("0,00001"), Some(1));
        assert_eq!(lire_decimal_e5(",5"), Some(50_000));
        assert_eq!(lire_decimal_e5("5,"), Some(500_000));
        assert_eq!(lire_decimal_e5("0"), Some(0));
        assert_eq!(lire_decimal_e5("0,000001"), None);
        assert_eq!(lire_decimal_e5(""), None);
        assert_eq!(lire_decimal_e5("-"), None);
        assert_eq!(lire_decimal_e5("abc"), None);
        assert_eq!(lire_decimal_e5("1,2,3"), None);
        assert_eq!(lire_decimal_e5("+5"), None);
        assert_eq!(lire_decimal_e5("--5"), None);
        assert_eq!(lire_decimal_e5("99999999999999999999"), None);
        assert_eq!(lire_decimal_e5("10000001"), None);
        assert_eq!(lire_decimal_e5("10000000"), Some(1_000_000_000_000));
    }

    #[test]
    fn nombres_ecrits_pour_le_formulaire() {
        assert_eq!(ecrire_decimal_e5(250_000), "2,5");
        assert_eq!(ecrire_decimal_e5(100_000), "1");
        assert_eq!(ecrire_decimal_e5(0), "0");
        assert_eq!(ecrire_decimal_e5(1), "0,00001");
        assert_eq!(ecrire_decimal_e5(-378_036_000), "-3780,36");
        assert_eq!(ecrire_decimal_e5(1_999_000), "19,99");
        for v in [0, 1, 250_000, -378_036_000, 1_999_000, 12_345_678] {
            assert_eq!(lire_decimal_e5(&ecrire_decimal_e5(v)), Some(v));
        }
    }

    #[test]
    fn taux_ecrits_en_texte() {
        assert_eq!(taux_en_texte(600), "6 %");
        assert_eq!(taux_en_texte(2_100), "21 %");
        assert_eq!(taux_en_texte(0), "0 %");
        assert_eq!(taux_en_texte(550), "5,5 %");
        assert_eq!(taux_en_texte(205), "2,05 %");
    }
}
