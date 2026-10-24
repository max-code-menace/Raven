// Dossier de preuve de la signature en ligne d'un devis : empreinte et copie du PDF signé, texte accepté, adresse e-mail
// (vérifiée ou non), IP, navigateur, journal ; exemplaire signé envoyé au client ; dossier non modifiable ;
// codes à usage unique. Aucune clé d'envoi pendant les tests : les e-mails sont seulement enregistrés.

mod commun;

use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, app, archive, signature};

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    (statut, lieu, octets)
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let (s, _, o) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, o)
}

/// `entetes` : des en-têtes, et « connexion » pour l'adresse qui ouvre la connexion (comme le fait `main.rs`).
async fn poster(site: &Router, uri: &str, corps: &str, entetes: &[(&str, &str)]) -> (StatusCode, Option<String>) {
    let mut requete = Request::builder().method("POST").uri(uri).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    for (nom, valeur) in entetes {
        requete = match *nom {
            "connexion" => requete.extension(ConnectInfo(valeur.parse::<SocketAddr>().unwrap())),
            _ => requete.header(*nom, *valeur),
        };
    }
    let (s, lieu, _) = envoyer(site, requete.body(Body::from(corps.to_string())).unwrap()).await;
    (s, lieu)
}

async fn societe(pool: &PgPool) {
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone, numero_tva) VALUES ('Ubique SRL', 'Ubique', 'info@ubique.be', '010 00 00 00', 'BE0726407561')")
        .execute(pool)
        .await
        .unwrap();
}

/// Un devis envoyé de 1 060 € TTC dont le client a (ou non) une adresse e-mail ; renvoie (id, chemin de la page client).
async fn devis_envoye(pool: &PgPool, s: &Router, email: &str) -> (i64, String) {
    let (_, lieu, _) = envoyer(s, Request::builder().method("POST").uri("/devis").body(Body::empty()).unwrap()).await;
    let id: i64 = lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils, email) VALUES ('Claes SPRL', '{client}', NULLIF($1, '')) RETURNING id")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE devis SET reference = 'PL46 Salle de bain', montant_ht_cents = 100000, montant_ttc_cents = 106000, client_entreprise_id = $2 WHERE id = $1")
        .bind(id)
        .bind(client)
        .execute(pool)
        .await
        .unwrap();
    poster(s, &format!("/devis/{id}/statut/envoye"), "", &[]).await;
    let (_, apercu) = lire(s, &format!("/devis/{id}/apercu")).await;
    let apercu = String::from_utf8_lossy(&apercu).to_string();
    let debut = apercu.find("http://127.0.0.1:3001/client/devis/").expect("lien de la page client");
    let reste = &apercu[debut + "http://127.0.0.1:3001".len()..];
    (id, reste[..reste.find('"').unwrap()].to_string())
}

#[sqlx::test]
async fn la_signature_cree_un_dossier_de_preuve_complet(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let (id, chemin) = devis_envoye(&pool, &s, "anne@claes.be").await;

    // La page affiche la phrase d'acceptation complète, celle qui sera gardée.
    let (_, page) = lire(&s, &chemin).await;
    let page = String::from_utf8_lossy(&page).to_string();
    assert!(page.contains("je reconnais avoir lu le devis n°") && page.contains("l&#39;empreinte numérique du document") || page.contains("l'empreinte numérique du document"), "{page}");

    // Sans serveur frontal (tests), l'adresse est celle de la connexion ; X-Forwarded-For, que le client peut écrire
    // lui-même, est ignoré (`securite::ip_de`).
    let entetes = [("connexion", "203.0.113.7:51000"), ("x-forwarded-for", "198.51.100.66"), ("user-agent", "Mozilla/5.0 (essai)")];
    let (statut, lieu) = poster(&s, &chemin, "nom=Anne+Claes&accord=oui", &entetes).await;
    assert!(statut.is_redirection());
    assert_eq!(lieu, Some(format!("{chemin}?ok=signe")));

    let (nom, email, verification, ip, navigateur, texte, montant, pdf_nom, empreinte): (String, String, String, String, String, String, i64, String, String) =
        sqlx::query_as("SELECT nom, email, verification, ip, navigateur, texte_accepte, montant_ttc_cents, pdf_nom, empreinte_pdf FROM devis_signatures WHERE devis_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((nom.as_str(), email.as_str(), montant), ("Anne Claes", "anne@claes.be", 106_000));
    assert_eq!(verification, "mode_test", "sans clé d'envoi, l'adresse n'est pas vérifiée et le dossier le dit");
    assert_eq!((ip.as_str(), navigateur.as_str()), ("203.0.113.7", "Mozilla/5.0 (essai)"));
    assert!(texte.contains("devis n°") && texte.contains("1\u{a0}060,00\u{a0}€ TVA comprise") && texte.contains("adresse IP"), "{texte}");
    assert!(!texte.contains("conditions générales"), "aucune condition générale déposée : le client n'en accepte pas : {texte}");
    assert!(!texte.contains("vérifiée"), "le texte ne promet pas une vérification qui n'a pas eu lieu : {texte}");
    assert!(pdf_nom.starts_with("Devis-") && pdf_nom.ends_with(".pdf"), "{pdf_nom}");

    // L'empreinte est bien celle du PDF gardé, et ce PDF se télécharge des deux côtés.
    let pdf: Vec<u8> = sqlx::query_scalar("SELECT pdf FROM devis_signatures WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(pdf.starts_with(b"%PDF-") && archive::empreinte(&pdf) == empreinte);
    let (statut, client) = lire(&s, &format!("{chemin}/signe")).await;
    assert_eq!((statut, client == pdf), (StatusCode::OK, true));
    let (statut, artisan) = lire(&s, &format!("/devis/{id}/signature/pdf")).await;
    assert_eq!((statut, artisan == pdf), (StatusCode::OK, true));

    // Le devis passe à « Signé » ; le journal et la page du client le disent.
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(statut, "signe");
    let evenement: String = sqlx::query_scalar("SELECT detail FROM devis_evenements WHERE devis_id = $1 AND detail LIKE 'Signé en ligne%'").bind(id).fetch_one(&pool).await.unwrap();
    assert!(evenement.contains("Anne Claes") && evenement.contains(&empreinte[..16]), "{evenement}");
    let (_, page) = lire(&s, &format!("{chemin}?ok=signe")).await;
    assert!(String::from_utf8_lossy(&page).contains("Télécharger l&#39;exemplaire signé") || String::from_utf8_lossy(&page).contains("Télécharger l'exemplaire signé"));

    // Un exemplaire signé part chez le client (enregistré seulement), avec le PDF et l'empreinte.
    let (modele, destinataires, corps, pieces): (String, String, String, Vec<String>) =
        sqlx::query_as("SELECT modele, destinataires, corps, pieces FROM emails WHERE devis_id = $1 AND modele = 'devis_signe'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((modele.as_str(), destinataires.as_str(), pieces.len()), ("devis_signe", "anne@claes.be", 1));
    assert!(corps.contains(&empreinte) && corps.contains("Anne Claes"), "{corps}");

    // L'artisan retrouve le dossier dans l'historique du devis.
    let (_, historique) = lire(&s, &format!("/devis/{id}/historique")).await;
    let historique = String::from_utf8_lossy(&historique).to_string();
    for attendu in ["Dossier de preuve de la signature", "E-mail non vérifié", "203.0.113.7", &empreinte, "Télécharger le PDF signé"] {
        assert!(historique.contains(attendu), "manque « {attendu} »");
    }
}

#[sqlx::test]
async fn sans_adresse_e_mail_la_signature_reste_possible_mais_le_dossier_le_dit(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let (id, chemin) = devis_envoye(&pool, &s, "").await;
    let (_, page) = lire(&s, &chemin).await;
    assert!(!String::from_utf8_lossy(&page).contains("exemplaire signé me sera envoyé"), "pas d'adresse : pas de promesse d'envoi");
    poster(&s, &chemin, "nom=Anne+Claes&accord=oui", &[]).await;
    let verification: String = sqlx::query_scalar("SELECT verification FROM devis_signatures WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(verification, "sans_email");
    let envoyes: i64 = sqlx::query_scalar("SELECT count(*) FROM emails WHERE devis_id = $1 AND modele = 'devis_signe'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(envoyes, 0);
    // Sans en-têtes : IP et navigateur vides, jamais inventés.
    let (ip, navigateur): (String, String) = sqlx::query_as("SELECT ip, navigateur FROM devis_signatures WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((ip.as_str(), navigateur.as_str()), ("", ""));
}

#[sqlx::test]
async fn le_dossier_ne_se_modifie_ni_ne_se_supprime_et_une_seule_signature_compte(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let (id, chemin) = devis_envoye(&pool, &s, "anne@claes.be").await;
    poster(&s, &chemin, "nom=Anne+Claes&accord=oui", &[]).await;
    // Une deuxième signature, sous un autre nom, ne change rien.
    let (_, lieu) = poster(&s, &chemin, "nom=Quelqu%27un+d%27autre&accord=oui", &[]).await;
    assert_eq!(lieu, Some(chemin.clone()));
    let (n, nom): (i64, String) = sqlx::query_as("SELECT count(*), max(nom) FROM devis_signatures WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((n, nom.as_str()), (1, "Anne Claes"));

    let modif = sqlx::query("UPDATE devis_signatures SET nom = 'X' WHERE devis_id = $1").bind(id).execute(&pool).await;
    assert!(modif.is_err(), "modifier le dossier est refusé par la base");
    let suppr = sqlx::query("DELETE FROM devis_signatures WHERE devis_id = $1").bind(id).execute(&pool).await;
    assert!(suppr.is_err(), "supprimer le dossier est refusé par la base");
}

#[sqlx::test]
async fn en_mode_test_la_demande_de_code_ne_change_rien(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let (id, chemin) = devis_envoye(&pool, &s, "anne@claes.be").await;
    // Sans clé d'envoi, aucun code n'est exigé ni envoyé : on revient simplement à la page.
    let (_, lieu) = poster(&s, &format!("{chemin}/code"), "nom=Anne+Claes&accord=oui", &[]).await;
    assert_eq!(lieu, Some(chemin.clone()));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_codes_signature WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
    // Les erreurs de saisie sont les mêmes que pour la signature.
    let (_, lieu) = poster(&s, &format!("{chemin}/code"), "nom=&accord=oui", &[]).await;
    assert_eq!(lieu, Some(format!("{chemin}?erreur=nom#signer")));
}

#[sqlx::test]
async fn les_codes_a_usage_unique(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let s = site(&pool);
    let (id, _) = devis_envoye(&pool, &s, "anne@claes.be").await;
    let email = "Anne@Claes.be";

    // Un code juste ne sert qu'une fois ; un faux code ne passe pas.
    let code = signature::emettre_code(&mut *pool.acquire().await.unwrap(), id, email).await.unwrap().unwrap();
    assert_eq!(code.len(), 6);
    assert!(!signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, email, "abcdef").await.unwrap());
    assert!(!signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, "autre@claes.be", &code).await.unwrap(), "le code est lié à l'adresse");
    assert!(signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, email, &format!(" {code} ")).await.unwrap(), "espaces autour du code tolérés, majuscules de l'adresse ignorées");
    assert!(!signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, email, &code).await.unwrap(), "un code juste est détruit après usage");

    // Cinq essais ratés : le code est brûlé, même s'il est ensuite saisi correctement.
    let code = signature::emettre_code(&mut *pool.acquire().await.unwrap(), id, email).await.unwrap().unwrap();
    for _ in 0..5 {
        assert!(!signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, email, "000000x").await.unwrap());
    }
    assert!(!signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, email, &code).await.unwrap());

    // Un code expiré ne passe pas.
    sqlx::query("UPDATE devis_codes_signature SET created_at = created_at - interval '1 hour'").execute(&pool).await.unwrap();
    let code = signature::emettre_code(&mut *pool.acquire().await.unwrap(), id, email).await.unwrap().unwrap();
    sqlx::query("UPDATE devis_codes_signature SET expire_le = now() - interval '1 minute' WHERE id = (SELECT max(id) FROM devis_codes_signature)").execute(&pool).await.unwrap();
    assert!(!signature::verifier_code(&mut *pool.acquire().await.unwrap(), id, email, &code).await.unwrap());

    // Trois codes par quart d'heure au plus.
    sqlx::query("DELETE FROM devis_codes_signature").execute(&pool).await.unwrap();
    for _ in 0..3 {
        assert!(signature::emettre_code(&mut *pool.acquire().await.unwrap(), id, email).await.unwrap().is_some());
    }
    assert!(signature::emettre_code(&mut *pool.acquire().await.unwrap(), id, email).await.unwrap().is_none(), "le quatrième est refusé");
    // La base ne garde que l'empreinte du code, jamais le code.
    let empreintes: Vec<String> = sqlx::query_scalar("SELECT empreinte_code FROM devis_codes_signature").fetch_all(&pool).await.unwrap();
    assert!(empreintes.iter().all(|e| e.len() == 64));
}
