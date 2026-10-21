// Tests de l'étape 3 : métier (« Chantier » ou « Projet »), modules allumés ou éteints, franchise de TVA, taux de 12 %.

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
use ubifacture::{AppState, app, application, base::Base, connexion, organisations};

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

/// L'organisation n° 2, son propriétaire connecté et son assistant de démarrage fait (métier et régime de TVA donnés) :
/// le cookie.
async fn nouvelle_organisation(pool: &PgPool, site: &Router, metier: &str, regime: &str) -> String {
    let id = organisations::creer(pool, "Organisation B").await.unwrap();
    assert_eq!(id, 2);
    connexion::ajouter_utilisateur(pool, id, "b@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    let requete = Request::builder()
        .method("POST")
        .uri("/connexion")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("email=b%40exemple.be&mot_de_passe=un-long-mot-de-passe"))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    let cookie = r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let demarrage = format!(
        "nom=Atelier+B&pays=BE&email=b%40exemple.be&telephone=0&numero_tva=BE0123456749&iban=BE68+5390+0754+7034&regime_tva={regime}&profil_metier={metier}"
    );
    assert_eq!(poster(site, "/demarrage", &demarrage, &cookie).await.lieu.as_deref(), Some("/"));
    cookie
}

#[sqlx::test]
async fn un_autre_metier_parle_de_projets_et_eteint_les_modules(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = nouvelle_organisation(&pool, &s, "graphisme", "assujetti").await;
    let (vocabulaire, avancements, avenants, achats, import): (String, bool, bool, bool, bool) = sqlx::query_as(
        "SELECT vocabulaire, module_avancements, module_avenants, module_achats, module_import_metres FROM organisations WHERE id = 2",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((vocabulaire.as_str(), avancements, avenants, achats, import), ("projet", false, false, false, false));

    for chemin in ["/", "/chantiers", "/factures", "/devis"] {
        let r = lire(&s, chemin, &cookie).await;
        assert_eq!(r.statut, StatusCode::OK, "{chemin}");
        assert!(r.corps.contains("Projets"), "{chemin} : le menu dit « Projets »");
        assert!(!r.corps.contains("Chantier"), "{chemin} : jamais « Chantier » à l'écran");
        assert!(!r.corps.contains("Factures fournisseurs") && !r.corps.contains("Depuis Excel"), "{chemin} : modules éteints");
    }
    assert!(lire(&s, "/chantiers", &cookie).await.corps.contains("Aucun projet trouvé"));
    assert!(!lire(&s, "/factures/rappels", &cookie).await.corps.contains("href=\"/achats/recues\""), "Paiements : pas d'onglet « À payer »");
    // Les adresses des modules éteints répondent « module éteint », même tapées à la main.
    for (chemin, methode) in [("/achats", "GET"), ("/chantiers/1/avancements", "POST"), ("/chantiers/1/avenants", "POST"), ("/devis/import", "POST")] {
        let r = envoyer(&s, methode, chemin, "", &cookie).await;
        assert!(r.statut == StatusCode::CONFLICT && r.corps.contains("est éteint"), "{methode} {chemin}");
    }

    // Le propriétaire rallume des modules et choisit « Chantier ».
    let r = lire(&s, "/parametres/metier", &cookie).await;
    assert!(r.corps.contains("Métier et modules") && r.corps.contains("Import de métrés Excel"));
    let corps = "profil_metier=graphisme&vocabulaire=chantier&module_avancements=on&module_achats=on";
    assert_eq!(poster(&s, "/parametres/metier", corps, &cookie).await.lieu.as_deref(), Some("/parametres/metier?ok=1"));
    let r = lire(&s, "/chantiers", &cookie).await;
    assert!(r.corps.contains("Chantiers") && !r.corps.contains("Depuis Excel"));
    assert!(lire(&s, "/factures/rappels", &cookie).await.corps.contains("href=\"/achats/recues\">À payer"), "Paiements › À payer");
    assert_eq!(lire(&s, "/achats", &cookie).await.statut, StatusCode::OK);
    assert_eq!(envoyer(&s, "POST", "/chantiers/1/avenants", "", &cookie).await.statut, StatusCode::CONFLICT, "avenants toujours éteints");

    // Un membre voit les réglages mais ne les change pas.
    connexion::ajouter_utilisateur(&pool, 2, "membre@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    sqlx::query("UPDATE membres SET role = 'membre' WHERE utilisateur_id = (SELECT id FROM utilisateurs WHERE email = 'membre@exemple.be')")
        .execute(&pool)
        .await
        .unwrap();
    let requete = Request::builder()
        .method("POST")
        .uri("/connexion")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("email=membre%40exemple.be&mot_de_passe=un-long-mot-de-passe"))
        .unwrap();
    let r = s.clone().oneshot(requete).await.unwrap();
    let membre = r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    assert!(!lire(&s, "/parametres/metier", &membre).await.corps.contains(">Enregistrer<"));
    let r = poster(&s, "/parametres/metier", "profil_metier=autre&vocabulaire=projet", &membre).await;
    assert_eq!(r.lieu.as_deref(), Some("/parametres/metier?erreur=droits"));
    let vocabulaire: String = sqlx::query_scalar("SELECT vocabulaire FROM organisations WHERE id = 2").fetch_one(&pool).await.unwrap();
    assert_eq!(vocabulaire, "chantier");
}

#[sqlx::test]
async fn le_batiment_garde_tout_et_ubique_garde_l_import_excel(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = nouvelle_organisation(&pool, &s, "batiment", "assujetti").await;
    let r = lire(&s, "/chantiers", &cookie).await;
    assert!(r.corps.contains("Chantiers"));
    assert!(lire(&s, "/factures/rappels", &cookie).await.corps.contains("href=\"/achats/recues\">À payer"), "Paiements › À payer");
    assert!(!lire(&s, "/devis", &cookie).await.corps.contains("Depuis Excel"), "l'import du modèle de métré est éteint par défaut");
    // L'organisation n° 1 (Ubique, déjà en service) garde l'import.
    let r = app(AppState { db: pool.clone() }).oneshot(Request::builder().uri("/devis").body(Body::empty()).unwrap()).await.unwrap();
    let corps = String::from_utf8_lossy(&r.into_body().collect().await.unwrap().to_bytes()).to_string();
    assert!(corps.contains("Depuis Excel"));
}

/// Une facture simple de l'organisation au client donné, avec une ligne au taux donné (id) : son numéro.
async fn facture(site: &Router, cookie: &str, pool: &PgPool, organisation: i64, taux: i64) -> i64 {
    let mut b = Base::ouvrir(pool, organisation).await.unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Client') RETURNING id").fetch_one(&mut *b).await.unwrap();
    let conditions: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7")
        .fetch_one(&mut *b)
        .await
        .unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&mut *b).await.unwrap();
    drop(b);
    let lieu = poster(site, "/factures", "", cookie).await.lieu.unwrap();
    let id: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(site, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference=R&client=e{client}&conditions_paiement_id={conditions}"), cookie).await;
    poster(site, &format!("/factures/{id}/lignes"), "genre=poste", cookie).await;
    let mut b = Base::ouvrir(pool, organisation).await.unwrap();
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&mut *b).await.unwrap();
    drop(b);
    poster(site, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travail&quantite=1&unite=h&prix_unitaire=100&taux_tva_id={taux}"), cookie).await;
    id
}

async fn taux(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT id FROM taux_tva WHERE {sql}"))).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn une_societe_en_franchise_facture_sans_tva(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = nouvelle_organisation(&pool, &s, "peinture", "franchise").await;
    let franchise = taux(&pool, "franchise").await;
    let normal = taux(&pool, "taux_centiemes = 2100").await;

    let id = facture(&s, &cookie, &pool, 2, franchise).await;
    let page = lire(&s, &format!("/factures/{id}"), &cookie).await.corps;
    assert!(page.contains("Franchise de TVA") && !page.contains("Taux normal") && !page.contains("Taux intermédiaire"), "seul taux proposé");
    let lieu = poster(&s, "/factures", "", &cookie).await.lieu.unwrap();
    let nouvelle: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let defaut: Option<i64> = {
        let mut b = Base::ouvrir(&pool, 2).await.unwrap();
        sqlx::query_scalar("SELECT taux_tva_id FROM factures WHERE id = $1").bind(nouvelle).fetch_one(&mut *b).await.unwrap()
    };
    assert_eq!(defaut, Some(franchise), "taux par défaut d'une nouvelle facture");
    // Un autre taux n'est pas accepté sur la ligne.
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let ligne: i64 = sqlx::query_scalar("SELECT id FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&mut *b).await.unwrap();
    drop(b);
    let r = poster(&s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travail&quantite=1&unite=h&prix_unitaire=100&taux_tva_id={normal}"), &cookie).await;
    assert!(r.corps.contains("Choisissez un taux de TVA."));

    assert!(poster(&s, &format!("/factures/{id}/valider"), "", &cookie).await.lieu.unwrap().starts_with(&format!("/factures/{id}")));
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let (numero, ht, ttc): (Option<i64>, i64, i64) =
        sqlx::query_as("SELECT numero, montant_ht_cents, montant_ttc_cents FROM factures WHERE id = $1").bind(id).fetch_one(&mut *b).await.unwrap();
    assert_eq!((numero, ht, ttc), (Some(1), 10_000, 10_000), "pas de TVA");
    let xml = ubifacture::ubl::de_la_facture(&mut b, id).await.unwrap().unwrap();
    assert!(xml.contains("<cbc:ID>E</cbc:ID><cbc:Percent>0</cbc:Percent><cbc:TaxExemptionReason>Régime particulier de franchise des petites entreprises."));
    let page = lire(&s, &format!("/factures/{id}"), &cookie).await;
    assert_eq!(page.statut, StatusCode::OK);
    let pdf = lire(&s, &format!("/factures/{id}/pdf"), &cookie).await;
    assert_eq!(pdf.statut, StatusCode::OK, "le PDF se fabrique avec la mention");
}

#[sqlx::test]
async fn une_societe_assujettie_n_a_pas_la_franchise_mais_a_le_12_pour_cent(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    let cookie = nouvelle_organisation(&pool, &s, "batiment", "assujetti").await;
    let franchise = taux(&pool, "franchise").await;
    let douze = taux(&pool, "taux_centiemes = 1200").await;

    let id = facture(&s, &cookie, &pool, 2, douze).await;
    let page = lire(&s, &format!("/factures/{id}"), &cookie).await.corps;
    assert!(page.contains("Taux intermédiaire") && !page.contains("Franchise de TVA"));
    poster(&s, &format!("/factures/{id}/valider"), "", &cookie).await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let (numero, ttc): (Option<i64>, i64) = sqlx::query_as("SELECT numero, montant_ttc_cents FROM factures WHERE id = $1").bind(id).fetch_one(&mut *b).await.unwrap();
    assert_eq!((numero, ttc), (Some(1), 11_200), "12 % de TVA");
    drop(b);

    // Une ligne au taux de franchise (glissée autrement que par l'écran) empêche la validation.
    let autre = facture(&s, &cookie, &pool, 2, douze).await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    sqlx::query("UPDATE facture_lignes SET taux_tva_id = $2, taux_centiemes = 0 WHERE facture_id = $1").bind(autre).bind(franchise).execute(&mut *b).await.unwrap();
    drop(b);
    assert_eq!(poster(&s, &format!("/factures/{autre}/valider"), "", &cookie).await.statut, StatusCode::CONFLICT);
    assert!(lire(&s, &format!("/factures/{autre}"), &cookie).await.corps.contains("réservé aux sociétés en franchise"));
}

#[sqlx::test]
async fn le_pdf_du_devis_suit_le_vocabulaire_et_la_franchise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = application(AppState { db: pool.clone() });
    nouvelle_organisation(&pool, &s, "graphisme", "franchise").await;
    let franchise = taux(&pool, "franchise").await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let devis: i64 = sqlx::query_scalar("INSERT INTO devis (numero, chantier_rue, chantier_ville) VALUES (1, 'Rue Haute 3', 'Wavre') RETURNING id")
        .fetch_one(&mut *b)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id)
         VALUES ($1, 1, 'poste_libre', 'Logo', 100000, 'forfait', 50000000, $2)",
    )
    .bind(devis)
    .bind(franchise)
    .execute(&mut *b)
    .await
    .unwrap();
    let donnees = ubifacture::devis_pdf::charger_donnees(&mut b, devis).await.unwrap_or_else(|_| panic!("devis"));
    let (source, _) = ubifacture::devis_pdf::source_typst(&donnees);
    assert!(source.contains("mot_chantier: \"Adresse du projet\"") && source.contains("franchise: true"), "{source}");
    assert!(source.contains("Régime particulier de franchise des petites entreprises."));
    assert!(donnees.a_completer().iter().all(|m| !m.contains("Franchise")), "les lignes suivent le régime");
}
