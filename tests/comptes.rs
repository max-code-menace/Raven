// Tests des comptes en libre-service (étape 2) : inscription, confirmation de l'adresse, assistant de démarrage,
// mot de passe oublié. Aucun e-mail ne part pendant les tests : on lit la boîte de test (`emails_systeme`).

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
use ubifacture::{AppState, application, emails_systeme};

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

async fn lire(site: &Router, chemin: &str, cookie: Option<&str>) -> Reponse {
    envoyer(site, "GET", chemin, "", cookie).await
}

async fn poster(site: &Router, chemin: &str, corps: &str, cookie: Option<&str>) -> Reponse {
    envoyer(site, "POST", chemin, corps, cookie).await
}

/// Le chemin du dernier lien reçu à cette adresse (« /inscription/verifier/… »), et le nombre d'e-mails reçus.
fn dernier_lien(email: &str) -> (String, usize) {
    let boite = emails_systeme::boite_de_test(email);
    let (_, texte) = boite.last().expect("un e-mail reçu");
    let debut = texte.find("http://127.0.0.1:3001/").expect("un lien") + "http://127.0.0.1:3001".len();
    let lien = texte[debut..].split_whitespace().next().unwrap().to_string();
    (lien, boite.len())
}

async fn compte(pool: &PgPool, email: &str) -> Option<(i64, bool, i64, String)> {
    sqlx::query_as(
        "SELECT u.id, u.email_verifie_le IS NOT NULL, m.organisation_id, m.role::text
         FROM utilisateurs u JOIN membres m ON m.utilisateur_id = u.id WHERE lower(u.email) = lower($1)",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
    .unwrap()
}

const MDP: &str = "un-long-mot-de-passe";

#[sqlx::test]
async fn s_inscrire_confirmer_puis_demarrer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let r = lire(&s, "/inscription", None).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Créer un compte") && r.corps.contains("name=\"site\""));
    assert!(lire(&s, "/connexion", None).await.corps.contains("href=\"/inscription\""), "lien depuis la connexion");

    let r = poster(&s, "/inscription", &format!("entreprise=Peinture+Dupont&email=jean%40dupont.be&mot_de_passe={MDP}&confirmation={MDP}"), None).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Vérifie ta boîte e-mail") && r.cookie.is_none());
    let (id, verifie, organisation, role) = compte(&pool, "jean@dupont.be").await.expect("compte créé");
    assert!(!verifie && organisation == 2 && role == "proprietaire");
    let (nom, demarre): (String, bool) =
        sqlx::query_as("SELECT nom, demarre_le IS NOT NULL FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    assert_eq!((nom.as_str(), demarre), ("Peinture Dupont", false));
    let empreintes: i64 = sqlx::query_scalar("SELECT count(*) FROM jetons_compte WHERE utilisateur_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(empreintes, 1);
    let journal: (String, String) = sqlx::query_as("SELECT genre, statut FROM emails_systeme").fetch_one(&pool).await.unwrap();
    assert_eq!(journal, ("verification".to_string(), "enregistre".to_string()), "rien ne part pendant les tests");

    // Pas de connexion avant la confirmation (mais le bon mot de passe propose de renvoyer le lien).
    let r = poster(&s, "/connexion", &format!("email=jean%40dupont.be&mot_de_passe={MDP}"), None).await;
    assert!(r.statut == StatusCode::UNAUTHORIZED && r.cookie.is_none() && r.corps.contains("Confirme d") && r.corps.contains("/inscription/renvoyer"));
    let r = poster(&s, "/connexion", "email=jean%40dupont.be&mot_de_passe=mauvais-mot-de-passe", None).await;
    assert!(!r.corps.contains("Confirme d"), "rien n'est révélé sans le bon mot de passe");

    // Le lien de l'e-mail : la page n'utilise pas le lien, le bouton si.
    let (lien, _) = dernier_lien("jean@dupont.be");
    assert!(lien.starts_with("/inscription/verifier/"));
    let r = lire(&s, &lien, None).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Confirmer mon adresse"));
    assert!(!compte(&pool, "jean@dupont.be").await.unwrap().1, "ouvrir la page ne confirme rien");
    let r = poster(&s, &lien, "", None).await;
    assert_eq!(r.lieu.as_deref(), Some("/"));
    let cookie = r.cookie.expect("connecté après la confirmation");
    assert!(compte(&pool, "jean@dupont.be").await.unwrap().1);
    assert_eq!(poster(&s, &lien, "", None).await.statut, StatusCode::GONE, "un lien ne sert qu'une fois");
    assert_eq!(lire(&s, &lien, None).await.statut, StatusCode::GONE);

    // L'assistant de démarrage : toutes les pages y mènent tant qu'il n'est pas fait.
    assert_eq!(lire(&s, "/", Some(&cookie)).await.lieu.as_deref(), Some("/demarrage"));
    assert_eq!(lire(&s, "/factures/3", Some(&cookie)).await.lieu.as_deref(), Some("/demarrage"));
    let r = lire(&s, "/demarrage", Some(&cookie)).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("value=\"Peinture Dupont\"") && r.corps.contains("value=\"jean@dupont.be\""));
    let incomplet = "nom=Peinture+Dupont&pays=BE&email=jean%40dupont.be&telephone=&regime_tva=franchise&profil_metier=peinture&derniere_facture=douze";
    let r = poster(&s, "/demarrage", incomplet, Some(&cookie)).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.corps.contains("téléphone est obligatoire") && r.corps.contains("Dernière facture"));
    let complet = "nom=Peinture+Dupont+SRL&rue=Rue+Haute&numero=3&code_postal=1300&ville=Wavre&pays=BE&email=jean%40dupont.be&telephone=010+11+22+33\
                   &numero_tva=BE0123456749&regime_tva=franchise&iban=BE68+5390+0754+7034&bic=&profil_metier=peinture\
                   &dernier_devis=12&derniere_facture=41&derniere_note_credit=2";
    let r = poster(&s, "/demarrage", complet, Some(&cookie)).await;
    assert_eq!(r.lieu.as_deref(), Some("/"), "{}", r.corps);
    let r = lire(&s, "/", Some(&cookie)).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Tableau de bord"));
    assert_eq!(lire(&s, "/demarrage", Some(&cookie)).await.lieu.as_deref(), Some("/"), "une seule fois");

    let (nom, profil): (String, String) =
        sqlx::query_as("SELECT nom, profil_metier FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    assert_eq!((nom.as_str(), profil.as_str()), ("Peinture Dupont SRL", "peinture"));
    let mut b = ubifacture::base::Base::ouvrir(&pool, 2).await.unwrap();
    let (societe, regime, tva): (String, String, Option<String>) =
        sqlx::query_as("SELECT nom, regime_tva, numero_tva FROM societe").fetch_one(&mut *b).await.unwrap();
    assert_eq!((societe.as_str(), regime.as_str(), tva.is_some()), ("Peinture Dupont SRL", "franchise", true));
    let iban: String = sqlx::query_scalar("SELECT iban FROM societe_comptes_bancaires WHERE par_defaut").fetch_one(&mut *b).await.unwrap();
    assert_eq!(iban, "BE68 5390 0754 7034");
    let compteurs: Vec<(String, i64, Option<i32>)> =
        sqlx::query_as("SELECT nom, valeur, annee FROM compteurs ORDER BY nom").fetch_all(&mut *b).await.unwrap();
    let annee: i32 = sqlx::query_scalar("SELECT EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(&mut *b).await.unwrap();
    assert_eq!(
        compteurs,
        vec![
            ("chantier".to_string(), 0, None),
            ("devis".to_string(), 12, None),
            ("facture".to_string(), 41, Some(annee)),
            ("note_credit".to_string(), 2, Some(annee)),
        ],
        "la prochaine facture sera la n° 42"
    );
    // Et l'organisation n° 1 n'a rien reçu.
    let societe_a: i64 = sqlx::query_scalar("SELECT count(*) FROM societe").fetch_one(&pool).await.unwrap();
    assert_eq!(societe_a, 0);
}

#[sqlx::test]
async fn une_inscription_incomplete_ne_cree_rien(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let r = poster(&s, "/inscription", "entreprise=&email=pas-un-email&mot_de_passe=court&confirmation=autre", None).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.corps.contains("nom de ton entreprise") && r.corps.contains("pas valable") && r.corps.contains("10 caractères"));
    let r = poster(&s, "/inscription", &format!("entreprise=X&email=x%40x.be&mot_de_passe={MDP}&confirmation=un-autre-mot-de-passe"), None).await;
    assert!(r.corps.contains("pas identiques"));
    // Un robot remplit le champ invisible : même réponse, rien n'est créé.
    let r = poster(&s, "/inscription", &format!("entreprise=Robot&email=robot%40x.be&mot_de_passe={MDP}&confirmation={MDP}&site=http%3A%2F%2Fspam"), None).await;
    assert!(r.corps.contains("Vérifie ta boîte e-mail"));
    let organisations: i64 = sqlx::query_scalar("SELECT count(*) FROM organisations").fetch_one(&pool).await.unwrap();
    let comptes: i64 = sqlx::query_scalar("SELECT count(*) FROM utilisateurs").fetch_one(&pool).await.unwrap();
    assert_eq!((organisations, comptes), (1, 0));
    assert!(emails_systeme::boite_de_test("robot@x.be").is_empty());
}

#[sqlx::test]
async fn une_adresse_deja_inscrite_ne_se_devine_pas(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    ubifacture::connexion::ajouter_utilisateur(&pool, 1, "deja@exemple.be", "", MDP).await.unwrap();
    let s = application(AppState { db: pool.clone() });
    let corps = format!("entreprise=Autre&email=DEJA%40exemple.be&mot_de_passe={MDP}&confirmation={MDP}");
    let r = poster(&s, "/inscription", &corps, None).await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Vérifie ta boîte e-mail"), "la même réponse qu'une nouvelle adresse");
    let organisations: i64 = sqlx::query_scalar("SELECT count(*) FROM organisations").fetch_one(&pool).await.unwrap();
    assert_eq!(organisations, 1, "pas de deuxième compte");
    let (objet, texte) = emails_systeme::boite_de_test("deja@exemple.be").pop().unwrap();
    assert!(objet.contains("déjà un compte") && texte.contains("/mot-de-passe-oublie"));
    // 3 e-mails au plus par heure vers une même adresse.
    for _ in 0..4 {
        poster(&s, "/inscription", &corps, None).await;
    }
    assert_eq!(emails_systeme::boite_de_test("deja@exemple.be").len(), 3);
}

#[sqlx::test]
async fn renvoyer_le_lien_de_confirmation(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    poster(&s, "/inscription", &format!("entreprise=Jardins+Martin&email=lea%40martin.be&mot_de_passe={MDP}&confirmation={MDP}"), None).await;
    for _ in 0..4 {
        let r = poster(&s, "/inscription/renvoyer", "email=lea%40martin.be", None).await;
        assert!(r.corps.contains("Vérifie ta boîte e-mail"));
    }
    let (lien, recus) = dernier_lien("lea@martin.be");
    assert_eq!(recus, 3, "le premier e-mail et deux renvois : 3 par heure au plus");
    assert!(poster(&s, &lien, "", None).await.cookie.is_some());
    // Une adresse inconnue ou déjà confirmée : même réponse, rien ne part.
    poster(&s, "/inscription/renvoyer", "email=lea%40martin.be", None).await;
    poster(&s, "/inscription/renvoyer", "email=inconnu%40martin.be", None).await;
    assert_eq!(dernier_lien("lea@martin.be").1, 3);
    assert!(emails_systeme::boite_de_test("inconnu@martin.be").is_empty());
}

#[sqlx::test]
async fn mot_de_passe_oublie(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    ubifacture::connexion::ajouter_utilisateur(&pool, 1, "max@exemple.be", "", MDP).await.unwrap();
    let s = application(AppState { db: pool.clone() });
    let ancienne = poster(&s, "/connexion", &format!("email=max%40exemple.be&mot_de_passe={MDP}"), None).await.cookie.unwrap();
    assert!(lire(&s, "/connexion", None).await.corps.contains("href=\"/mot-de-passe-oublie\""));
    assert!(lire(&s, "/mot-de-passe-oublie", None).await.corps.contains("Recevoir le lien"));

    let inconnu = poster(&s, "/mot-de-passe-oublie", "email=personne%40exemple.be", None).await;
    let connu = poster(&s, "/mot-de-passe-oublie", "email=MAX%40exemple.be", None).await;
    assert!(inconnu.corps.contains("Si un compte existe") && connu.corps.contains("Si un compte existe"), "même réponse");
    assert!(emails_systeme::boite_de_test("personne@exemple.be").is_empty());
    let (lien, _) = dernier_lien("max@exemple.be");
    assert!(lien.starts_with("/mot-de-passe/"));
    assert!(lire(&s, &lien, None).await.corps.contains("Nouveau mot de passe"));

    let r = poster(&s, &lien, "mot_de_passe=nouveau-mot-de-passe&confirmation=autre-chose-encore", None).await;
    assert!(r.statut == StatusCode::UNPROCESSABLE_ENTITY && r.corps.contains("pas identiques"));
    let r = poster(&s, &lien, "mot_de_passe=nouveau-mot-de-passe&confirmation=nouveau-mot-de-passe", None).await;
    assert_eq!(r.lieu.as_deref(), Some("/connexion?ok=mot_de_passe"));
    assert!(lire(&s, "/connexion?ok=mot_de_passe", None).await.corps.contains("Ton mot de passe est changé"));
    assert_eq!(lire(&s, "/", Some(&ancienne)).await.lieu.as_deref(), Some("/connexion"), "les sessions ouvertes sont fermées");
    assert_eq!(poster(&s, "/connexion", &format!("email=max%40exemple.be&mot_de_passe={MDP}"), None).await.statut, StatusCode::UNAUTHORIZED);
    assert!(poster(&s, "/connexion", "email=max%40exemple.be&mot_de_passe=nouveau-mot-de-passe", None).await.cookie.is_some());
    assert_eq!(poster(&s, &lien, "mot_de_passe=encore-un-autre-mdp&confirmation=encore-un-autre-mdp", None).await.statut, StatusCode::GONE);

    // Un lien de plus d'une heure ne marche plus.
    poster(&s, "/mot-de-passe-oublie", "email=max%40exemple.be", None).await;
    let (lien, _) = dernier_lien("max@exemple.be");
    sqlx::query("UPDATE jetons_compte SET expire_le = now() - interval '1 second'").execute(&pool).await.unwrap();
    assert_eq!(lire(&s, &lien, None).await.statut, StatusCode::GONE);
    assert_eq!(poster(&s, &lien, "mot_de_passe=encore-un-autre-mdp&confirmation=encore-un-autre-mdp", None).await.statut, StatusCode::GONE);
    // Un lien de confirmation d'adresse ne change pas un mot de passe (et l'inverse).
    let verification = lien.replace("/mot-de-passe/", "/inscription/verifier/");
    assert_eq!(poster(&s, &verification, "", None).await.statut, StatusCode::GONE);
}
