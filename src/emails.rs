//! E-mails (étape 8-2) : modèles modifiables, variables entre crochets, envoi par l'API de Brevo.
//!
//! **Sécurité** : rien ne part tant que `configurer` n'a pas reçu de clé Brevo. Seul `main.rs` l'appelle, avec la
//! variable d'environnement `BREVO_CLE_API` ; les tests ne passent jamais par `main` : pendant les tests, chaque
//! e-mail est seulement enregistré dans la table `emails` (statut « enregistre »).

use std::sync::OnceLock;

use base64::Engine;
use chrono::NaiveDate;
use sqlx::PgPool;

use crate::commun::{email_valide, format_euros};

const ADRESSE_PAR_DEFAUT: &str = "http://127.0.0.1:3001";
const API_BREVO: &str = "https://api.brevo.com/v3/smtp/email";
pub(crate) const LONGUEUR_MAX_OBJET: usize = 300;
pub(crate) const LONGUEUR_MAX_CORPS: usize = 20_000;

struct Config {
    cle_brevo: Option<String>,
    adresse_publique: String,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Appelé une seule fois par `main.rs` au lancement. Sans clé, les e-mails sont seulement enregistrés.
/// `adresse_publique` : l'adresse du site vue par les clients (liens vers la page client en ligne).
pub fn configurer(cle_brevo: Option<String>, adresse_publique: Option<String>) {
    let cle_brevo = cle_brevo.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    let adresse_publique = adresse_publique
        .map(|a| a.trim().trim_end_matches('/').to_string())
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| ADRESSE_PAR_DEFAUT.to_string());
    let _ = CONFIG.set(Config { cle_brevo, adresse_publique });
}

/// Une clé Brevo est configurée : les e-mails partent vraiment.
pub fn envoi_actif() -> bool {
    CONFIG.get().is_some_and(|c| c.cle_brevo.is_some())
}

/// « https://ubifacture.exemple.be » : le début des liens envoyés aux clients.
pub fn adresse_publique() -> String {
    CONFIG.get().map(|c| c.adresse_publique.clone()).unwrap_or_else(|| ADRESSE_PAR_DEFAUT.to_string())
}

// ─── Modèles et variables ───────────────────────────────────────────────────

/// Les variables connues, avec ce qu'elles deviennent (affiché dans Réglages › E-mails).
pub const VARIABLES: [(&str, &str); 19] = [
    ("[CIVILITE_CLIENT]", "Monsieur ou Madame (contact), rien pour une entreprise"),
    ("[NOM_CLIENT]", "Nom du client"),
    ("[NUMBER_QUOTE]", "Numéro du devis (2026-57) ou de l'avenant"),
    ("[DATE_DEVIS]", "Date du devis"),
    ("[LINK_DEVIS]", "Lien vers la page en ligne du devis (consulter, signer)"),
    ("[NO_FACTURE]", "Numéro de la facture ou de la note de crédit (2026-125)"),
    ("[DATE_FACTURE]", "Date de la facture"),
    ("[MONTANT_TTC]", "Montant TTC de la pièce"),
    ("[RESTE_A_PAYER]", "Reste à payer TTC"),
    ("[ECHEANCE]", "Date d'échéance"),
    ("[COMMUNICATION]", "Communication structurée"),
    ("[IBAN]", "IBAN par défaut de la société"),
    ("[LINK_FACTURE]", "Lien vers la page en ligne de la facture (consulter, payer par QR code)"),
    ("[REFERENCE]", "Référence de la pièce"),
    ("[NOM_CHANTIER]", "Chantier (sa référence, sinon son numéro)"),
    ("[NOM_GESTIONNAIRE]", "Responsable de la pièce (sinon le nom de la société)"),
    ("[MAIL_GESTIONNAIRE]", "E-mail de la société"),
    ("[NOM_SOCIETE]", "Nom de la société"),
    ("[PERIODE]", "Période d'un export pour le comptable"),
];

/// Remplace les variables par leur valeur ; une variable inconnue reste telle quelle.
/// Les espaces doublés par une variable vide (« Bonjour  Client SA ») sont resserrés.
pub fn remplir(modele: &str, valeurs: &[(&str, String)]) -> String {
    let mut texte = modele.to_string();
    for (nom, valeur) in valeurs {
        texte = texte.replace(nom, valeur);
    }
    texte
        .lines()
        .map(|ligne| {
            let mut propre = String::with_capacity(ligne.len());
            for c in ligne.chars() {
                if c == ' ' && propre.ends_with(' ') {
                    continue;
                }
                propre.push(c);
            }
            propre
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) async fn modele(db: &PgPool, code: &str) -> Result<(String, String), sqlx::Error> {
    sqlx::query_as("SELECT objet, corps FROM modeles_email WHERE code = $1").bind(code).fetch_one(db).await
}

/// Le lien secret d'une pièce (créé au premier besoin, jamais changé ensuite).
pub(crate) async fn jeton(db: &PgPool, table: Piece, id: i64) -> Result<String, sqlx::Error> {
    let nouveau = nouveau_jeton();
    let sql = match table {
        Piece::Devis => "UPDATE devis SET jeton = COALESCE(jeton, $2) WHERE id = $1 RETURNING jeton",
        Piece::Facture => "UPDATE factures SET jeton = COALESCE(jeton, $2) WHERE id = $1 RETURNING jeton",
    };
    sqlx::query_scalar(sql).bind(id).bind(nouveau).fetch_one(db).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Piece {
    Devis,
    Facture,
}

/// 144 bits de hasard, écrits en 24 caractères sûrs dans une adresse.
fn nouveau_jeton() -> String {
    let mut octets = [0u8; 18];
    getrandom::fill(&mut octets).expect("le système ne fournit pas de hasard");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(octets)
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// Ce que l'on sait du client pour l'e-mail : (nom, civilité, e-mail).
async fn client(db: &PgPool, entreprise: Option<i64>, contact: Option<i64>) -> Result<(String, String, String), sqlx::Error> {
    if let Some(id) = entreprise {
        let ligne: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT nom, email FROM entreprises WHERE id = $1").bind(id).fetch_optional(db).await?;
        if let Some((nom, email)) = ligne {
            return Ok((nom, String::new(), email.unwrap_or_default()));
        }
    }
    if let Some(id) = contact {
        let ligne: Option<(String, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT trim(concat_ws(' ', prenom, nom)), civilite, email FROM contacts WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?;
        if let Some((nom, civilite, email)) = ligne {
            let civilite = match civilite.as_deref() {
                Some("M.") => "Monsieur",
                Some("Mme.") => "Madame",
                _ => "",
            };
            return Ok((nom, civilite.to_string(), email.unwrap_or_default()));
        }
    }
    Ok((String::new(), String::new(), String::new()))
}

/// (nom, e-mail, IBAN par défaut) de la société.
pub(crate) async fn societe(db: &PgPool) -> Result<(String, String, String), sqlx::Error> {
    let ligne: Option<(String, String)> = sqlx::query_as("SELECT nom, email FROM societe WHERE id = 1").fetch_optional(db).await?;
    let iban: Option<String> = sqlx::query_scalar("SELECT iban FROM societe_comptes_bancaires WHERE par_defaut").fetch_optional(db).await?;
    let (nom, email) = ligne.unwrap_or_default();
    Ok((nom, email, iban.unwrap_or_default()))
}

/// Les valeurs des variables pour un devis ou un avenant, et l'e-mail du client.
pub(crate) async fn variables_devis(db: &PgPool, id: i64) -> Result<(Vec<(&'static str, String)>, String), sqlx::Error> {
    let (numero, annee, date, reference, responsable, entreprise, contact, chantier): (
        i64,
        i32,
        NaiveDate,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = sqlx::query_as(
        "SELECT numero, extract(year FROM date_devis)::int, date_devis, reference, responsable, client_entreprise_id, client_contact_id, chantier_id
         FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    let nom_chantier: Option<String> = sqlx::query_scalar(
        "SELECT CASE WHEN d.reference <> '' THEN d.reference ELSE 'n° ' || ch.numero END
         FROM chantiers ch JOIN devis d ON d.id = ch.devis_id WHERE ch.id = $1 OR ch.devis_id = $2",
    )
    .bind(chantier)
    .bind(id)
    .fetch_optional(db)
    .await?;
    let (nom_client, civilite, email) = client(db, entreprise, contact).await?;
    let (nom_societe, email_societe, _) = societe(db).await?;
    let lien = format!("{}/client/devis/{}", adresse_publique(), jeton(db, Piece::Devis, id).await?);
    let gestionnaire = if responsable.trim().is_empty() { nom_societe.clone() } else { responsable };
    Ok((
        vec![
            ("[CIVILITE_CLIENT]", civilite),
            ("[NOM_CLIENT]", nom_client),
            ("[NUMBER_QUOTE]", crate::devis_lignes::numero_affiche(annee, numero, chantier.is_some())),
            ("[DATE_DEVIS]", date_belge(date)),
            ("[LINK_DEVIS]", lien),
            ("[REFERENCE]", reference),
            ("[NOM_CHANTIER]", nom_chantier.unwrap_or_default()),
            ("[NOM_GESTIONNAIRE]", gestionnaire),
            ("[MAIL_GESTIONNAIRE]", email_societe),
            ("[NOM_SOCIETE]", nom_societe),
        ],
        email,
    ))
}

/// Les valeurs des variables pour une facture ou une note de crédit, et l'e-mail du client facturé.
pub(crate) async fn variables_facture(db: &PgPool, id: i64) -> Result<(Vec<(&'static str, String)>, String), sqlx::Error> {
    let (numero, date, echeance, reference, responsable, entreprise, contact, chantier, vcs, ttc): (
        Option<i64>,
        NaiveDate,
        Option<NaiveDate>,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        i64,
    ) = sqlx::query_as(
        "SELECT numero, date_facture, date_echeance, reference, responsable, client_entreprise_id, client_contact_id, chantier_id, vcs,
                montant_ttc_cents
         FROM factures WHERE id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    let reste: i64 = sqlx::query_scalar("SELECT GREATEST(reste_cents, 0) FROM factures_etats WHERE facture_id = $1").bind(id).fetch_one(db).await?;
    let nom_chantier: Option<String> = sqlx::query_scalar(
        "SELECT CASE WHEN d.reference <> '' THEN d.reference ELSE 'n° ' || ch.numero END
         FROM chantiers ch JOIN devis d ON d.id = ch.devis_id WHERE ch.id = $1",
    )
    .bind(chantier)
    .fetch_optional(db)
    .await?;
    let (nom_client, civilite, email) = client(db, entreprise, contact).await?;
    let (nom_societe, email_societe, iban) = societe(db).await?;
    let lien = format!("{}/client/facture/{}", adresse_publique(), jeton(db, Piece::Facture, id).await?);
    let gestionnaire = if responsable.trim().is_empty() { nom_societe.clone() } else { responsable };
    Ok((
        vec![
            ("[CIVILITE_CLIENT]", civilite),
            ("[NOM_CLIENT]", nom_client),
            ("[NO_FACTURE]", numero.map(|n| crate::factures::numero_affiche(date, n)).unwrap_or_default()),
            ("[DATE_FACTURE]", date_belge(date)),
            ("[MONTANT_TTC]", format_euros(ttc)),
            ("[RESTE_A_PAYER]", format_euros(reste)),
            ("[ECHEANCE]", echeance.map(date_belge).unwrap_or_default()),
            ("[COMMUNICATION]", vcs.unwrap_or_default()),
            ("[IBAN]", iban),
            ("[LINK_FACTURE]", lien),
            ("[REFERENCE]", reference),
            ("[NOM_CHANTIER]", nom_chantier.unwrap_or_default()),
            ("[NOM_GESTIONNAIRE]", gestionnaire),
            ("[MAIL_GESTIONNAIRE]", email_societe),
            ("[NOM_SOCIETE]", nom_societe),
        ],
        email,
    ))
}

/// « a@b.be, c@d.be » ou « a@b.be; c@d.be » → la liste ; `None` si une adresse n'est pas valable.
pub fn lire_adresses(texte: &str) -> Option<Vec<String>> {
    let adresses: Vec<String> =
        texte.split([',', ';']).map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
    adresses.iter().all(|a| email_valide(a)).then_some(adresses)
}

// ─── L'envoi ────────────────────────────────────────────────────────────────

pub(crate) struct Message {
    pub(crate) modele: &'static str,
    pub(crate) devis_id: Option<i64>,
    pub(crate) facture_id: Option<i64>,
    pub(crate) destinataires: Vec<String>,
    pub(crate) copie: Vec<String>,
    pub(crate) objet: String,
    pub(crate) corps: String,
    /// (nom du fichier, contenu).
    pub(crate) pieces: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Issue {
    /// Brevo a accepté l'e-mail.
    Envoye,
    /// Aucune clé : l'e-mail est seulement enregistré.
    Enregistre,
    /// Brevo a refusé ou n'a pas répondu (le détail est enregistré avec l'e-mail).
    Echec,
}

impl Issue {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Issue::Envoye => "envoye",
            Issue::Enregistre => "enregistre",
            Issue::Echec => "echec",
        }
    }
}

/// Le corps de la requête envoyée à Brevo.
fn requete_brevo(expediteur: (&str, &str), m: &Message) -> serde_json::Value {
    let personnes = |liste: &[String]| liste.iter().map(|a| serde_json::json!({ "email": a })).collect::<Vec<_>>();
    let mut corps = serde_json::json!({
        "sender": { "name": expediteur.0, "email": expediteur.1 },
        "to": personnes(&m.destinataires),
        "subject": m.objet,
        "textContent": m.corps,
    });
    if !m.copie.is_empty() {
        corps["cc"] = serde_json::Value::Array(personnes(&m.copie));
    }
    if !m.pieces.is_empty() {
        corps["attachment"] = serde_json::Value::Array(
            m.pieces
                .iter()
                .map(|(nom, contenu)| serde_json::json!({ "name": nom, "content": base64::engine::general_purpose::STANDARD.encode(contenu) }))
                .collect(),
        );
    }
    corps
}

/// Envoie (ou enregistre seulement, sans clé) un e-mail et le note dans le journal des e-mails.
pub(crate) async fn envoyer(db: &PgPool, m: Message) -> Result<(i64, Issue), sqlx::Error> {
    let cle = CONFIG.get().and_then(|c| c.cle_brevo.clone());
    let (issue, erreur, reference) = match cle {
        None => (Issue::Enregistre, String::new(), String::new()),
        Some(cle) => {
            let (nom, email, _) = societe(db).await?;
            if !email_valide(&email) {
                (Issue::Echec, "La société n'a pas d'adresse e-mail valable (Réglages › Société).".to_string(), String::new())
            } else {
                let corps = requete_brevo((&nom, &email), &m);
                let reponse = tokio::task::spawn_blocking(move || {
                    ureq::post(API_BREVO)
                        .set("api-key", &cle)
                        .set("accept", "application/json")
                        .timeout(std::time::Duration::from_secs(30))
                        .send_json(corps)
                        .map_err(|e| match e {
                            ureq::Error::Status(code, r) => format!("Brevo a répondu {code} : {}", r.into_string().unwrap_or_default()),
                            autre => format!("Brevo injoignable : {autre}"),
                        })
                        .and_then(|r| r.into_json::<serde_json::Value>().map_err(|e| e.to_string()))
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                match reponse {
                    Ok(v) => (Issue::Envoye, String::new(), v["messageId"].as_str().unwrap_or_default().to_string()),
                    Err(e) => {
                        eprintln!("E-mail non envoyé : {e}");
                        (Issue::Echec, e, String::new())
                    }
                }
            }
        }
    };
    let noms: Vec<String> = m.pieces.iter().map(|(n, _)| n.clone()).collect();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO emails (modele, devis_id, facture_id, destinataires, copie, objet, corps, pieces, statut, erreur, reference_brevo)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(m.modele)
    .bind(m.devis_id)
    .bind(m.facture_id)
    .bind(m.destinataires.join(", "))
    .bind(m.copie.join(", "))
    .bind(&m.objet)
    .bind(&m.corps)
    .bind(&noms)
    .bind(issue.code())
    .bind(&erreur)
    .bind(&reference)
    .fetch_one(db)
    .await?;
    Ok((id, issue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_variables_sont_remplacees() {
        let texte = remplir(
            "Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],\nFacture n° [NO_FACTURE] : [MONTANT_TTC]. [INCONNUE]",
            &[("[CIVILITE_CLIENT]", String::new()), ("[NOM_CLIENT]", "Client SA".into()), ("[NO_FACTURE]", "125".into()), ("[MONTANT_TTC]", "1 020,98 €".into())],
        );
        assert_eq!(texte, "Bonjour Client SA,\nFacture n° 125 : 1 020,98 €. [INCONNUE]");
    }

    #[test]
    fn adresses_lues() {
        assert_eq!(lire_adresses("a@b.be, c@d.be; "), Some(vec!["a@b.be".to_string(), "c@d.be".to_string()]));
        assert_eq!(lire_adresses(""), Some(vec![]));
        assert_eq!(lire_adresses("a@b.be, pas-une-adresse"), None);
    }

    #[test]
    fn les_jetons_sont_longs_et_differents() {
        let (a, b) = (nouveau_jeton(), nouveau_jeton());
        assert_eq!(a.len(), 24);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn la_requete_brevo() {
        let m = Message {
            modele: "facture",
            devis_id: None,
            facture_id: Some(1),
            destinataires: vec!["client@exemple.be".into()],
            copie: vec![],
            objet: "Facture n° 125".into(),
            corps: "Bonjour".into(),
            pieces: vec![("Facture-125.pdf".into(), b"%PDF".to_vec())],
        };
        let r = requete_brevo(("Ubique", "info@exemple.be"), &m);
        assert_eq!(r["sender"]["email"], "info@exemple.be");
        assert_eq!(r["to"][0]["email"], "client@exemple.be");
        assert_eq!(r["attachment"][0]["name"], "Facture-125.pdf");
        assert_eq!(r["attachment"][0]["content"], "JVBERg==");
        assert!(r.get("cc").is_none());
    }

    #[test]
    fn sans_configuration_rien_ne_part() {
        // Les tests n'appellent jamais `configurer` : l'envoi reste coupé.
        assert!(!envoi_actif());
        assert_eq!(adresse_publique(), "http://127.0.0.1:3001");
    }
}
