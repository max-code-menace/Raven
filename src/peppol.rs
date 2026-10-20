//! Envoi Peppol (étape 5) : l'adresse Peppol des parties et les contrôles avant l'envoi. L'envoi lui-même passera par
//! le point d'accès Recommand (choix de Mahx du 20/10/2026, voir `docs/peppol-points-acces.md`).
//!
//! Une adresse Peppol suit la norme ISO 6523 : un schéma sur 4 chiffres et un numéro (« 0208:0123456789 »). Une
//! entreprise belge est inscrite sous son **numéro d'entreprise** (schéma 0208), le numéro de TVA sans « BE » : on le
//! déduit du numéro de TVA. Pour un client étranger ou inscrit autrement, l'identifiant se saisit sur sa fiche.

use sqlx::PgConnection;

use crate::factures::Facture;

/// Le schéma du numéro d'entreprise belge (Banque-Carrefour des Entreprises).
pub const SCHEMA_BCE: &str = "0208";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifiant {
    pub schema: String,
    pub numero: String,
}

impl std::fmt::Display for Identifiant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.schema, self.numero)
    }
}

/// Le numéro d'entreprise belge (10 chiffres) d'un numéro de TVA belge, quelle que soit son écriture
/// (« BE 0123.456.789 », « be0123456789 »). Un ancien numéro à 9 chiffres prend un 0 devant.
fn numero_bce(tva: &str) -> Option<String> {
    let compact: String = tva.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    let chiffres = compact.strip_prefix("BE")?;
    if !chiffres.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match chiffres.len() {
        10 => Some(chiffres.to_string()),
        9 => Some(format!("0{chiffres}")),
        _ => None,
    }
}

/// Les deux derniers chiffres d'un numéro d'entreprise valent 97 moins le reste de la division des huit premiers
/// par 97.
pub fn bce_valide(numero: &str) -> bool {
    if numero.len() != 10 || !numero.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let (base, controle) = numero.split_at(8);
    let base: u64 = base.parse().unwrap_or(0);
    let controle: u64 = controle.parse().unwrap_or(0);
    97 - base % 97 == controle
}

/// L'adresse Peppol déduite d'un numéro de TVA : seulement pour un numéro belge.
pub fn depuis_tva(tva: &str) -> Option<Identifiant> {
    numero_bce(tva).map(|numero| Identifiant { schema: SCHEMA_BCE.into(), numero })
}

/// Lit un identifiant saisi (« 0208:0123.456.789 », « 0088 : 5400000000001 ») et le remet au propre.
pub fn lire(texte: &str) -> Result<Identifiant, String> {
    let erreur = || "L'identifiant Peppol s'écrit avec le schéma, deux-points et le numéro (exemple : 0208:0123456789).".to_string();
    let (schema, numero) = texte.split_once(':').ok_or_else(erreur)?;
    let schema = schema.trim();
    if schema.len() != 4 || !schema.chars().all(|c| c.is_ascii_digit()) {
        return Err(erreur());
    }
    let mut numero: String = numero.trim().chars().filter(|c| !c.is_whitespace()).collect();
    if schema == SCHEMA_BCE {
        numero.retain(|c| c.is_ascii_digit());
        if !bce_valide(&numero) {
            return Err("Ce numéro d'entreprise belge n'est pas valide (10 chiffres, les deux derniers de contrôle).".into());
        }
    }
    if numero.is_empty() || numero.len() > 50 || !numero.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(erreur());
    }
    Ok(Identifiant { schema: schema.into(), numero })
}

/// L'adresse Peppol d'un client : celle de sa fiche, sinon celle de son numéro de TVA belge.
pub fn du_client(peppol_id: Option<&str>, tva: &str) -> Option<Identifiant> {
    match peppol_id.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => lire(p).ok(),
        None => depuis_tva(tva),
    }
}

/// Le bloc « Facturation électronique » de la fiche d'une facture validée.
pub struct Bloc {
    /// L'adresse Peppol du client (« 0208:0123456789 »), vide s'il n'en a pas.
    pub client: String,
    /// Un client sans numéro de TVA ni identifiant : un particulier, qui reçoit la facture par e-mail.
    pub particulier: bool,
    /// Facture reprise d'un autre logiciel : elle n'est pas renvoyée.
    pub reprise: bool,
    /// Ce qui empêche l'envoi (vide : la facture peut partir par Peppol).
    pub problemes: Vec<String>,
}

impl Bloc {
    pub fn prete(&self) -> bool {
        !self.particulier && !self.reprise && self.problemes.is_empty()
    }
}

/// Les contrôles d'une facture (ou note de crédit) validée avant son envoi par Peppol. `None` pour un brouillon.
pub(crate) async fn controler(db: &mut PgConnection, f: &Facture) -> Result<Option<Bloc>, sqlx::Error> {
    if f.numero.is_none() {
        return Ok(None);
    }
    let client: Option<(Option<String>, Option<String>, String)> = match (f.client_entreprise_id, f.client_contact_id) {
        (Some(e), _) => sqlx::query_as("SELECT peppol_id, numero_tva, pays FROM entreprises WHERE id = $1").bind(e).fetch_optional(&mut *db).await?,
        (_, Some(c)) => sqlx::query_as("SELECT peppol_id, numero_tva, pays FROM contacts WHERE id = $1").bind(c).fetch_optional(&mut *db).await?,
        _ => None,
    };
    let (peppol_id, tva, pays) = client.unwrap_or((None, None, String::new()));
    let tva = tva.unwrap_or_default();
    let identifiant = du_client(peppol_id.as_deref(), &tva);
    let particulier = identifiant.is_none() && tva.trim().is_empty() && peppol_id.is_none();

    let mut problemes = Vec::new();
    match &identifiant {
        Some(i) if i.schema == SCHEMA_BCE && !bce_valide(&i.numero) => problemes.push(format!(
            "Le numéro de TVA du client ({}) n'est pas valide : vérifiez-le sur sa fiche.",
            tva.trim()
        )),
        Some(_) => {}
        None if !particulier => problemes.push(if pays.trim().is_empty() || pays == "BE" {
            "Le numéro de TVA du client n'est pas un numéro belge valide : corrigez-le, ou saisissez son identifiant Peppol sur sa fiche.".to_string()
        } else {
            "Client hors de Belgique : saisissez son identifiant Peppol sur sa fiche (il vous le communique).".to_string()
        }),
        None => {}
    }
    let tva_societe: Option<Option<String>> =
        sqlx::query_scalar("SELECT numero_tva FROM societe WHERE organisation_id = app_organisation()").fetch_optional(&mut *db).await?;
    match depuis_tva(tva_societe.flatten().as_deref().unwrap_or("")) {
        Some(i) if bce_valide(&i.numero) => {}
        Some(_) => problemes.push("Le numéro de TVA de votre société n'est pas valide (Réglages › Société).".into()),
        None => problemes.push("Le numéro de TVA belge de votre société manque (Réglages › Société).".into()),
    }

    Ok(Some(Bloc { client: identifiant.map(|i| i.to_string()).unwrap_or_default(), particulier, reprise: f.reprise, problemes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_numero_d_entreprise_vient_du_numero_de_tva() {
        let i = depuis_tva("BE 0123.456.749").unwrap();
        assert_eq!(i.to_string(), "0208:0123456749");
        assert_eq!(depuis_tva("be123456749").unwrap().numero, "0123456749", "ancien numéro à 9 chiffres");
        assert_eq!(depuis_tva("FR12345678901"), None, "pas de déduction hors de Belgique");
        assert_eq!(depuis_tva(""), None);
        assert_eq!(depuis_tva("BE12"), None);
    }

    #[test]
    fn chiffres_de_controle() {
        assert!(bce_valide("0123456749"));
        assert!(!bce_valide("0123456748"));
        assert!(!bce_valide("0123456789"));
        assert!(!bce_valide("012345674"));
    }

    #[test]
    fn identifiant_saisi() {
        assert_eq!(lire(" 0208 : 0123.456.749 ").unwrap().to_string(), "0208:0123456749");
        assert_eq!(lire("0088:5400000000001").unwrap().to_string(), "0088:5400000000001");
        assert_eq!(lire("9925:BE0123456749").unwrap().numero, "BE0123456749");
        assert!(lire("0208:0123456789").is_err(), "chiffres de contrôle faux");
        assert!(lire("0123456749").is_err(), "sans schéma");
        assert!(lire("208:0123456749").is_err());
        assert!(lire("0088:").is_err());
        assert!(lire("0088:54 00<>").is_err());
    }

    #[test]
    fn adresse_du_client() {
        assert_eq!(du_client(None, "BE0123456749").unwrap().to_string(), "0208:0123456749");
        assert_eq!(du_client(Some("0088:5400000000001"), "BE0123456749").unwrap().to_string(), "0088:5400000000001", "la fiche l'emporte");
        assert_eq!(du_client(Some(""), "BE0123456749").unwrap().schema, "0208");
        assert_eq!(du_client(None, ""), None);
    }
}
