// Tests de l'étape 4-1 : les bibliothèques de Raven (catalogue commun sans prix, cocher / décocher, copie dans
// l'organisation, ouvrages modifiés gardés, « Vider ma bibliothèque », types de ligne des métiers « Projet »).

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
use ubifacture::{AppState, application, base::Base, connexion, organisations};

struct Reponse {
    statut: StatusCode,
    lieu: Option<String>,
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
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    Reponse { statut, lieu, corps: String::from_utf8_lossy(&octets).to_string() }
}

async fn lire(site: &Router, chemin: &str, cookie: &str) -> Reponse {
    envoyer(site, "GET", chemin, "", cookie).await
}

async fn poster(site: &Router, chemin: &str, corps: &str, cookie: &str) -> Reponse {
    envoyer(site, "POST", chemin, corps, cookie).await
}

async fn se_connecter(site: &Router, email: &str) -> String {
    let requete = Request::builder()
        .method("POST")
        .uri("/connexion")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("email={}&mot_de_passe=un-long-mot-de-passe", email.replace('@', "%40"))))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
}

/// Une nouvelle organisation, son propriétaire connecté et son assistant de démarrage fait avec ce métier : (n°, cookie).
async fn nouvelle_organisation(pool: &PgPool, site: &Router, nom: &str, email: &str, metier: &str) -> (i64, String) {
    let id = organisations::creer(pool, nom).await.unwrap();
    connexion::ajouter_utilisateur(pool, id, email, "", "un-long-mot-de-passe").await.unwrap();
    let cookie = se_connecter(site, email).await;
    let demarrage = format!(
        "nom=Atelier&pays=BE&email={}&telephone=0&numero_tva=BE0123456749&iban=BE68+5390+0754+7034&regime_tva=assujetti&profil_metier={metier}",
        email.replace('@', "%40")
    );
    assert_eq!(poster(site, "/demarrage", &demarrage, cookie.as_str()).await.lieu.as_deref(), Some("/"));
    (id, cookie)
}

/// (bibliothèques cochées, nombre d'ouvrages) de l'organisation.
async fn etat(pool: &PgPool, organisation: i64) -> (Vec<String>, i64) {
    let mut b = Base::ouvrir(pool, organisation).await.unwrap();
    let cochees = sqlx::query_scalar("SELECT bibliotheque FROM organisation_bibliotheques ORDER BY 1").fetch_all(&mut *b).await.unwrap();
    let nombre = sqlx::query_scalar("SELECT count(*) FROM ouvrages").fetch_one(&mut *b).await.unwrap();
    (cochees, nombre)
}

#[sqlx::test]
async fn le_catalogue_de_depart(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let par_bibliotheque: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT b.metier, b.code, count(c.id) FROM catalogue_bibliotheques b LEFT JOIN catalogue_ouvrages c ON c.bibliotheque = b.code
         GROUP BY b.metier, b.code, b.position ORDER BY b.position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(par_bibliotheque.len(), 12, "7 bibliothèques du bâtiment, 3 des espaces verts, peinture, graphisme");
    assert_eq!(par_bibliotheque.iter().filter(|(m, _, _)| m == "batiment").count(), 7);
    assert!(par_bibliotheque.iter().all(|(_, _, n)| (17..=36).contains(n)), "{par_bibliotheque:?}");
    assert_eq!(par_bibliotheque.iter().map(|(_, _, n)| n).sum::<i64>(), 300);
    // Chaque ouvrage a une unité ; le graphisme parle de prestations, sans vocabulaire de chantier.
    let sans_unite: i64 = sqlx::query_scalar("SELECT count(*) FROM catalogue_ouvrages WHERE unite = ''").fetch_one(&pool).await.unwrap();
    assert_eq!(sans_unite, 0);
    let chantier: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalogue_ouvrages WHERE bibliotheque = 'graphisme.graphisme' AND (description ILIKE '%chantier%' OR genre = 'ouvrage')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(chantier, 0);
    // L'organisation n° 1 (Ubique, déjà en service) n'a rien de coché d'office.
    assert_eq!(etat(&pool, 1).await, (Vec::new(), 0));
}

#[sqlx::test]
async fn cocher_modifier_decocher_recocher(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    // Un peintre : l'assistant de démarrage coche la bibliothèque « Peinture ».
    let (b, cookie) = nouvelle_organisation(&pool, &s, "Peintre", "b@exemple.be", "peinture").await;
    assert_eq!(etat(&pool, b).await, (vec!["peinture.peinture".to_string()], 26));

    // Les ouvrages copiés sont sans prix : « À compléter », jamais 0 €.
    let r = lire(&s, "/bibliotheque", &cookie).await;
    assert_eq!(r.statut, StatusCode::OK);
    assert!(r.corps.contains("Peinture des murs, deux couches") && r.corps.contains("À compléter") && !r.corps.contains("0,00"));
    assert!(r.corps.contains("Prestation") && !r.corps.contains("Main d&#39;œuvre"), "type de ligne d'un métier « Projet »");
    let mut base = Base::ouvrir(&pool, b).await.unwrap();
    let (categorie, prix): (String, Option<i64>) =
        sqlx::query_as("SELECT categorie, prix_vente_e5 FROM ouvrages WHERE description = 'Laque'").fetch_one(&mut *base).await.unwrap();
    assert_eq!((categorie.as_str(), prix), ("Peinture", None));

    // La page des bibliothèques : le métier de l'organisation ouvert et coché.
    let r = lire(&s, "/parametres/bibliotheques", &cookie).await;
    assert_eq!(r.statut, StatusCode::OK);
    assert!(r.corps.contains("Gyproc et plafonnage") && r.corps.contains("value=\"peinture.peinture\" checked") && r.corps.contains(">Enregistrer<"));

    // Il ajoute « Gyproc » (un autre métier) : 26 ouvrages en plus.
    let r = poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture&bibliotheque=batiment.gyproc", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/bibliotheques?ajoutes=26&retires=0&gardes=0"));
    assert!(lire(&s, r.lieu.as_deref().unwrap(), &cookie).await.corps.contains("26 ouvrages ajoutés à votre bibliothèque"));
    assert_eq!(etat(&pool, b).await.1, 52);

    // Il met son prix sur un ouvrage de gyproc et en ouvre un autre sans rien changer.
    let (plaque, trappe): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT id FROM ouvrages WHERE description = 'Plaque coupe-feu'), (SELECT id FROM ouvrages WHERE description = 'Trappe de visite')",
    )
    .fetch_one(&mut *base)
    .await
    .unwrap();
    let r = poster(&s, &format!("/bibliotheque/{plaque}"), "description=Plaque+coupe-feu&genre=ouvrage&categorie=Gyproc+et+plafonnage&unite=m%C2%B2&prix_vente=38", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/bibliotheque?ok=modifie"));
    let r = poster(&s, &format!("/bibliotheque/{trappe}"), "description=Trappe+de+visite&genre=ouvrage&categorie=Gyproc+et+plafonnage&unite=pi%C3%A8ce", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/bibliotheque?ok=modifie"));
    let personnalises: Vec<String> =
        sqlx::query_scalar("SELECT description FROM ouvrages WHERE personnalise ORDER BY 1").fetch_all(&mut *base).await.unwrap();
    assert_eq!(personnalises, ["Plaque coupe-feu"], "enregistrer sans rien changer ne compte pas comme une modification");

    // Il décoche « Gyproc » : 25 ouvrages retirés, celui qu'il a modifié est gardé.
    let r = poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/bibliotheques?ajoutes=0&retires=25&gardes=1"));
    assert!(lire(&s, r.lieu.as_deref().unwrap(), &cookie).await.corps.contains("25 retirés · 1 gardé (vous l&#39;aviez modifié)"));
    assert_eq!(etat(&pool, b).await, (vec!["peinture.peinture".to_string()], 27));
    let prix: Option<i64> = sqlx::query_scalar("SELECT prix_vente_e5 FROM ouvrages WHERE id = $1").bind(plaque).fetch_one(&mut *base).await.unwrap();
    assert_eq!(prix, Some(3_800_000));

    // Il le recoche : pas de doublon, son ouvrage modifié reste tel quel.
    let r = poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture&bibliotheque=batiment.gyproc", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/bibliotheques?ajoutes=25&retires=0&gardes=0"));
    assert_eq!(etat(&pool, b).await.1, 52);

    // L'organisation n° 1 n'a rien reçu ; une autre organisation ne voit rien de B.
    assert_eq!(etat(&pool, 1).await, (Vec::new(), 0));
    let (c, cookie_c) = nouvelle_organisation(&pool, &s, "Jardinier", "c@exemple.be", "espaces_verts").await;
    let (cochees, nombre) = etat(&pool, c).await;
    assert_eq!(cochees, ["espaces_verts.amenagement", "espaces_verts.elagage", "espaces_verts.entretien"]);
    assert_eq!(nombre, 19 + 25 + 23);
    let r = lire(&s, "/bibliotheque?q=coupe", &cookie_c).await;
    assert!(!r.corps.contains("Plaque coupe-feu"));
    assert_eq!(lire(&s, &format!("/bibliotheque/{plaque}"), &cookie_c).await.statut, StatusCode::NOT_FOUND);
    assert!(lire(&s, "/parametres/bibliotheques", &cookie_c).await.corps.contains("value=\"batiment.gyproc\">"), "rien de coché chez C");
}

#[sqlx::test]
async fn un_membre_ne_change_rien_et_le_proprietaire_vide_tout(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let (b, cookie) = nouvelle_organisation(&pool, &s, "Bâtisseur", "b@exemple.be", "batiment").await;
    let (cochees, nombre) = etat(&pool, b).await;
    assert_eq!((cochees.len(), nombre), (7, 36 + 26 + 25 + 23 + 26 + 25 + 17));

    connexion::ajouter_utilisateur(&pool, b, "membre@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    sqlx::query("UPDATE membres SET role = 'membre' WHERE utilisateur_id = (SELECT id FROM utilisateurs WHERE email = 'membre@exemple.be')")
        .execute(&pool)
        .await
        .unwrap();
    let membre = se_connecter(&s, "membre@exemple.be").await;
    let r = lire(&s, "/parametres/bibliotheques", &membre).await;
    assert!(r.corps.contains("disabled") && !r.corps.contains(">Enregistrer<"));
    let r = poster(&s, "/parametres/bibliotheques", "", &membre).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/bibliotheques?erreur=droits"));
    let r = poster(&s, "/bibliotheque/vider", "confirme=oui", &membre).await;
    assert_eq!(r.lieu.as_deref(), Some("/bibliotheque?erreur=droits"));
    assert!(lire(&s, "/bibliotheque?erreur=droits", &membre).await.corps.contains("Seul un propriétaire peut vider la bibliothèque."));
    assert_eq!(etat(&pool, b).await.1, nombre);

    // Sans la case « Je confirme », rien ne se passe ; avec elle, tout est vidé et décoché.
    assert!(lire(&s, "/bibliotheque", &cookie).await.corps.contains("Vider ma bibliothèque"));
    assert_eq!(poster(&s, "/bibliotheque/vider", "", &cookie).await.lieu.as_deref(), Some("/bibliotheque"));
    assert_eq!(etat(&pool, b).await.1, nombre);
    assert_eq!(poster(&s, "/bibliotheque/vider", "confirme=oui", &cookie).await.lieu.as_deref(), Some("/bibliotheque?ok=vide"));
    assert_eq!(etat(&pool, b).await, (Vec::new(), 0));
    let r = lire(&s, "/bibliotheque?ok=vide", &cookie).await;
    assert!(r.corps.contains("Votre bibliothèque est vide") && !r.corps.contains("Vider ma bibliothèque"));
}

#[sqlx::test]
async fn un_ouvrage_sans_prix_dans_un_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let (b, cookie) = nouvelle_organisation(&pool, &s, "Graphiste", "b@exemple.be", "graphisme").await;
    let r = poster(&s, "/devis", "", &cookie).await;
    let devis: i64 = r.lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    let mut base = Base::ouvrir(&pool, b).await.unwrap();
    let logo: i64 = sqlx::query_scalar("SELECT id FROM ouvrages WHERE description = 'Création de logo'").fetch_one(&mut *base).await.unwrap();
    sqlx::query("UPDATE devis SET taux_tva_id = (SELECT id FROM taux_tva WHERE taux_centiemes = 2100) WHERE id = $1")
        .bind(devis)
        .execute(&mut *base)
        .await
        .unwrap();
    let r = poster(&s, &format!("/devis/{devis}/bibliotheque"), &format!("ouvrage={logo}"), &cookie).await;
    assert_eq!(r.statut, StatusCode::OK);
    assert!(r.corps.contains("pas encore de prix dans votre bibliothèque"), "le devis signale l'ouvrage sans prix : {}", &r.corps[..r.corps.len().min(3000)]);
    assert!(r.corps.contains("Prestation"), "le menu des types parle de prestation");
    let (genre, prix): (String, Option<i64>) =
        sqlx::query_as("SELECT genre::text, prix_unitaire_e5 FROM devis_lignes WHERE devis_id = $1").bind(devis).fetch_one(&mut *base).await.unwrap();
    assert_eq!((genre.as_str(), prix), ("main_oeuvre", Some(0)));
}

#[sqlx::test]
async fn la_corbeille_et_les_filtres(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let (b, cookie) = nouvelle_organisation(&pool, &s, "Peintre", "b@exemple.be", "peinture").await;
    let mut base = Base::ouvrir(&pool, b).await.unwrap();
    // Un ouvrage à lui, avec son prix.
    let r = poster(&s, "/bibliotheque", "description=Ma+peinture+sp%C3%A9ciale&genre=ouvrage&unite=m%C2%B2&prix_vente=25", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/bibliotheque?ok=cree"));

    let r = lire(&s, "/bibliotheque", &cookie).await;
    assert!(r.corps.contains("Mes ouvrages</span><small>1</small>") && r.corps.contains("Prix à compléter</span><small>26</small>"));
    assert!(r.corps.contains("Supprimés</span><small>0</small>"));
    let r = lire(&s, "/bibliotheque?vue=miens", &cookie).await;
    assert!(r.corps.contains("Ma peinture spéciale") && !r.corps.contains("Laque") && r.corps.contains("1 ouvrage(s)"));
    let r = lire(&s, "/bibliotheque?vue=a_completer", &cookie).await;
    assert!(r.corps.contains("Laque") && !r.corps.contains("Ma peinture spéciale") && r.corps.contains("26 ouvrage(s)"));

    // Supprimer : à la corbeille, plus proposé ni modifiable, mais récupérable.
    let laque: i64 = sqlx::query_scalar("SELECT id FROM ouvrages WHERE description = 'Laque'").fetch_one(&mut *base).await.unwrap();
    let r = poster(&s, &format!("/bibliotheque/{laque}/supprimer"), "", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/bibliotheque?ok=supprime"));
    assert!(lire(&s, "/bibliotheque?ok=supprime", &cookie).await.corps.contains("dans la corbeille"));
    assert!(!lire(&s, "/bibliotheque?q=Laque", &cookie).await.corps.contains(">Laque<"));
    let r = lire(&s, "/bibliotheque?vue=supprimes", &cookie).await;
    assert!(r.corps.contains("Laque") && r.corps.contains(&format!("/bibliotheque/{laque}/recuperer")) && !r.corps.contains(&format!("href=\"/bibliotheque/{laque}\"")));
    assert_eq!(lire(&s, &format!("/bibliotheque/{laque}"), &cookie).await.statut, StatusCode::NOT_FOUND);
    assert_eq!(poster(&s, &format!("/bibliotheque/{laque}"), "description=Laque&genre=ouvrage", &cookie).await.statut, StatusCode::NOT_FOUND);
    assert_eq!(poster(&s, &format!("/bibliotheque/{laque}/supprimer"), "", &cookie).await.statut, StatusCode::NOT_FOUND);
    let r = poster(&s, "/devis", "", &cookie).await;
    let devis: i64 = r.lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    assert!(!lire(&s, &format!("/bibliotheque/choisir?devis={devis}&q=Laque"), &cookie).await.corps.contains("Laque"));
    sqlx::query("UPDATE devis SET taux_tva_id = (SELECT id FROM taux_tva WHERE taux_centiemes = 2100) WHERE id = $1").bind(devis).execute(&mut *base).await.unwrap();
    let r = poster(&s, &format!("/devis/{devis}/bibliotheque"), &format!("ouvrage={laque}"), &cookie).await;
    assert!(r.corps.contains("Cet ouvrage n&#39;existe plus dans la bibliothèque."));

    // Décocher la bibliothèque : l'ouvrage de la corbeille y reste ; recocher ne le ramène pas en double.
    poster(&s, "/parametres/bibliotheques", "", &cookie).await;
    let (dans_la_corbeille, ailleurs): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE supprime_le IS NOT NULL), count(*) FILTER (WHERE supprime_le IS NULL) FROM ouvrages",
    )
    .fetch_one(&mut *base)
    .await
    .unwrap();
    assert_eq!((dans_la_corbeille, ailleurs), (1, 1), "Laque dans la corbeille, et son propre ouvrage");
    poster(&s, "/parametres/bibliotheques", "bibliotheque=peinture.peinture", &cookie).await;
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ouvrages WHERE description = 'Laque'").fetch_one(&mut *base).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ouvrages WHERE supprime_le IS NULL").fetch_one(&mut *base).await.unwrap(), 26);

    // Récupérer.
    let r = poster(&s, &format!("/bibliotheque/{laque}/recuperer"), "", &cookie).await;
    assert_eq!(r.lieu.as_deref(), Some("/bibliotheque?ok=recupere"));
    assert!(lire(&s, "/bibliotheque?vue=supprimes", &cookie).await.corps.contains("La corbeille est vide."));
    assert_eq!(lire(&s, &format!("/bibliotheque/{laque}"), &cookie).await.statut, StatusCode::OK);
    assert_eq!(poster(&s, &format!("/bibliotheque/{laque}/recuperer"), "", &cookie).await.statut, StatusCode::NOT_FOUND);

    // « Vider ma bibliothèque » vide aussi la corbeille.
    poster(&s, &format!("/bibliotheque/{laque}/supprimer"), "", &cookie).await;
    poster(&s, "/bibliotheque/vider", "confirme=oui", &cookie).await;
    assert_eq!(etat(&pool, b).await, (Vec::new(), 0));
}
