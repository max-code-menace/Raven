//! Les e-mails du site lui-même (pas ceux d'une organisation) : vérification de l'adresse à l'inscription, nouveau mot
//! de passe, invitation dans une équipe.
//!
//! - Ils partent de l'adresse du site (`EMAIL_EXPEDITEUR`) par Brevo, comme les autres e-mails.
//! - **Sans clé Brevo** (pendant les tests, et chez Mahx tant qu'il essaie), rien ne part : l'e-mail est écrit dans
//!   la console du serveur (on peut y copier le lien) et gardé dans une boîte en mémoire que les tests lisent
//!   (`boite_de_test`). Avec une clé mais sans `EMAIL_EXPEDITEUR`, l'envoi échoue sans écrire le lien nulle part.
//! - Le journal `emails_systeme` garde le destinataire, l'objet et le résultat, **jamais le texte** (il contient un
//!   lien secret). Il sert aussi à limiter les envois répétés vers une même adresse (`envois_recents`).

use std::sync::Mutex;

use sqlx::PgPool;

use crate::emails;

/// Le nom affiché comme expéditeur.
pub const NOM_DU_SITE: &str = "Ubifacture";

/// Les e-mails gardés en mémoire quand rien ne part : (destinataire, objet, texte).
static BOITE: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
const BOITE_MAX: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Genre {
    Verification,
    DejaInscrit,
    MotDePasse,
    Invitation,
}

impl Genre {
    fn code(self) -> &'static str {
        match self {
            Genre::Verification => "verification",
            Genre::DejaInscrit => "deja_inscrit",
            Genre::MotDePasse => "mot_de_passe",
            Genre::Invitation => "invitation",
        }
    }
}

/// Envoie un e-mail du site (ou le garde seulement, sans configuration) et le note dans le journal.
pub async fn envoyer(db: &PgPool, genre: Genre, destinataire: &str, objet: &str, texte: &str) -> Result<(), sqlx::Error> {
    let (statut, erreur) = match emails::config_site() {
        (Some(cle), Some(expediteur)) => {
            let corps = serde_json::json!({
                "sender": { "name": NOM_DU_SITE, "email": expediteur },
                "to": [{ "email": destinataire }],
                "subject": objet,
                "textContent": texte,
            });
            match emails::poster_brevo(cle, corps).await {
                Ok(_) => ("envoye", String::new()),
                Err(e) => {
                    eprintln!("E-mail du site non envoyé à {destinataire} : {e}");
                    ("echec", e)
                }
            }
        }
        // Une clé Brevo sans expéditeur : c'est un oubli de configuration en production. Le lien secret n'est écrit nulle part.
        (Some(_), None) => {
            eprintln!("E-mail du site non envoyé à {destinataire} : EMAIL_EXPEDITEUR manque (ou n'est pas une adresse valable).");
            ("echec", "EMAIL_EXPEDITEUR manquant".to_string())
        }
        (None, _) => {
            println!("E-mail du site NON ENVOYÉ (mode test, pas de BREVO_CLE_API), à {destinataire} : {objet}\n{texte}\n");
            let mut boite = BOITE.lock().unwrap_or_else(|e| e.into_inner());
            if boite.len() >= BOITE_MAX {
                boite.remove(0);
            }
            boite.push((destinataire.to_string(), objet.to_string(), texte.to_string()));
            ("enregistre", String::new())
        }
    };
    sqlx::query("INSERT INTO emails_systeme (genre, destinataire, objet, statut, erreur) VALUES ($1, $2, $3, $4, $5)")
        .bind(genre.code())
        .bind(destinataire)
        .bind(objet)
        .bind(statut)
        .bind(erreur)
        .execute(db)
        .await?;
    Ok(())
}

/// Le nombre d'e-mails de ce genre envoyés à cette adresse dans la dernière heure.
pub async fn envois_recents(db: &PgPool, genre: Genre, destinataire: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM emails_systeme WHERE lower(destinataire) = lower($1) AND genre = $2 AND created_at > now() - interval '1 hour'",
    )
    .bind(destinataire)
    .bind(genre.code())
    .fetch_one(db)
    .await
}

/// Les e-mails gardés en mémoire pour cette adresse (objet, texte), du plus ancien au plus récent. Pour les tests.
pub fn boite_de_test(destinataire: &str) -> Vec<(String, String)> {
    let boite = BOITE.lock().unwrap_or_else(|e| e.into_inner());
    boite.iter().filter(|(d, _, _)| d.eq_ignore_ascii_case(destinataire)).map(|(_, o, t)| (o.clone(), t.clone())).collect()
}
