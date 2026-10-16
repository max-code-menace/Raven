//! Calculs d'un avancement, en entiers (aucun nombre à virgule) : une ligne s'avance en quantité, en pourcentage
//! ou en euros, mais ce qui est enregistré est toujours le montant de la période en centimes.
//! La quantité et le pourcentage affichés en sont déduits.

use crate::calcul;

/// Arrondi au plus proche, la moitié s'éloigne de zéro (comme pour les totaux du devis).
fn diviser_arrondi(n: i128, d: i128) -> i64 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    let q = if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) };
    q as i64
}

/// Montant de la période pour une quantité avancée : quantité × prix unitaire, au centime.
pub fn montant_depuis_quantite(quantite_e5: i64, prix_unitaire_e5: i64) -> i64 {
    calcul::total_ligne_cents(quantite_e5, prix_unitaire_e5)
}

/// Montant de la période pour un pourcentage du total de la ligne (55,04 % s'écrit 5 504 000 en 1/100 000).
pub fn montant_depuis_pourcentage(total_cents: i64, pourcentage_e5: i64) -> i64 {
    // total × (p / 1e5) / 100
    diviser_arrondi(i128::from(total_cents) * i128::from(pourcentage_e5), 10_000_000)
}

/// Un montant saisi en euros (en 1/100 000) ramené au centime.
pub fn montant_depuis_euros(euros_e5: i64) -> i64 {
    diviser_arrondi(i128::from(euros_e5), 1_000)
}

/// Part du total, en centièmes de pour cent (5504 = 55,04 %). Aucune part pour un total nul.
pub fn pourcentage_centiemes(montant_cents: i64, total_cents: i64) -> Option<i64> {
    (total_cents != 0).then(|| diviser_arrondi(i128::from(montant_cents) * 10_000, i128::from(total_cents)))
}

/// La quantité qui correspond à un montant, pour une ligne de `quantite_e5` pour `total_cents`.
pub fn quantite_depuis_montant(montant_cents: i64, total_cents: i64, quantite_e5: i64) -> Option<i64> {
    (total_cents != 0).then(|| diviser_arrondi(i128::from(montant_cents) * i128::from(quantite_e5), i128::from(total_cents)))
}

/// Le cumul d'une ligne reste entre 0 et 100 % de son total (un total négatif, comme un avenant
/// qui retire des travaux, s'avance de 0 jusqu'à ce total négatif).
pub fn cumul_permis(cumul_cents: i64, total_cents: i64) -> bool {
    if total_cents >= 0 { (0..=total_cents).contains(&cumul_cents) } else { (total_cents..=0).contains(&cumul_cents) }
}

/// 5504 → « 55,04 % » ; −250 → « -2,50 % ».
pub fn pourcentage_en_texte(centiemes: i64) -> String {
    format!("{} %", pourcentage_a_saisir(centiemes))
}

/// 5504 → « 55,04 » : la valeur proposée dans le champ de saisie.
pub fn pourcentage_a_saisir(centiemes: i64) -> String {
    let signe = if centiemes < 0 { "-" } else { "" };
    let a = centiemes.unsigned_abs();
    format!("{signe}{},{:02}", a / 100, a % 100)
}

/// 3634,47 € en centimes → « 3634,47 » : la valeur proposée dans le champ de saisie.
pub fn euros_a_saisir(cents: i64) -> String {
    let signe = if cents < 0 { "-" } else { "" };
    let a = cents.unsigned_abs();
    format!("{signe}{},{:02}", a / 100, a % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_exemple_reel_donne_55_04_pour_cent() {
        // Total devis + avenant 6 603,77 € ; avancé 3 634,47 € ; reste 2 969,30 €.
        assert_eq!(pourcentage_centiemes(363_447, 660_377), Some(5504));
        assert_eq!(pourcentage_en_texte(5504), "55,04 %");
        assert_eq!(660_377 - 363_447, 296_930);
    }

    #[test]
    fn saisir_en_quantite() {
        // 2,5 m² à 40 € = 100 €
        assert_eq!(montant_depuis_quantite(250_000, 4_000_000), 10_000);
        // la quantité affichée se retrouve depuis le montant
        assert_eq!(quantite_depuis_montant(10_000, 40_000, 1_000_000), Some(250_000));
    }

    #[test]
    fn saisir_en_pourcentage() {
        // 35 % de 10 384,13 € = 3 634,4455 → 3 634,45 €
        assert_eq!(montant_depuis_pourcentage(1_038_413, 3_500_000), 363_445);
        // 100 % redonne le total exact, même négatif
        assert_eq!(montant_depuis_pourcentage(-378_036, 10_000_000), -378_036);
        assert_eq!(montant_depuis_pourcentage(1_038_413, 0), 0);
    }

    #[test]
    fn saisir_en_euros() {
        assert_eq!(montant_depuis_euros(363_447_000), 363_447);
        assert_eq!(montant_depuis_euros(1_234_567), 1_235); // 12,34567 € → 12,35 €
        assert_eq!(montant_depuis_euros(-100_000), -100);
    }

    #[test]
    fn le_cumul_reste_entre_0_et_100_pour_cent() {
        assert!(cumul_permis(0, 1000) && cumul_permis(1000, 1000) && cumul_permis(500, 1000));
        assert!(!cumul_permis(1001, 1000) && !cumul_permis(-1, 1000));
        // une ligne négative s'avance vers son total négatif
        assert!(cumul_permis(-300, -1000) && cumul_permis(-1000, -1000) && cumul_permis(0, -1000));
        assert!(!cumul_permis(1, -1000) && !cumul_permis(-1001, -1000));
    }

    #[test]
    fn rien_a_calculer_sur_un_total_nul() {
        assert_eq!(pourcentage_centiemes(0, 0), None);
        assert_eq!(quantite_depuis_montant(0, 0, 100_000), None);
    }

    #[test]
    fn valeurs_a_saisir() {
        assert_eq!(pourcentage_a_saisir(10_000), "100,00");
        assert_eq!(pourcentage_a_saisir(-250), "-2,50");
        assert_eq!(euros_a_saisir(363_447), "3634,47");
        assert_eq!(euros_a_saisir(-5), "-0,05");
    }
}
