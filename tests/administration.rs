// Tests des étapes 4-3 et 4-4 : le back office de Raven (connexion séparée, catalogue, publication, organisations)
// et les mises à jour du catalogue chez les organisations (« nouveaux ouvrages disponibles »).

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
use ubifacture::{AppState, administration, application, base::Base, connexion, organisations};

struct Reponse {
    statut: StatusCode,
    lieu: Option<String>,
    cookie: Option<String>,
    corps: String,
}

async fn envoyer(site: &Router, methode: &str, chemin: &str, corps: &str, cookie: &str) -> Reponse {
    let mut r = Request::builder().method(methode).uri(chemin).header(header::COOKIE, cookie);
    if methode == "POST" {
        r = r.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let reponse = site.clone().oneshot(r.body(Body::from(corps.to_string())).unwrap()).await.unwrap();
    let statut = reponse.status();
    let lieu = reponse.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let cookie = reponse.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().to_string());
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    Reponse { statut, lieu, cookie, corps: String::from_utf8_lossy(&octets).to_string() }
}

async fn lire(site: &Router, chemin: &str, cookie: &str) -> Reponse {
    envoyer(site, "GET", chemin, "", cookie).await
}

async fn poster(site: &Router, chemin: &str, corps: &str, cookie: &str) -> Reponse {
    envoyer(site, "POST", chemin, corps, cookie).await
}

/// Le cookie (nom=valeur) d'une réponse de connexion.
fn valeur(cookie: &Option<String>) -> String {
    cookie.as_deref().unwrap().split(';').next().unwrap().to_string()
}

/// Une nouvelle organisation, son propriétaire connecté et son assistant de démarrage fait avec ce métier : (n°, cookie).
async fn nouvelle_organisation(pool: &PgPool, site: &Router, nom: &str, email: &str, metier: &str) -> (i64, String) {
    let id = organisations::creer(pool, nom).await.unwrap();
    connexion::ajouter_utilisateur(pool, id, email, "", "un-long-mot-de-passe").await.unwrap();
    let r = poster(site, "/connexion", &format!("email={}&mot_de_passe=un-long-mot-de-passe", email.replace('@', "%40")), "").await;
    let cookie = valeur(&r.cookie);
    let demarrage = format!(
        "nom={}&pays=BE&email={}&telephone=0&numero_tva=BE0123456749&iban=BE68+5390+0754+7034&regime_tva=assujetti&profil_metier={metier}",
        nom.replace(' ', "+"),
        email.replace('@', "%40")
    );
    assert_eq!(poster(site, "/demarrage", &demarrage, &cookie).await.lieu.as_deref(), Some("/"));
    (id, cookie)
}

async fn connexion_admin(site: &Router) -> String {
    let r = poster(site, "/admin/connexion", "email=mahx%40raven.example&mot_de_passe=un-mot-de-passe-solide", "").await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/catalogue"));
    valeur(&r.cookie)
}

#[sqlx::test]
async fn le_back_office_a_sa_propre_connexion(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });

    // Sans session du back office : la page de connexion, rien d'autre.
    for chemin in ["/admin", "/admin/catalogue", "/admin/organisations", "/admin/catalogue/peinture.peinture"] {
        let r = lire(&s, chemin, "").await;
        assert_eq!((r.statut, r.lieu.as_deref()), (StatusCode::SEE_OTHER, Some("/admin/connexion")), "{chemin}");
    }
    assert_eq!(poster(&s, "/admin/catalogue/publier", "", "").await.lieu.as_deref(), Some("/admin/connexion"));
    let r = lire(&s, "/admin/connexion", "").await;
    assert!(r.statut == StatusCode::OK && r.corps.contains("Back office de Raven"));

    // Le propriétaire d'une organisation n'y entre pas avec sa session.
    let (_, cookie_b) = nouvelle_organisation(&pool, &s, "Peintre B", "b@exemple.be", "peinture").await;
    for chemin in ["/admin/catalogue", "/admin/organisations"] {
        assert_eq!(lire(&s, chemin, &cookie_b).await.lieu.as_deref(), Some("/admin/connexion"), "{chemin}");
    }
    // … ni n'y écrit : rien n'est créé dans le catalogue commun.
    let avant: i64 = sqlx::query_scalar("SELECT count(*) FROM catalogue_bibliotheques").fetch_one(&pool).await.unwrap();
    for (chemin, corps) in [
        ("/admin/catalogue/bibliotheques", "metier=peinture&nom=Intrus"),
        ("/admin/catalogue/peinture.peinture/ouvrages", "description=Intrus&genre=ouvrage&unite=m"),
        ("/admin/catalogue/peinture.peinture/retirer", ""),
    ] {
        assert_eq!(poster(&s, chemin, corps, &cookie_b).await.lieu.as_deref(), Some("/admin/connexion"), "{chemin}");
    }
    let apres: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM catalogue_bibliotheques), (SELECT count(*) FROM catalogue_ouvrages WHERE description = 'Intrus'),
                (SELECT count(*) FROM catalogue_bibliotheques WHERE retiree_le IS NOT NULL)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(apres, (avant, 0, 0));
    // Un compte d'organisation n'est pas un administrateur.
    let r = poster(&s, "/admin/connexion", "email=b%40exemple.be&mot_de_passe=un-long-mot-de-passe", "").await;
    assert_eq!(r.statut, StatusCode::UNAUTHORIZED);

    // Comptes créés en ligne de commande seulement.
    assert!(administration::ajouter_administrateur(&pool, "mahx@raven.example", "Mahx", "court").await.is_err());
    administration::ajouter_administrateur(&pool, "mahx@raven.example", "Mahx", "un-mot-de-passe-solide").await.unwrap();
    assert!(administration::ajouter_administrateur(&pool, "MAHX@raven.example", "", "un-mot-de-passe-solide").await.is_err(), "doublon");
    let r = poster(&s, "/admin/connexion", "email=mahx%40raven.example&mot_de_passe=mauvais-mot-de-passe", "").await;
    assert!(r.statut == StatusCode::UNAUTHORIZED && r.corps.contains("E-mail ou mot de passe incorrect."));

    let r = poster(&s, "/admin/connexion", "email=mahx%40raven.example&mot_de_passe=un-mot-de-passe-solide", "").await;
    let brut = r.cookie.clone().unwrap();
    assert!(brut.starts_with("raven_admin=") && brut.contains("Path=/admin") && brut.contains("HttpOnly") && brut.contains("SameSite=Strict"));
    let admin = valeur(&r.cookie);
    assert!(lire(&s, "/admin/catalogue", &admin).await.corps.contains("Catalogue commun"));
    // La session du back office n'ouvre aucune page d'organisation.
    assert_eq!(lire(&s, "/devis", &admin).await.lieu.as_deref(), Some("/connexion?suite=/devis"));
    assert_eq!(lire(&s, "/", &admin).await.lieu.as_deref(), Some("/connexion"));

    // La liste des organisations : nom, métier, utilisateurs ; jamais leurs données.
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    sqlx::query("UPDATE devis SET reference = 'Référence secrète de B'").execute(&mut *b).await.unwrap();
    let r = poster(&s, "/devis", "", &cookie_b).await;
    let devis: i64 = r.lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    sqlx::query("UPDATE devis SET reference = 'Référence secrète de B' WHERE id = $1").bind(devis).execute(&mut *b).await.unwrap();
    let r = lire(&s, "/admin/organisations", &admin).await;
    assert!(r.corps.contains("Peintre B") && r.corps.contains("Peinture et décoration"));
    assert!(!r.corps.contains("Référence secrète de B") && !r.corps.contains("Peinture des murs"));

    // Se déconnecter ferme la session.
    let r = poster(&s, "/admin/deconnexion", "", &admin).await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/connexion"));
    assert_eq!(lire(&s, "/admin/catalogue", &admin).await.lieu.as_deref(), Some("/admin/connexion"));

    // 5 essais ratés : un quart d'heure d'attente, même avec le bon mot de passe.
    for _ in 0..5 {
        poster(&s, "/admin/connexion", "email=mahx%40raven.example&mot_de_passe=faux-faux-faux", "").await;
    }
    let r = poster(&s, "/admin/connexion", "email=mahx%40raven.example&mot_de_passe=un-mot-de-passe-solide", "").await;
    assert!(r.statut == StatusCode::UNAUTHORIZED && r.corps.contains("Trop d&#39;essais ratés"));
}

#[sqlx::test]
async fn publier_une_mise_a_jour_du_catalogue(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    administration::ajouter_administrateur(&pool, "mahx@raven.example", "Mahx", "un-mot-de-passe-solide").await.unwrap();
    let admin = connexion_admin(&s).await;
    let (b, cookie_b) = nouvelle_organisation(&pool, &s, "Peintre B", "b@exemple.be", "peinture").await;
    let mut base_b = Base::ouvrir(&pool, b).await.unwrap();

    // B met « Laque » à la corbeille et garde sa copie de « Ponçage ».
    let laque: i64 = sqlx::query_scalar("SELECT id FROM ouvrages WHERE description = 'Laque'").fetch_one(&mut *base_b).await.unwrap();
    assert_eq!(poster(&s, &format!("/bibliotheque/{laque}/supprimer"), "", &cookie_b).await.lieu.as_deref(), Some("/bibliotheque?ok=supprime"));

    // Dans le back office : un ouvrage ajouté à « Peinture », une nouvelle bibliothèque avec un ouvrage, une correction,
    // un retrait.
    let r = poster(&s, "/admin/catalogue/peinture.peinture/ouvrages", "description=Pose+de+frise+d%C3%A9corative&genre=ouvrage&unite=m", &admin).await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/catalogue/peinture.peinture?ok=ajoute"));
    let r = poster(&s, "/admin/catalogue/bibliotheques", "metier=peinture&nom=D%C3%A9coration+murale", &admin).await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/catalogue/peinture.decoration_murale?ok=cree"));
    poster(&s, "/admin/catalogue/peinture.decoration_murale/ouvrages", "description=Fresque+murale&genre=main_oeuvre&unite=m%C2%B2", &admin).await;
    let r = poster(&s, "/admin/catalogue/peinture.decoration_murale/ouvrages", "description=&genre=ouvrage&unite=m", &admin).await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/catalogue/peinture.decoration_murale?erreur=description"));
    let poncage: i64 = sqlx::query_scalar("SELECT id FROM catalogue_ouvrages WHERE description = 'Ponçage'").fetch_one(&pool).await.unwrap();
    let r = poster(&s, &format!("/admin/catalogue/ouvrages/{poncage}"), "description=Pon%C3%A7age+et+d%C3%A9poussi%C3%A9rage&genre=ouvrage&unite=m%C2%B2", &admin).await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/catalogue/peinture.peinture?ok=modifie"));
    let detapissage: i64 = sqlx::query_scalar("SELECT id FROM catalogue_ouvrages WHERE description = 'Détapissage'").fetch_one(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/admin/catalogue/ouvrages/{detapissage}/retirer"), "", &admin).await.lieu.as_deref(), Some("/admin/catalogue/peinture.peinture?ok=retire"));
    let r = lire(&s, "/admin/catalogue", &admin).await;
    assert!(r.corps.contains("3\u{a0}ajouts en attente") && r.corps.contains("Décoration murale"));

    // Avant la publication, les organisations ne voient rien de nouveau.
    assert!(!lire(&s, "/bibliotheque", &cookie_b).await.corps.contains("nouvel ouvrage"));
    let r = lire(&s, "/parametres/bibliotheques", &cookie_b).await;
    assert!(!r.corps.contains("Décoration murale") && !r.corps.contains("Ajouter les nouveaux ouvrages"));
    // La correction ne change pas la copie de B.
    assert_eq!(nombre(&mut base_b, "Ponçage").await, 1);

    let r = poster(&s, "/admin/catalogue/publier", "", &admin).await;
    assert_eq!(r.lieu.as_deref(), Some("/admin/catalogue?ok=publie&ouvrages=2&bibliotheques=1"));
    let r = lire(&s, r.lieu.as_deref().unwrap(), &admin).await;
    assert!(r.corps.contains("Mise à jour publiée") && r.corps.contains("mahx@raven.example") && r.corps.contains("Rien en attente"));
    assert_eq!(poster(&s, "/admin/catalogue/publier", "", &admin).await.lieu.as_deref(), Some("/admin/catalogue?ok=publie&ouvrages=0&bibliotheques=0"));

    // B voit le nouvel ouvrage de sa bibliothèque cochée, et la nouvelle bibliothèque à cocher.
    let r = lire(&s, "/bibliotheque", &cookie_b).await;
    assert!(r.corps.contains("1 nouvel ouvrage est disponible dans vos bibliothèques de Raven"), "bandeau de la bibliothèque");
    let r = lire(&s, "/parametres/bibliotheques", &cookie_b).await;
    assert!(r.corps.contains("1 nouveau") && r.corps.contains("Ajouter les nouveaux ouvrages") && r.corps.contains("Décoration murale"));
    let r = poster(&s, "/parametres/bibliotheques/nouveautes", "", &cookie_b).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/bibliotheques?nouveaux=1"));
    assert!(lire(&s, r.lieu.as_deref().unwrap(), &cookie_b).await.corps.contains("1 nouvel ouvrage ajouté à votre bibliothèque"));
    let (prix, corbeille): (Option<i64>, bool) =
        sqlx::query_as("SELECT prix_vente_e5, supprime_le IS NOT NULL FROM ouvrages WHERE description = 'Pose de frise décorative'").fetch_one(&mut *base_b).await.unwrap();
    assert_eq!((prix, corbeille), (None, false), "sans prix");
    // La corbeille est respectée, rien n'est en double, l'ouvrage retiré du catalogue reste chez B.
    let laque_dans_la_corbeille: bool =
        sqlx::query_scalar("SELECT supprime_le IS NOT NULL FROM ouvrages WHERE id = $1").bind(laque).fetch_one(&mut *base_b).await.unwrap();
    assert!(laque_dans_la_corbeille);
    assert_eq!(nombre(&mut base_b, "Laque").await, 1);
    assert_eq!(nombre(&mut base_b, "Détapissage").await, 1);
    assert_eq!(poster(&s, "/parametres/bibliotheques/nouveautes", "", &cookie_b).await.lieu.as_deref(), Some("/parametres/bibliotheques?nouveaux=0"));
    assert!(!lire(&s, "/bibliotheque", &cookie_b).await.corps.contains("nouvel ouvrage"));

    // Une nouvelle organisation de peinture reçoit le catalogue à jour : la correction, sans l'ouvrage retiré.
    let (c, _) = nouvelle_organisation(&pool, &s, "Peintre C", "c@exemple.be", "peinture").await;
    let mut base_c = Base::ouvrir(&pool, c).await.unwrap();
    assert_eq!(nombre(&mut base_c, "Ponçage et dépoussiérage").await, 1);
    assert_eq!(nombre(&mut base_c, "Détapissage").await, 0);
    assert_eq!(nombre(&mut base_c, "Pose de frise décorative").await, 1);
    assert_eq!(nombre(&mut base_c, "Fresque murale").await, 1, "un nouveau peintre reçoit aussi la nouvelle bibliothèque de son métier");

    // Retirer une bibliothèque : plus proposée, mais B, qui l'a cochée, la garde et peut la décocher.
    poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture&bibliotheque=peinture.decoration_murale", &cookie_b).await;
    assert_eq!(nombre(&mut base_b, "Fresque murale").await, 1);
    assert_eq!(poster(&s, "/admin/catalogue/peinture.decoration_murale/retirer", "", &admin).await.lieu.as_deref(), Some("/admin/catalogue/peinture.decoration_murale?ok=retire"));
    assert!(lire(&s, "/parametres/bibliotheques", &cookie_b).await.corps.contains("retirée du catalogue de Raven"));
    // Une organisation qui arrive après le retrait ne la reçoit pas et ne la voit pas.
    let (d, cookie_d) = nouvelle_organisation(&pool, &s, "Peintre D", "d@exemple.be", "peinture").await;
    let mut base_d = Base::ouvrir(&pool, d).await.unwrap();
    assert_eq!(nombre(&mut base_d, "Fresque murale").await, 0);
    assert!(!lire(&s, "/parametres/bibliotheques", &cookie_d).await.corps.contains("Décoration murale"), "plus proposée");
    // Une demande trafiquée ne coche pas une bibliothèque retirée.
    poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture", &cookie_b).await;
    poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture&bibliotheque=peinture.decoration_murale", &cookie_b).await;
    let cochees: Vec<String> = sqlx::query_scalar("SELECT bibliotheque FROM organisation_bibliotheques ORDER BY 1").fetch_all(&mut *base_b).await.unwrap();
    assert_eq!(cochees, ["peinture.peinture"]);

    // Un ajout pas encore publié s'efface ; un ouvrage retiré se rétablit.
    poster(&s, "/admin/catalogue/peinture.peinture/ouvrages", "description=Essai&genre=ouvrage&unite=m", &admin).await;
    let essai: i64 = sqlx::query_scalar("SELECT id FROM catalogue_ouvrages WHERE description = 'Essai'").fetch_one(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/admin/catalogue/ouvrages/{essai}/retirer"), "", &admin).await.lieu.as_deref(), Some("/admin/catalogue/peinture.peinture?ok=efface"));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalogue_ouvrages WHERE description = 'Essai'").fetch_one(&pool).await.unwrap(), 0);
    assert_eq!(poster(&s, &format!("/admin/catalogue/ouvrages/{detapissage}/retablir"), "", &admin).await.lieu.as_deref(), Some("/admin/catalogue/peinture.peinture?ok=retabli"));
    assert_eq!(lire(&s, "/admin/catalogue/inconnue", &admin).await.statut, StatusCode::NOT_FOUND);
    assert_eq!(lire(&s, "/admin/catalogue/ouvrages/999999", &admin).await.statut, StatusCode::NOT_FOUND);
}

/// Le nombre d'ouvrages de l'organisation qui portent cette description (corbeille comprise).
async fn nombre(base: &mut Base, description: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ouvrages WHERE description = $1").bind(description).fetch_one(&mut **base).await.unwrap()
}
