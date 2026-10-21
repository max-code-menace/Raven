// Tests de l'équipe (étape 2) : Réglages › Équipe, invitations, rôles, retrait, limite de 8 personnes.

mod commun;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, application, connexion, emails_systeme};

struct Reponse {
    statut: StatusCode,
    lieu: Option<String>,
    cookie: Option<String>,
    corps: String,
}

async fn envoyer(site: &Router, methode: &str, chemin: &str, corps: &str, cookie: Option<&str>) -> Reponse {
    let mut r = Request::builder().method(methode).uri(chemin);
    if methode == "POST" {
        r = r.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    if let Some(c) = cookie {
        r = r.header(header::COOKIE, c);
    }
    let reponse = site.clone().oneshot(r.body(Body::from(corps.to_string())).unwrap()).await.unwrap();
    let statut = reponse.status();
    let lieu = reponse.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let cookie = reponse.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    Reponse { statut, lieu, cookie, corps: String::from_utf8_lossy(&octets).to_string() }
}

async fn lire(site: &Router, chemin: &str, cookie: &str) -> Reponse {
    envoyer(site, "GET", chemin, "", Some(cookie)).await
}

async fn poster(site: &Router, chemin: &str, corps: &str, cookie: Option<&str>) -> Reponse {
    envoyer(site, "POST", chemin, corps, cookie).await
}

const MDP: &str = "un-long-mot-de-passe";

async fn connecter(site: &Router, email: &str) -> String {
    let corps = format!("email={}&mot_de_passe={MDP}", email.replace('@', "%40"));
    poster(site, "/connexion", &corps, None).await.cookie.unwrap_or_else(|| panic!("connexion de {email}"))
}

fn lien_invitation(email: &str) -> String {
    let (_, texte) = emails_systeme::boite_de_test(email).pop().expect("invitation reçue");
    let debut = texte.find("http://127.0.0.1:3001/invitation/").unwrap() + "http://127.0.0.1:3001".len();
    texte[debut..].split_whitespace().next().unwrap().to_string()
}

async fn id_de(pool: &PgPool, email: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM utilisateurs WHERE lower(email) = lower($1)").bind(email).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn inviter_accepter_et_gerer_l_equipe(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    connexion::ajouter_utilisateur(&pool, 1, "patron@equipe.be", "Patron", MDP).await.unwrap();
    let s = application(AppState { db: pool.clone() });
    let patron = connecter(&s, "patron@equipe.be").await;

    let r = lire(&s, "/parametres/equipe", &patron).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("patron@equipe.be") && r.corps.contains("(1 / 8)") && r.corps.contains("Inviter quelqu"));
    assert!(lire(&s, "/parametres/societe", &patron).await.corps.contains("href=\"/parametres/equipe\">Équipe"), "Réglages › Équipe");

    // Inviter : un e-mail part avec un lien, l'invitation attend.
    let r = poster(&s, "/parametres/equipe/inviter", "email=Ouvrier%40equipe.be&role=membre", Some(&patron)).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/equipe?ok=invitation"));
    let r = lire(&s, "/parametres/equipe", &patron).await;
    assert!(r.corps.contains("Ouvrier@equipe.be") && r.corps.contains("Invitations en attente") && r.corps.contains("(2 / 8)"));
    let lien = lien_invitation("ouvrier@equipe.be");
    let (objet, _) = emails_systeme::boite_de_test("ouvrier@equipe.be").pop().unwrap();
    assert!(objet.contains("Invitation à rejoindre"));
    assert_eq!(poster(&s, "/parametres/equipe/inviter", "email=patron%40equipe.be", Some(&patron)).await.lieu.as_deref(), Some("/parametres/equipe?erreur=membre"));
    assert_eq!(poster(&s, "/parametres/equipe/inviter", "email=pas-un-email", Some(&patron)).await.lieu.as_deref(), Some("/parametres/equipe?erreur=email"));

    // Accepter (sans être connecté) : nom et mot de passe, puis connecté dans l'équipe.
    let r = envoyer(&s, "GET", &lien, "", None).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Rejoindre") && r.corps.contains("Ouvrier@equipe.be"));
    let r = poster(&s, &lien, "nom=Ouvrier&mot_de_passe=court&confirmation=court", None).await;
    assert!(r.statut == StatusCode::UNPROCESSABLE_ENTITY && r.corps.contains("10 caractères"));
    let r = poster(&s, &lien, &format!("nom=Paul+Ouvrier&mot_de_passe={MDP}&confirmation={MDP}"), None).await;
    assert_eq!(r.lieu.as_deref(), Some("/"));
    let ouvrier = r.cookie.expect("connecté");
    assert_eq!(lire(&s, "/factures", &ouvrier).await.statut, StatusCode::OK);
    let (role, verifie): (String, bool) = sqlx::query_as(
        "SELECT m.role::text, u.email_verifie_le IS NOT NULL FROM membres m JOIN utilisateurs u ON u.id = m.utilisateur_id WHERE u.email = 'Ouvrier@equipe.be'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((role.as_str(), verifie), ("membre", true));
    assert_eq!(poster(&s, &lien, &format!("nom=X&mot_de_passe={MDP}&confirmation={MDP}"), None).await.statut, StatusCode::GONE, "une seule fois");

    // Un membre voit l'équipe mais ne la gère pas.
    let r = lire(&s, "/parametres/equipe", &ouvrier).await;
    assert!(r.corps.contains("Paul Ouvrier") && !r.corps.contains("Inviter quelqu") && !r.corps.contains("/retirer"));
    let id_patron = id_de(&pool, "patron@equipe.be").await;
    let id_ouvrier = id_de(&pool, "ouvrier@equipe.be").await;
    for (chemin, corps) in [
        ("/parametres/equipe/inviter".to_string(), "email=intrus%40equipe.be"),
        (format!("/parametres/equipe/membres/{id_patron}/role"), "role=membre"),
        (format!("/parametres/equipe/membres/{id_patron}/retirer"), ""),
    ] {
        assert_eq!(poster(&s, &chemin, corps, Some(&ouvrier)).await.lieu.as_deref(), Some("/parametres/equipe?erreur=droits"), "{chemin}");
    }
    assert!(emails_systeme::boite_de_test("intrus@equipe.be").is_empty());

    // Rôles : toujours au moins un propriétaire.
    let r = poster(&s, &format!("/parametres/equipe/membres/{id_patron}/role"), "role=membre", Some(&patron)).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/equipe?erreur=dernier"));
    assert_eq!(poster(&s, &format!("/parametres/equipe/membres/{id_ouvrier}/role"), "role=proprietaire", Some(&patron)).await.lieu.as_deref(), Some("/parametres/equipe?ok=role"));
    assert_eq!(poster(&s, &format!("/parametres/equipe/membres/{id_patron}/role"), "role=membre", Some(&patron)).await.lieu.as_deref(), Some("/parametres/equipe?ok=role"));
    assert_eq!(poster(&s, &format!("/parametres/equipe/membres/{id_patron}/role"), "role=proprietaire", Some(&ouvrier)).await.lieu.as_deref(), Some("/parametres/equipe?ok=role"));

    // Retirer : jamais soi-même ; le compte retiré disparaît et sa session se ferme.
    assert_eq!(poster(&s, &format!("/parametres/equipe/membres/{id_patron}/retirer"), "", Some(&patron)).await.lieu.as_deref(), Some("/parametres/equipe?erreur=toi"));
    assert_eq!(poster(&s, &format!("/parametres/equipe/membres/{id_ouvrier}/retirer"), "", Some(&patron)).await.lieu.as_deref(), Some("/parametres/equipe?ok=retire"));
    assert_eq!(lire(&s, "/factures", &ouvrier).await.lieu.as_deref(), Some("/connexion?suite=/factures"));
    let reste: i64 = sqlx::query_scalar("SELECT count(*) FROM utilisateurs").fetch_one(&pool).await.unwrap();
    assert_eq!(reste, 1);
    assert_eq!(poster(&s, &format!("/parametres/equipe/membres/{id_ouvrier}/retirer"), "", Some(&patron)).await.statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn huit_personnes_au_plus_et_invitations_annulees(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    connexion::ajouter_utilisateur(&pool, 1, "chef@huit.be", "", MDP).await.unwrap();
    connexion::ajouter_utilisateur(&pool, 1, "ailleurs@huit.be", "", MDP).await.unwrap();
    sqlx::query("DELETE FROM membres WHERE utilisateur_id = (SELECT id FROM utilisateurs WHERE email = 'ailleurs@huit.be')").execute(&pool).await.unwrap();
    let s = application(AppState { db: pool.clone() });
    let chef = connecter(&s, "chef@huit.be").await;

    // Une adresse qui a déjà un compte (ailleurs) ne s'invite pas.
    assert_eq!(poster(&s, "/parametres/equipe/inviter", "email=ailleurs%40huit.be", Some(&chef)).await.lieu.as_deref(), Some("/parametres/equipe?erreur=compte"));
    for i in 1..=7 {
        let r = poster(&s, "/parametres/equipe/inviter", &format!("email=p{i}%40huit.be"), Some(&chef)).await;
        assert_eq!(r.lieu.as_deref(), Some("/parametres/equipe?ok=invitation"), "invitation {i}");
    }
    assert_eq!(poster(&s, "/parametres/equipe/inviter", "email=p8%40huit.be", Some(&chef)).await.lieu.as_deref(), Some("/parametres/equipe?erreur=limite"));
    // Réinviter la même adresse remplace son invitation (le premier lien ne marche plus).
    let premier = lien_invitation("p1@huit.be");
    assert_eq!(poster(&s, "/parametres/equipe/inviter", "email=p1%40huit.be", Some(&chef)).await.lieu.as_deref(), Some("/parametres/equipe?ok=invitation"));
    assert_eq!(envoyer(&s, "GET", &premier, "", None).await.statut, StatusCode::GONE);
    // Annuler une invitation libère une place.
    let invitation: i64 = sqlx::query_scalar("SELECT id FROM invitations WHERE email = 'p2@huit.be'").fetch_one(&pool).await.unwrap();
    let lien_p2 = lien_invitation("p2@huit.be");
    assert_eq!(poster(&s, &format!("/parametres/equipe/invitations/{invitation}/annuler"), "", Some(&chef)).await.lieu.as_deref(), Some("/parametres/equipe?ok=annulee"));
    assert_eq!(envoyer(&s, "GET", &lien_p2, "", None).await.statut, StatusCode::GONE);
    assert_eq!(poster(&s, "/parametres/equipe/inviter", "email=p8%40huit.be", Some(&chef)).await.lieu.as_deref(), Some("/parametres/equipe?ok=invitation"));
    // Une invitation de plus de 7 jours ne marche plus.
    let lien_p3 = lien_invitation("p3@huit.be");
    sqlx::query("UPDATE invitations SET expire_le = now() - interval '1 second' WHERE email = 'p3@huit.be'").execute(&pool).await.unwrap();
    assert_eq!(envoyer(&s, "GET", &lien_p3, "", None).await.statut, StatusCode::GONE);
    assert_eq!(poster(&s, &lien_p3, &format!("nom=P3&mot_de_passe={MDP}&confirmation={MDP}"), None).await.statut, StatusCode::GONE);
}
