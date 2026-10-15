//! Petits outils partagés par plusieurs écrans.

pub const PAYS: [(&str, &str); 16] = [
    ("BE", "Belgique"),
    ("FR", "France"),
    ("LU", "Luxembourg"),
    ("NL", "Pays-Bas"),
    ("DE", "Allemagne"),
    ("GB", "Angleterre"),
    ("ES", "Espagne"),
    ("HU", "Hongrie"),
    ("IE", "Irlande"),
    ("IT", "Italie"),
    ("MC", "Monaco"),
    ("PL", "Pologne"),
    ("PT", "Portugal"),
    ("CZ", "République Tchèque"),
    ("RO", "Roumanie"),
    ("CH", "Suisse"),
];

/// Une option d'un menu déroulant ou d'une case à cocher.
pub struct Choix {
    pub valeur: String,
    pub libelle: String,
    pub choisi: bool,
}

pub fn choix_fixes(liste: &[(&str, &str)], actuels: &[String]) -> Vec<Choix> {
    liste
        .iter()
        .map(|(v, l)| Choix {
            valeur: (*v).to_string(),
            libelle: (*l).to_string(),
            choisi: actuels.iter().any(|a| a == v),
        })
        .collect()
}

pub fn libelle(liste: &[(&str, &str)], valeur: &str) -> String {
    liste
        .iter()
        .find(|(v, _)| *v == valeur)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_else(|| valeur.to_string())
}

pub fn email_valide(s: &str) -> bool {
    !s.contains(char::is_whitespace)
        && s.split_once('@')
            .is_some_and(|(local, domaine)| !local.is_empty() && domaine.contains('.') && !domaine.ends_with('.'))
}

pub fn vide_en_none(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// 443184 → « 4 431,84 € » (espaces insécables, pour que le montant ne se coupe pas en fin de ligne).
pub fn format_euros(centimes: i64) -> String {
    let signe = if centimes < 0 { "-" } else { "" };
    let absolu = centimes.unsigned_abs();
    let entier = (absolu / 100).to_string();
    let mut groupes: Vec<&str> = Vec::new();
    let mut fin = entier.len();
    while fin > 0 {
        let debut = fin.saturating_sub(3);
        groupes.push(&entier[debut..fin]);
        fin = debut;
    }
    groupes.reverse();
    format!("{signe}{},{:02}\u{a0}€", groupes.join("\u{a0}"), absolu % 100)
}

/// Les caractères % et _ sont des jokers dans une recherche SQL : on les neutralise.
pub fn echapper_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Encode un texte pour le mettre dans une adresse (« a b&c » → « a%20b%26c »).
pub fn encoder_url(texte: &str) -> String {
    let mut sortie = String::new();
    for octet in texte.bytes() {
        if octet.is_ascii_alphanumeric() || matches!(octet, b'-' | b'_' | b'.' | b'~') {
            sortie.push(octet as char);
        } else {
            sortie.push_str(&format!("%{octet:02X}"));
        }
    }
    sortie
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn montants_affiches_a_la_belge() {
        assert_eq!(format_euros(0), "0,00\u{a0}€");
        assert_eq!(format_euros(7), "0,07\u{a0}€");
        assert_eq!(format_euros(443_184), "4\u{a0}431,84\u{a0}€");
        assert_eq!(format_euros(115_000), "1\u{a0}150,00\u{a0}€");
        assert_eq!(format_euros(100_000_000), "1\u{a0}000\u{a0}000,00\u{a0}€");
        assert_eq!(format_euros(-12_345), "-123,45\u{a0}€");
        assert_eq!(format_euros(99_999), "999,99\u{a0}€");
    }

    #[test]
    fn adresses_encodees() {
        assert_eq!(encoder_url("a b&c=é"), "a%20b%26c%3D%C3%A9");
        assert_eq!(encoder_url("simple-1_2.3~"), "simple-1_2.3~");
    }
}
