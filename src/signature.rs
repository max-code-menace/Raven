//! Dossier de preuve de la signature en ligne d'un devis (voir la migration 0034).
//!
//! La signature du client est une signature électronique **simple renforcée** (eIDAS, art. 25 : elle ne peut être écartée
//! au seul motif qu'elle est électronique). Elle est renforcée par : un code à usage unique envoyé à l'adresse e-mail du
//! client (preuve qu'il contrôle cette boîte), l'empreinte SHA-256 et la copie du PDF exact signé, l'heure du serveur,
//! l'adresse IP et le navigateur, le texte exact accepté, et l'envoi d'un exemplaire signé au client.
//!
//! Limites à connaître (recherche du 19/10/2026, à faire valider par un avocat) : une signature simple reste contestable
//! (c'est à l'artisan de prouver qu'elle émane bien du client) ; face à un particulier, un écrit signé est exigé dès
//! 3 500 € (art. 8.9 du Code civil) et la signature qualifiée (itsme) est recommandée à partir de ce montant. Sans clé
//! d'envoi d'e-mails (mode test) ou sans adresse e-mail du client, la signature est acceptée mais le dossier dit que
//! l'adresse n'a pas été vérifiée.

use axum::{
    Router,
    extract::Path,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use sha2::{Digest, Sha256};
use sqlx::{Connection as _, PgConnection, Postgres, Transaction};

use crate::archive::empreinte;
use crate::base::Base;
use crate::{AppState, Erreur};

const VALIDITE_MINUTES: i64 = 10;
const ESSAIS_MAX: i32 = 5;
const CODES_MAX_PAR_QUART_D_HEURE: i64 = 3;

/// Comment l'adresse e-mail du client a été vérifiée (ou pas).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verification {
    /// Le client a saisi le code reçu à son adresse.
    CodeParEmail,
    /// Aucune clé d'envoi : rien n'est parti, l'adresse n'est pas vérifiée.
    ModeTest,
    /// Pas d'adresse e-mail sur le devis.
    SansEmail,
}

impl Verification {
    pub fn code(self) -> &'static str {
        match self {
            Verification::CodeParEmail => "code_par_email",
            Verification::ModeTest => "mode_test",
            Verification::SansEmail => "sans_email",
        }
    }

    pub fn libelle(code: &str) -> &'static str {
        match code {
            "code_par_email" => "Adresse e-mail vérifiée par un code à usage unique",
            "mode_test" => "Adresse e-mail non vérifiée (mode test : aucune clé d'envoi)",
            _ => "Adresse e-mail non vérifiée (pas d'adresse sur le devis)",
        }
    }

    /// Un code par e-mail est exigé quand on peut en envoyer un.
    pub fn pour(envoi_actif: bool, email: &str) -> Verification {
        if !crate::commun::email_valide(email) {
            Verification::SansEmail
        } else if envoi_actif {
            Verification::CodeParEmail
        } else {
            Verification::ModeTest
        }
    }
}

/// Un code à six chiffres.
pub fn nouveau_code() -> String {
    let mut octets = [0u8; 4];
    getrandom::fill(&mut octets).expect("le système ne fournit pas de hasard");
    format!("{:06}", u32::from_le_bytes(octets) % 1_000_000)
}

/// L'empreinte gardée à la place du code (le code lui-même n'est jamais enregistré).
fn empreinte_du_code(devis_id: i64, email: &str, code: &str) -> String {
    hex::encode(Sha256::digest(format!("{devis_id}:{}:{code}", email.trim().to_lowercase()).as_bytes()))
}

/// « jean.dupont@exemple.be » → « j***@exemple.be ».
pub fn masquer_email(email: &str) -> String {
    match email.trim().split_once('@') {
        Some((nom, domaine)) if !nom.is_empty() => format!("{}***@{domaine}", nom.chars().next().unwrap_or('*')),
        _ => "***".to_string(),
    }
}

/// Crée un code à usage unique pour ce devis et cette adresse. `None` : trop de codes demandés (3 par quart d'heure).
pub async fn emettre_code(db: &mut PgConnection, devis_id: i64, email: &str) -> Result<Option<String>, sqlx::Error> {
    let recents: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_codes_signature WHERE devis_id = $1 AND created_at > now() - interval '15 minutes'")
        .bind(devis_id)
        .fetch_one(&mut *db)
        .await?;
    if recents >= CODES_MAX_PAR_QUART_D_HEURE {
        return Ok(None);
    }
    let code = nouveau_code();
    sqlx::query("INSERT INTO devis_codes_signature (devis_id, email, empreinte_code, expire_le) VALUES ($1, $2, $3, now() + make_interval(mins => $4))")
        .bind(devis_id)
        .bind(email.trim().to_lowercase())
        .bind(empreinte_du_code(devis_id, email, &code))
        .bind(VALIDITE_MINUTES as i32)
        .execute(&mut *db)
        .await?;
    Ok(Some(code))
}

/// Vérifie le dernier code encore valable. Chaque vérification compte comme un essai ; un code juste ne sert qu'une fois.
pub async fn verifier_code(db: &mut PgConnection, devis_id: i64, email: &str, code: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let ligne: Option<(i64, String, i32)> = sqlx::query_as(
        "SELECT id, empreinte_code, essais FROM devis_codes_signature
         WHERE devis_id = $1 AND email = $2 AND expire_le > now() AND NOT utilise ORDER BY id DESC LIMIT 1 FOR UPDATE",
    )
    .bind(devis_id)
    .bind(email.trim().to_lowercase())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, attendue, essais)) = ligne else { return Ok(false) };
    if essais >= ESSAIS_MAX {
        return Ok(false);
    }
    let bon = attendue == empreinte_du_code(devis_id, email, code.trim());
    sqlx::query("UPDATE devis_codes_signature SET essais = essais + 1, utilise = $2 WHERE id = $1").bind(id).bind(bon).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(bon)
}

/// Le navigateur déclaré, nettoyé et raccourci.
pub fn navigateur(entetes: &HeaderMap) -> String {
    entetes.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").chars().filter(|c| !c.is_control()).take(300).collect()
}

/// Le texte que le client accepte en signant : le même est affiché à l'écran et gardé dans le dossier.
/// `conditions_generales` : l'entreprise a déposé des conditions générales (elles sont jointes au PDF du devis) ; sans
/// elles, le texte n'en parle pas (le client ne peut pas accepter un document qui n'existe pas).
pub fn texte_accord(titre: &str, date: &str, ttc: &str, entreprise: &str, conditions_generales: bool, verification: Verification) -> String {
    let conditions = if conditions_generales { format!(", ainsi que les conditions générales de {entreprise},") } else { String::new() };
    let adresse = if verification == Verification::CodeParEmail { "mon adresse e-mail vérifiée, " } else { "" };
    let exemplaire = if verification == Verification::SansEmail { "" } else { " Un exemplaire signé me sera envoyé immédiatement par e-mail." };
    format!(
        "En cochant la case « Bon pour accord » et en cliquant sur « Signer », je reconnais avoir lu {titre} du {date} d'un montant de {ttc} TVA comprise{conditions} et je l'accepte. Ma signature électronique est enregistrée avec la date, l'heure, {adresse}l'empreinte numérique du document et mon adresse IP.{exemplaire}"
    )
}

/// Ce qui est gardé au moment de la signature.
pub(crate) struct Preuve {
    pub devis_id: i64,
    pub nom: String,
    pub email: String,
    pub verification: Verification,
    pub ip: String,
    pub navigateur: String,
    pub texte_accepte: String,
    pub montant_ttc_cents: i64,
    pub pdf_nom: String,
    pub pdf: Vec<u8>,
}

/// Écrit le dossier dans la transaction de la signature. Renvoie l'empreinte du PDF signé.
pub(crate) async fn enregistrer(tx: &mut Transaction<'_, Postgres>, p: &Preuve) -> Result<String, sqlx::Error> {
    let hash = empreinte(&p.pdf);
    sqlx::query(
        "INSERT INTO devis_signatures (devis_id, nom, email, verification, ip, navigateur, texte_accepte, montant_ttc_cents, pdf_nom, pdf, empreinte_pdf)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(p.devis_id)
    .bind(&p.nom)
    .bind(p.email.trim().to_lowercase())
    .bind(p.verification.code())
    .bind(&p.ip)
    .bind(&p.navigateur)
    .bind(&p.texte_accepte)
    .bind(p.montant_ttc_cents)
    .bind(&p.pdf_nom)
    .bind(&p.pdf)
    .bind(&hash)
    .execute(&mut **tx)
    .await?;
    Ok(hash)
}

/// Le dossier, tel que l'artisan le consulte.
pub struct Dossier {
    pub quand: String,
    pub nom: String,
    pub email: String,
    pub verification: &'static str,
    pub verifiee: bool,
    pub ip: String,
    pub navigateur: String,
    pub texte_accepte: String,
    pub empreinte: String,
    pub telechargement: String,
}

pub(crate) async fn dossier(db: &mut PgConnection, devis_id: i64) -> Result<Option<Dossier>, sqlx::Error> {
    let ligne: Option<(String, String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT to_char(signe_le AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY · HH24:MI:SS'), nom, email, verification, ip, navigateur, texte_accepte, empreinte_pdf
         FROM devis_signatures WHERE devis_id = $1",
    )
    .bind(devis_id)
    .fetch_optional(&mut *db)
    .await?;
    Ok(ligne.map(|(quand, nom, email, verification, ip, navigateur, texte_accepte, empreinte)| Dossier {
        quand,
        nom,
        email,
        verifiee: verification == "code_par_email",
        verification: Verification::libelle(&verification),
        ip,
        navigateur,
        texte_accepte,
        empreinte,
        telechargement: format!("/devis/{devis_id}/signature/pdf"),
    }))
}

/// Le PDF signé, tel que le client l'a signé (nom, octets).
pub(crate) async fn pdf_signe(db: &mut PgConnection, devis_id: i64) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    sqlx::query_as("SELECT pdf_nom, pdf FROM devis_signatures WHERE devis_id = $1").bind(devis_id).fetch_optional(&mut *db).await
}

pub(crate) fn reponse_pdf(nom: &str, octets: Vec<u8>, disposition: &str) -> Result<Response, Erreur> {
    let mut reponse = octets.into_response();
    let entetes = reponse.headers_mut();
    entetes.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    // Le nom ne contient que des lettres sans accent, des chiffres et des tirets : il est sûr dans un en-tête.
    entetes.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("{disposition}; filename=\"{nom}\"")).map_err(|_| Erreur::pdf("nom de fichier"))?,
    );
    entetes.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(reponse)
}

/// Télécharger le PDF signé (côté artisan, derrière la connexion).
async fn telecharger(mut db: Base, Path(id): Path<i64>) -> Result<Response, Erreur> {
    let (nom, octets) = pdf_signe(&mut *db, id).await?.ok_or_else(Erreur::introuvable)?;
    reponse_pdf(&nom, octets, "attachment")
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/devis/{id}/signature/pdf", get(telecharger))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_code_a_six_chiffres() {
        for _ in 0..50 {
            let c = nouveau_code();
            assert_eq!(c.len(), 6);
            assert!(c.chars().all(|x| x.is_ascii_digit()));
        }
    }

    #[test]
    fn l_empreinte_du_code_depend_du_devis_de_l_adresse_et_du_code() {
        let a = empreinte_du_code(1, "Jean@exemple.be", "123456");
        assert_eq!(a, empreinte_du_code(1, "jean@exemple.be ", "123456"), "majuscules et espaces ignorés");
        assert_ne!(a, empreinte_du_code(2, "jean@exemple.be", "123456"));
        assert_ne!(a, empreinte_du_code(1, "autre@exemple.be", "123456"));
        assert_ne!(a, empreinte_du_code(1, "jean@exemple.be", "654321"));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn l_adresse_e_mail_est_masquee() {
        assert_eq!(masquer_email("jean.dupont@exemple.be"), "j***@exemple.be");
        assert_eq!(masquer_email("pas-une-adresse"), "***");
    }

    #[test]
    fn la_verification_depend_de_l_envoi_et_de_l_adresse() {
        assert_eq!(Verification::pour(true, "client@exemple.be"), Verification::CodeParEmail);
        assert_eq!(Verification::pour(false, "client@exemple.be"), Verification::ModeTest);
        assert_eq!(Verification::pour(true, ""), Verification::SansEmail);
        assert_eq!(Verification::pour(false, "x"), Verification::SansEmail);
    }

    #[test]
    fn le_navigateur_est_garde_tel_quel() {
        // L'adresse IP, elle, vient de `securite::Ip` (testée là).
        let mut h = HeaderMap::new();
        h.insert(header::USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (test)"));
        assert_eq!(navigateur(&h), "Mozilla/5.0 (test)");
    }

    #[test]
    fn le_texte_accepte_dit_ce_qui_est_vraiment_fait() {
        let t = texte_accord("le devis n° 2026-61", "19/10/2026", "23 304,95 €", "Ubique SRL", true, Verification::CodeParEmail);
        assert!(t.contains("les conditions générales de Ubique SRL, et je l'accepte"), "{t}");
        assert!(t.contains("mon adresse e-mail vérifiée") && t.contains("Un exemplaire signé me sera envoyé"), "{t}");
        let t = texte_accord("le devis n° 2026-61", "19/10/2026", "23 304,95 €", "Ubique SRL", false, Verification::ModeTest);
        assert!(!t.contains("conditions générales") && t.contains("TVA comprise et je l'accepte"), "pas de conditions déposées : {t}");
        assert!(!t.contains("vérifiée") && t.contains("exemplaire signé"), "{t}");
        let t = texte_accord("l'avenant n° 2", "19/10/2026", "23 304,95 €", "Ubique SRL", true, Verification::SansEmail);
        assert!(!t.contains("exemplaire signé") && !t.contains("vérifiée"), "pas d'adresse : pas de promesse d'envoi : {t}");
    }
}
