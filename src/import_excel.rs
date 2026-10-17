//! Lecture du modèle Excel de devis (celui que Mahx remplit depuis les métrés d'architecte et importait dans Vertuoza).
//!
//! Le modèle : un tableau avec une ligne d'en-tête « POSTE | DESCRIPTION | … | Qté | UNITE | PRIX (UNITAIRE) | PRIX (H.T.V.A.) ».
//! Les colonnes sont retrouvées par leur titre, pas par leur position. Ce qui est reconnu, ligne par ligne :
//! - « LOT n » dans la colonne POSTE → un **titre** (lot), avec le texte de DESCRIPTION ;
//! - une quantité (ou un prix) → un **poste** (description, quantité, unité, prix unitaire) ; « option » dans la colonne
//!   à côté de POSTE → la description commence par « Option : » ;
//! - un numéro (« 2.1 ») à côté de POSTE, ou un texte court en MAJUSCULES → un **sous-titre** ;
//! - un autre texte → une ligne de **texte** ; un poste sans description reprend les textes juste au-dessus de lui ;
//! - « SOUS-TOTAL » / « TOTAL » sont ignorés (l'application recalcule) ; tout s'arrête à « RECAPITULATIF ».
//! Le « TOTAL H.T.V.A. » et la TVA (« T.V.A. 6% ») du fichier servent à vérifier le résultat.

use serde::{Deserialize, Serialize};

use crate::calcul;

const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_UNITE: usize = 20;

/// Une case du tableur.
#[derive(Debug, Clone, PartialEq)]
pub enum Cellule {
    Vide,
    Texte(String),
    Nombre(f64),
}

impl Cellule {
    fn texte(&self) -> String {
        match self {
            Cellule::Texte(t) => propre(t),
            Cellule::Nombre(n) => {
                if n.fract() == 0.0 { format!("{}", *n as i64) } else { format!("{n}") }
            }
            Cellule::Vide => String::new(),
        }
    }

    /// Un nombre, écrit comme nombre ou comme texte (« 1 250,50 »). En 1/100 000.
    fn nombre_e5(&self) -> Option<i64> {
        match self {
            Cellule::Nombre(n) if n.is_finite() => Some((n * 100_000.0).round() as i64),
            Cellule::Texte(t) => calcul::lire_decimal_e5(&t.replace(['\u{a0}', ' '], "").replace('€', "")),
            _ => None,
        }
    }
}

/// Espaces resserrés, sans espaces au bout.
fn propre(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pour comparer les titres : minuscules, sans accents ni points.
fn cle(t: &str) -> String {
    propre(t)
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' => 'a',
            'î' | 'ï' => 'i',
            'ô' => 'o',
            'û' | 'ù' => 'u',
            'ç' => 'c',
            c => c,
        })
        .filter(|c| *c != '.')
        .collect()
}

/// Une ligne lue, prête à devenir une ligne de devis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LigneImport {
    /// « titre », « sous_titre », « texte » ou « poste_libre ».
    pub genre: String,
    pub description: String,
    pub quantite_e5: Option<i64>,
    pub unite: String,
    pub prix_e5: Option<i64>,
    /// Le numéro de la ligne dans le fichier (pour les messages).
    pub ligne_excel: usize,
}

impl LigneImport {
    pub fn total_cents(&self) -> i64 {
        match (self.quantite_e5, self.prix_e5) {
            (Some(q), Some(p)) => calcul::total_ligne_cents(q, p),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resultat {
    pub lignes: Vec<LigneImport>,
    /// « TOTAL H.T.V.A. » du fichier, en centimes.
    pub total_fichier_cents: Option<i64>,
    /// Taux de TVA du fichier, en centièmes (600 = 6 %).
    pub taux_fichier: Option<i32>,
    pub alertes: Vec<String>,
}

struct Colonnes {
    poste: Option<usize>,
    numero: Option<usize>,
    description: usize,
    quantite: usize,
    unite: Option<usize>,
    prix: usize,
    total: Option<usize>,
}

/// Retrouve la ligne d'en-tête et ses colonnes.
fn trouver_entete(grille: &[Vec<Cellule>]) -> Option<(usize, Colonnes)> {
    let case = |r: usize, c: usize| grille.get(r).and_then(|l| l.get(c)).map(|x| cle(&x.texte())).unwrap_or_default();
    for (r, ligne) in grille.iter().enumerate() {
        let cles: Vec<String> = ligne.iter().map(|c| cle(&c.texte())).collect();
        let Some(description) = cles.iter().position(|c| c == "description" || c == "designation") else { continue };
        let Some(quantite) = cles.iter().position(|c| matches!(c.as_str(), "qte" | "quantite" | "qt" | "qte prevue")) else { continue };
        let poste = cles.iter().position(|c| c == "poste" || c == "n°" || c == "num");
        let unite = cles.iter().position(|c| c == "unite" || c == "u");
        let prix: Vec<usize> = cles.iter().enumerate().filter(|(_, c)| c.starts_with("prix") || c.starts_with("pu")).map(|(i, _)| i).collect();
        if prix.is_empty() {
            continue;
        }
        // Le prix unitaire : la colonne « PRIX » sous laquelle on lit « UNITAIRE », sinon la première.
        let unitaire = prix.iter().copied().find(|&c| case(r + 1, c).contains("unitaire") || cles[c].contains("unitaire")).unwrap_or(prix[0]);
        let total = prix.iter().copied().filter(|&c| c != unitaire).last();
        let numero = poste.filter(|&p| p + 1 < description).map(|p| p + 1);
        return Some((r, Colonnes { poste, numero, description, quantite, unite, prix: unitaire, total }));
    }
    None
}

fn est_en_majuscules(t: &str) -> bool {
    t.chars().any(|c| c.is_alphabetic()) && t.chars().filter(|c| c.is_alphabetic()).all(|c| !c.is_lowercase())
}

/// « M2 » → « m² », « M3 » → « m³ » ; le reste tel quel.
fn unite(t: &str) -> String {
    let u = propre(t);
    let u = match u.to_lowercase().as_str() {
        "m2" => "m²".to_string(),
        "m3" => "m³".to_string(),
        _ => u,
    };
    u.chars().take(LONGUEUR_MAX_UNITE).collect()
}

/// « T.V.A. 6% » → 600.
fn taux_dans(t: &str) -> Option<i32> {
    let k = cle(t);
    if !k.starts_with("tva") && !k.starts_with("t v a") {
        return None;
    }
    let chiffres: String = k.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == ',').collect();
    if !k.contains('%') || chiffres.is_empty() {
        return None;
    }
    calcul::lire_decimal_e5(&chiffres).map(|v| (v / 1_000) as i32)
}

/// Lit le tableau. Erreur : un message simple pour Mahx.
pub fn lire_grille(grille: &[Vec<Cellule>]) -> Result<Resultat, String> {
    let (entete, col) = trouver_entete(grille)
        .ok_or("Ce fichier ne ressemble pas au modèle : je ne trouve pas la ligne d'en-tête « POSTE | DESCRIPTION | Qté | UNITE | PRIX ».")?;
    let vide = Cellule::Vide;
    let mut r = Resultat::default();
    // Les lignes de texte juste au-dessus du prochain poste (elles deviennent sa description s'il n'en a pas).
    let mut textes_en_attente: Vec<usize> = Vec::new();

    // Le total et la TVA du fichier, où qu'ils soient.
    for ligne in grille.iter().skip(entete) {
        for (c, case) in ligne.iter().enumerate() {
            let k = cle(&case.texte());
            if r.total_fichier_cents.is_none() && (k.starts_with("total htva") || k.starts_with("total h t v a") || k.starts_with("total ht")) {
                r.total_fichier_cents = ligne.iter().skip(c + 1).find_map(|x| match x {
                    Cellule::Nombre(_) => x.nombre_e5().map(|v| (v + 500).div_euclid(1_000)),
                    _ => None,
                });
            }
            if r.taux_fichier.is_none() {
                r.taux_fichier = taux_dans(&case.texte());
            }
        }
    }

    for (i, ligne) in grille.iter().enumerate().skip(entete + 1) {
        let n = i + 1;
        let case = |c: Option<usize>| c.and_then(|c| ligne.get(c)).unwrap_or(&vide);
        let textes: Vec<String> = ligne.iter().map(|c| cle(&c.texte())).collect();
        if textes.iter().any(|t| t.starts_with("recapitulatif")) {
            break;
        }
        // « SOUS-TOTAL », « TOTAL … » hors de la colonne DESCRIPTION (une description peut commencer par « Total »).
        let est_un_total = textes.iter().enumerate().any(|(c, t)| {
            c != col.description && (t.starts_with("sous-total") || t.starts_with("sous total") || t.starts_with("total"))
        });
        if est_un_total {
            textes_en_attente.clear();
            continue;
        }
        // La seconde ligne d'en-tête (« PREVUE | UNITAIRE | H.T.V.A. »).
        if i == entete + 1 && textes.iter().any(|t| t == "unitaire") {
            continue;
        }
        let poste = case(col.poste).texte();
        let numero = case(col.numero).texte();
        let mut description = case(Some(col.description)).texte();
        let quantite = case(Some(col.quantite)).nombre_e5();
        let prix = case(Some(col.prix)).nombre_e5();
        let unite_texte = unite(&case(col.unite).texte());

        if cle(&poste).starts_with("lot") {
            let titre = if description.is_empty() { poste.clone() } else { description };
            r.lignes.push(LigneImport { genre: "titre".into(), description: titre, quantite_e5: None, unite: String::new(), prix_e5: None, ligne_excel: n });
            textes_en_attente.clear();
            continue;
        }
        if quantite.is_some() || (prix.is_some() && !unite_texte.is_empty()) {
            if description.is_empty() {
                // Un poste sans description : les textes juste au-dessus sont sa description.
                let repris: Vec<String> = textes_en_attente.iter().map(|&k| r.lignes[k].description.clone()).collect();
                for &k in textes_en_attente.iter().rev() {
                    r.lignes.remove(k);
                }
                description = repris.join("\n");
                if description.is_empty() {
                    r.alertes.push(format!("Ligne {n} : un poste sans description."));
                }
            }
            if cle(&numero) == "option" {
                description = format!("Option : {description}");
            }
            if description.chars().count() > LONGUEUR_MAX_DESCRIPTION {
                description = description.chars().take(LONGUEUR_MAX_DESCRIPTION).collect();
                r.alertes.push(format!("Ligne {n} : description raccourcie à 5 000 caractères."));
            }
            if prix.is_none() {
                r.alertes.push(format!("Ligne {n} : pas de prix unitaire, la ligne est importée à 0,00 €."));
            }
            let l = LigneImport { genre: "poste_libre".into(), description, quantite_e5: quantite.or(Some(100_000)), unite: unite_texte, prix_e5: prix, ligne_excel: n };
            if let Some(total) = case(col.total).nombre_e5().map(|v| (v + 500).div_euclid(1_000))
                && prix.is_some()
                && (total - l.total_cents()).abs() > 1
            {
                r.alertes.push(format!(
                    "Ligne {n} : le fichier indique {} mais quantité × prix donne {} (c'est ce dernier qui est importé).",
                    crate::commun::format_euros(total),
                    crate::commun::format_euros(l.total_cents())
                ));
            }
            r.lignes.push(l);
            textes_en_attente.clear();
            continue;
        }
        if description.is_empty() {
            if ligne.iter().all(|c| c.texte().is_empty()) {
                textes_en_attente.clear();
            }
            continue;
        }
        let est_numero = !numero.is_empty() && numero.chars().next().is_some_and(|c| c.is_ascii_digit());
        if est_numero || (est_en_majuscules(&description) && description.chars().count() <= 100) {
            r.lignes.push(LigneImport { genre: "sous_titre".into(), description, quantite_e5: None, unite: String::new(), prix_e5: None, ligne_excel: n });
            textes_en_attente.clear();
        } else {
            if cle(&numero) == "option" {
                description = format!("Option : {description}");
            }
            r.lignes.push(LigneImport { genre: "texte".into(), description, quantite_e5: None, unite: String::new(), prix_e5: None, ligne_excel: n });
            textes_en_attente.push(r.lignes.len() - 1);
        }
    }
    if !r.lignes.iter().any(|l| l.genre == "poste_libre") {
        return Err("Aucune ligne chiffrée (avec une quantité) n'a été trouvée sous l'en-tête.".to_string());
    }
    Ok(r)
}

/// Ouvre le fichier Excel (.xlsx) et renvoie le tableau de la feuille qui contient l'en-tête du modèle.
pub fn lire_xlsx(octets: &[u8]) -> Result<Resultat, String> {
    use calamine::{Data, Reader, Xlsx};
    let mut classeur: Xlsx<_> = Xlsx::new(std::io::Cursor::new(octets.to_vec()))
        .map_err(|_| "Ce fichier n'est pas un classeur Excel (.xlsx) lisible.".to_string())?;
    let mut derniere_erreur = "Le classeur ne contient aucune feuille.".to_string();
    for nom in classeur.sheet_names() {
        let Ok(plage) = classeur.worksheet_range(&nom) else { continue };
        let (debut_ligne, debut_col) = plage.start().map(|(l, c)| (l as usize, c as usize)).unwrap_or((0, 0));
        let mut grille: Vec<Vec<Cellule>> = vec![Vec::new(); debut_ligne];
        for ligne in plage.rows() {
            let mut cases = vec![Cellule::Vide; debut_col];
            cases.extend(ligne.iter().map(|d| match d {
                Data::String(s) => Cellule::Texte(s.clone()),
                Data::Float(f) => Cellule::Nombre(*f),
                Data::Int(i) => Cellule::Nombre(*i as f64),
                _ => Cellule::Vide,
            }));
            grille.push(cases);
        }
        match lire_grille(&grille) {
            Ok(r) => return Ok(r),
            Err(e) => derniere_erreur = e,
        }
    }
    Err(derniere_erreur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Cellule {
        Cellule::Texte(s.to_string())
    }
    fn n(x: f64) -> Cellule {
        Cellule::Nombre(x)
    }
    const V: Cellule = Cellule::Vide;

    /// Le modèle de l'exemple (sans nom de client) : colonnes A à K.
    fn exemple() -> Vec<Vec<Cellule>> {
        vec![
            vec![V, V, V, V, V, V, V, t("DEVIS ")],
            vec![t("POSTE"), V, t("DESCRIPTION"), t("HAUT."), t("LONG."), t("LARG."), V, t("Qté"), t("UNITE"), t("PRIX"), t("PRIX")],
            vec![V, V, V, V, V, V, V, t("PREVUE"), V, t("UNITAIRE"), t("H.T.V.A.")],
            vec![],
            vec![t("LOT 0"), V, t("ENTREPRISE - CHANTIER")],
            vec![V, V, t("INSTALLATION DE CHANTIER")],
            vec![V, V, t("Déplacement "), V, V, V, V, n(4.0), t("QP"), n(50.0), n(200.0)],
            vec![V, V, V, V, V, V, V, V, t("SOUS-TOTAL"), V, n(200.0)],
            vec![t("LOT 1"), V, t("DEMOLITION ET RECALIBRAGE")],
            vec![V, V, t("Recalibrage de la baie ( création d'une trémie )")],
            vec![V, V, t("Prise de niveau / mesure "), n(1.0), t("fft"), n(100.0), n(100.0), n(1.0), t("fft"), n(90.0), n(90.0)],
            vec![t("LOT 2"), V, t("PARACHEVEMENT INTERIEUR")],
            vec![V, t("2.1"), t("ENDUITS INTERIEURS")],
            vec![V, t("option"), t("Plafonnage des pignons "), V, V, V, V, n(1.0), t("fft"), n(3570.0), n(3570.0)],
            vec![V, V, t("Store extérieur solaire ( en attente )"), V, V, V, V, n(1.0), t("fft"), V, n(0.0)],
            vec![V, V, t("Chape pour passages techniques")],
            vec![V, V, t("Fourniture et pose d'une chape adhérente.")],
            vec![V, V, V, V, V, V, V, n(60.0), t("M2"), n(47.0), n(2820.0)],
            vec![V, V, t("Démontage de l'existant (avenant)"), V, V, V, V, n(-2.0), t("fft"), n(150.0), n(-300.0)],
            vec![],
            vec![V, V, V, t("RECAPITULATIF")],
            vec![V, V, V, V, V, V, V, t("0. ENTREPRISE - CHANTIER"), V, V, n(200.0)],
            vec![V, V, V, V, V, V, V, V, t("TOTAL H.T.V.A."), V, n(6380.0)],
            vec![V, V, V, V, V, V, V, V, t("T.V.A. 6%"), V, n(382.8)],
        ]
    }

    #[test]
    fn le_modele_est_lu_ligne_par_ligne() {
        let r = lire_grille(&exemple()).unwrap();
        let resume: Vec<(&str, &str, Option<i64>, &str, Option<i64>)> =
            r.lignes.iter().map(|l| (l.genre.as_str(), l.description.as_str(), l.quantite_e5, l.unite.as_str(), l.prix_e5)).collect();
        assert_eq!(
            resume,
            vec![
                ("titre", "ENTREPRISE - CHANTIER", None, "", None),
                ("sous_titre", "INSTALLATION DE CHANTIER", None, "", None),
                ("poste_libre", "Déplacement", Some(400_000), "QP", Some(5_000_000)),
                ("titre", "DEMOLITION ET RECALIBRAGE", None, "", None),
                ("texte", "Recalibrage de la baie ( création d'une trémie )", None, "", None),
                ("poste_libre", "Prise de niveau / mesure", Some(100_000), "fft", Some(9_000_000)),
                ("titre", "PARACHEVEMENT INTERIEUR", None, "", None),
                ("sous_titre", "ENDUITS INTERIEURS", None, "", None),
                ("poste_libre", "Option : Plafonnage des pignons", Some(100_000), "fft", Some(357_000_000)),
                ("poste_libre", "Store extérieur solaire ( en attente )", Some(100_000), "fft", None),
                ("poste_libre", "Chape pour passages techniques\nFourniture et pose d'une chape adhérente.", Some(6_000_000), "m²", Some(4_700_000)),
                ("poste_libre", "Démontage de l'existant (avenant)", Some(-200_000), "fft", Some(15_000_000)),
            ]
        );
        assert_eq!(r.total_fichier_cents, Some(638_000));
        assert_eq!(r.taux_fichier, Some(600));
        assert_eq!(r.lignes.iter().map(LigneImport::total_cents).sum::<i64>(), 638_000, "même total que le fichier, négatif compris");
        assert_eq!(r.alertes, vec!["Ligne 15 : pas de prix unitaire, la ligne est importée à 0,00 €.".to_string()]);
        assert_eq!(r.lignes[2].ligne_excel, 7);
    }

    #[test]
    fn un_fichier_qui_n_est_pas_le_modele() {
        assert!(lire_grille(&[vec![t("Nom"), t("Prénom")]]).unwrap_err().contains("en-tête"));
        let sans_poste = vec![vec![t("POSTE"), t("DESCRIPTION"), t("Qté"), t("UNITE"), t("PRIX")], vec![t("LOT 1"), t("Titre")]];
        assert!(lire_grille(&sans_poste).unwrap_err().contains("Aucune ligne chiffrée"));
        assert!(lire_xlsx(b"pas un classeur").is_err());
    }

    #[test]
    fn petits_outils() {
        assert_eq!(taux_dans("T.V.A. 6%"), Some(600));
        assert_eq!(taux_dans("TVA 21 %"), Some(2100));
        assert_eq!(taux_dans("TOTAL T.V.A.C."), None);
        assert_eq!(unite("M2"), "m²");
        assert_eq!(unite(" fft "), "fft");
        assert_eq!(t("1 250,50").nombre_e5(), Some(125_050_000));
        assert!(est_en_majuscules("MENUISERIE EXTERIEURE") && !est_en_majuscules("Fourniture "));
    }
}
