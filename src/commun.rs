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
