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

/// Ajustement du devis : une remise (valeur négative) ou une majoration (valeur positive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ajustement {
    #[default]
    Aucun,
    /// En centièmes de pour cent : −500 = remise de 5 %.
    Pourcentage(i64),
    /// En centimes : −10 000 = remise de 100 €.
    Montant(i64),
}

impl Ajustement {
    /// Relit l'ajustement tel qu'il est enregistré dans le devis (mode « aucun », « pourcentage » ou « montant »).
    pub fn depuis_base(mode: &str, valeur: i64) -> Self {
        match mode {
            "pourcentage" => Ajustement::Pourcentage(valeur),
            "montant" => Ajustement::Montant(valeur),
            _ => Ajustement::Aucun,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TvaParTaux {
    pub taux_centiemes: i32,
    /// Base HT de ce taux, après ajustement.
    pub base_cents: i64,
    pub tva_cents: i64,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct Totaux {
    /// Somme des lignes, avant ajustement.
    pub sous_total_cents: i64,
    pub ajustement_cents: i64,
    /// Sous-total + ajustement.
    pub ht_cents: i64,
    pub tva: Vec<TvaParTaux>,
    pub ttc_cents: i64,
}

/// Répartit `total` entre les bases au prorata, sans perdre un centime : la somme des parts
/// vaut exactement `total`. Le reste va aux plus grandes fractions (à égalité, au premier).
/// `bases_somme` doit être la somme des bases et être strictement positive.
fn repartir(total: i64, bases: &[i64], bases_somme: i64) -> Vec<i64> {
    let (total, somme) = (i128::from(total), i128::from(bases_somme));
    let mut parts: Vec<i64> = Vec::with_capacity(bases.len());
    let mut restes: Vec<i128> = Vec::with_capacity(bases.len());
    for &b in bases {
        let produit = total * i128::from(b);
        parts.push(produit.div_euclid(somme) as i64);
        restes.push(produit.rem_euclid(somme));
    }
    let manque = total as i64 - parts.iter().sum::<i64>();
    let mut ordre: Vec<usize> = (0..bases.len()).collect();
    ordre.sort_by(|&a, &b| restes[b].cmp(&restes[a]).then(a.cmp(&b)));
    for &i in ordre.iter().take(manque.max(0) as usize) {
        parts[i] += 1;
    }
    parts
}

/// `lignes` : (total HT de la ligne en centimes, taux de TVA de la ligne).
///
/// Règles :
/// - l'ajustement (remise ou majoration) porte sur le sous-total HT ; il est réparti entre les taux
///   de TVA au prorata de leurs bases, pour que la TVA reste juste ;
/// - la TVA se calcule une fois par taux, sur la somme des bases (et non ligne par ligne) ;
/// - sans sous-total positif, il n'y a rien à ajuster : l'ajustement vaut 0.
pub fn totaux(lignes: &[(i64, i32)], ajustement: Ajustement) -> Totaux {
    let mut par_taux: Vec<(i32, i64)> = Vec::new();
    for &(total, taux) in lignes {
        match par_taux.iter_mut().find(|(t, _)| *t == taux) {
            Some((_, base)) => *base += total,
            None => par_taux.push((taux, total)),
        }
    }
    par_taux.sort_by_key(|(t, _)| *t);

    let sous_total_cents: i64 = par_taux.iter().map(|(_, b)| *b).sum();
    let ajustement_cents = if sous_total_cents <= 0 {
        0
    } else {
        match ajustement {
            Ajustement::Aucun => 0,
            Ajustement::Pourcentage(centiemes) => diviser_arrondi(i128::from(sous_total_cents) * i128::from(centiemes), 10_000),
            Ajustement::Montant(cents) => cents,
        }
    };
    let bases: Vec<i64> = par_taux.iter().map(|(_, b)| *b).collect();
    let parts = if ajustement_cents == 0 { vec![0; bases.len()] } else { repartir(ajustement_cents, &bases, sous_total_cents) };

    let tva: Vec<TvaParTaux> = par_taux
        .iter()
        .zip(parts)
        .map(|(&(taux_centiemes, base), part)| {
            let base_cents = base + part;
            TvaParTaux { taux_centiemes, base_cents, tva_cents: tva_cents(base_cents, taux_centiemes) }
        })
        .collect();
    let ht_cents = sous_total_cents + ajustement_cents;
    let ttc_cents = ht_cents + tva.iter().map(|t| t.tva_cents).sum::<i64>();
    Totaux { sous_total_cents, ajustement_cents, ht_cents, tva, ttc_cents }
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

/// Prix unitaire affiché : au moins 2 décimales, jusqu'à 5 si elles comptent.
/// 5000000 → « 50,00 € » ; 1999000 → « 19,99 € » ; 12345 → « 0,12345 € ».
pub fn prix_pour_affichage(valeur: i64) -> String {
    let texte = ecrire_decimal_e5(valeur);
    let decimales = texte.split_once(',').map_or(0, |(_, d)| d.len());
    let manque = 2usize.saturating_sub(decimales);
    let separateur = if decimales == 0 { "," } else { "" };
    format!("{texte}{separateur}{}\u{a0}€", "0".repeat(manque))
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

    const SANS: Ajustement = Ajustement::Aucun;

    #[test]
    fn totaux_regroupes_par_taux() {
        let t = totaux(&[(10_000, 600), (20_000, 2_100), (5_000, 600)], SANS);
        assert_eq!(t.sous_total_cents, 35_000);
        assert_eq!(t.ajustement_cents, 0);
        assert_eq!(t.ht_cents, 35_000);
        assert_eq!(
            t.tva,
            vec![
                TvaParTaux { taux_centiemes: 600, base_cents: 15_000, tva_cents: 900 },
                TvaParTaux { taux_centiemes: 2_100, base_cents: 20_000, tva_cents: 4_200 },
            ]
        );
        assert_eq!(t.ttc_cents, 40_100);
        assert_eq!(totaux(&[], SANS), Totaux::default());
    }

    #[test]
    fn remise_en_pourcentage() {
        // 1 000 € à 6 % − 10 % = 900 € ; TVA 54 € ; TTC 954 €
        let t = totaux(&[(100_000, 600)], Ajustement::Pourcentage(-1_000));
        assert_eq!((t.sous_total_cents, t.ajustement_cents, t.ht_cents), (100_000, -10_000, 90_000));
        assert_eq!(t.tva, vec![TvaParTaux { taux_centiemes: 600, base_cents: 90_000, tva_cents: 5_400 }]);
        assert_eq!(t.ttc_cents, 95_400);
        // majoration de 2,5 %
        let t = totaux(&[(100_000, 600)], Ajustement::Pourcentage(250));
        assert_eq!((t.ajustement_cents, t.ht_cents), (2_500, 102_500));
    }

    #[test]
    fn remise_en_montant_repartie_entre_les_taux() {
        // 600 € à 6 % et 400 € à 21 %, remise de 100 € : 60 € sur le 6 %, 40 € sur le 21 %
        let t = totaux(&[(60_000, 600), (40_000, 2_100)], Ajustement::Montant(-10_000));
        assert_eq!(
            t.tva,
            vec![
                TvaParTaux { taux_centiemes: 600, base_cents: 54_000, tva_cents: 3_240 },
                TvaParTaux { taux_centiemes: 2_100, base_cents: 36_000, tva_cents: 7_560 },
            ]
        );
        assert_eq!((t.ht_cents, t.ttc_cents), (90_000, 90_000 + 3_240 + 7_560));
    }

    #[test]
    fn la_repartition_ne_perd_jamais_un_centime() {
        for ajustement in [-1, 1, -100, 7, -1_001, 99_999, -99_999] {
            let t = totaux(&[(33_333, 600), (33_333, 2_100), (33_334, 0)], Ajustement::Montant(ajustement));
            assert_eq!(t.ajustement_cents, ajustement);
            assert_eq!(t.tva.iter().map(|v| v.base_cents).sum::<i64>(), 100_000 + ajustement, "{ajustement}");
            assert_eq!(t.ht_cents, 100_000 + ajustement);
        }
    }

    #[test]
    fn rien_a_ajuster_sans_sous_total_positif() {
        assert_eq!(totaux(&[], Ajustement::Montant(-500)).ajustement_cents, 0);
        assert_eq!(totaux(&[(0, 600)], Ajustement::Pourcentage(-500)).ajustement_cents, 0);
        // un devis entièrement en moins (avenant négatif) ne reçoit pas d'ajustement non plus
        assert_eq!(totaux(&[(-1_000, 600)], Ajustement::Montant(-500)).ajustement_cents, 0);
    }

    #[test]
    fn bases_de_signes_differents() {
        // 1 000 € à 21 % et −200 € à 6 % : sous-total 800 € ; remise de 80 € répartie au prorata
        // (−100 € sur le 21 %, +20 € sur le 6 %), ce qui laisse bien 720 € au total.
        let t = totaux(&[(100_000, 2_100), (-20_000, 600)], Ajustement::Montant(-8_000));
        assert_eq!(t.ht_cents, 72_000);
        assert_eq!(t.tva.iter().map(|v| v.base_cents).sum::<i64>(), 72_000);
    }

    #[test]
    fn la_tva_se_calcule_sur_la_base_du_taux_pas_ligne_par_ligne() {
        // Trois lignes de 0,25 € à 6 % : ligne par ligne chacune donnerait 0,02 (total 0,06) ;
        // sur la base de 0,75 € on obtient 0,05 €.
        let t = totaux(&[(25, 600), (25, 600), (25, 600)], SANS);
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
    fn prix_affiches_avec_deux_decimales_au_moins() {
        assert_eq!(prix_pour_affichage(5_000_000), "50,00\u{a0}€");
        assert_eq!(prix_pour_affichage(1_999_000), "19,99\u{a0}€");
        assert_eq!(prix_pour_affichage(1_950_000), "19,50\u{a0}€");
        assert_eq!(prix_pour_affichage(12_345), "0,12345\u{a0}€");
        assert_eq!(prix_pour_affichage(0), "0,00\u{a0}€");
        assert_eq!(prix_pour_affichage(-378_036_000), "-3780,36\u{a0}€");
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
