// Tests des organisations (étape 1-1) : la connexion d'une page est liée à une organisation, la session est ouverte
// dans l'organisation dont l'utilisateur est membre.

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
use ubifacture::{AppState, application, base::Base, connexion, db};

async fn reglage(conn: &mut sqlx::PgConnection) -> Option<String> {
    sqlx::query_scalar("SELECT nullif(current_setting('app.organisation_id', true), '')").fetch_one(conn).await.unwrap()
}

#[sqlx::test]
async fn l_organisation_1_existe_et_la_suivante_porte_le_2(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (id, nom): (i64, String) = sqlx::query_as("SELECT id, nom FROM organisations").fetch_one(&pool).await.unwrap();
    assert_eq!((id, nom.as_str()), (1, "Organisation 1"), "base neuve : pas de société, nom par défaut");
    let suivante: i64 = sqlx::query_scalar("INSERT INTO organisations (nom) VALUES ('B') RETURNING id").fetch_one(&pool).await.unwrap();
    assert_eq!(suivante, 2);
    assert!(sqlx::query("INSERT INTO organisations (nom) VALUES ('  ')").execute(&pool).await.is_err(), "un nom est obligatoire");
}

#[sqlx::test]
async fn la_connexion_d_une_page_porte_son_organisation(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let mut base = Base::ouvrir(&pool, 7).await.unwrap();
    assert_eq!(base.organisation, 7);
    assert_eq!(reglage(&mut base).await.as_deref(), Some("7"));
    // Une transaction sur cette connexion garde le réglage.
    let mut tx = base.begin().await.unwrap();
    assert_eq!(reglage(&mut tx).await.as_deref(), Some("7"));
}

#[sqlx::test]
async fn une_connexion_rendue_a_la_reserve_oublie_son_organisation(_: PgPoolOptions, connexion: PgConnectOptions) {
    // La vraie réserve (`db::options`), réduite à une connexion pour retrouver la même ensuite.
    let pool = db::options().max_connections(1).connect_with(connexion).await.unwrap();
    drop(Base::ouvrir(&pool, 7).await.unwrap());
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(reglage(&mut conn).await, None, "jamais l'organisation de la page précédente");
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let cookie = r.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, cookie, String::from_utf8_lossy(&octets).to_string())
}

async fn lire(site: &Router, uri: &str, cookie: &str) -> StatusCode {
    let requete = Request::builder().uri(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap();
    site.clone().oneshot(requete).await.unwrap().status()
}

#[sqlx::test]
async fn la_session_est_ouverte_dans_l_organisation_du_membre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO organisations (nom) VALUES ('Deuxième')").execute(&pool).await.unwrap();
    connexion::ajouter_utilisateur(&pool, 2, "deux@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    let erreur = connexion::ajouter_utilisateur(&pool, 99, "perdu@exemple.be", "", "un-long-mot-de-passe").await.unwrap_err();
    assert_eq!(erreur, "Aucune organisation n° 99.");
    let utilisateurs: i64 = sqlx::query_scalar("SELECT count(*) FROM utilisateurs").fetch_one(&pool).await.unwrap();
    assert_eq!(utilisateurs, 1, "rien n'est créé pour une organisation inconnue");
    let role: String = sqlx::query_scalar("SELECT role::text FROM membres WHERE organisation_id = 2").fetch_one(&pool).await.unwrap();
    assert_eq!(role, "proprietaire");

    let s = application(AppState { db: pool.clone() });
    let bon = "email=deux%40exemple.be&mot_de_passe=un-long-mot-de-passe";
    let (_, cookie, _) = poster(&s, "/connexion", bon).await;
    let cookie = cookie.expect("cookie de session");
    let organisation: i64 = sqlx::query_scalar("SELECT organisation_id FROM sessions").fetch_one(&pool).await.unwrap();
    assert_eq!(organisation, 2);
    assert_eq!(lire(&s, "/factures", &cookie).await, StatusCode::OK, "les pages reçoivent l'organisation de la session");

    // Retiré de son organisation : la session se ferme et il ne peut plus se connecter.
    sqlx::query("DELETE FROM membres").execute(&pool).await.unwrap();
    assert_eq!(lire(&s, "/factures", &cookie).await, StatusCode::SEE_OTHER);
    let (statut, cookie, corps) = poster(&s, "/connexion", bon).await;
    assert!(statut == StatusCode::UNAUTHORIZED && cookie.is_none() && corps.contains("rattaché à aucune organisation"));
}

// ─── Étape 1-2 : chaque donnée appartient à une organisation ─────────────────

/// Une deuxième organisation avec sa société et un devis envoyé (lien secret « jeton-b »).
async fn organisation_b(pool: &PgPool) -> i64 {
    sqlx::query("INSERT INTO organisations (nom) VALUES ('B')").execute(pool).await.unwrap();
    let mut b = Base::ouvrir(pool, 2).await.unwrap();
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('Société B', 'B', 'b@b.be', '0')")
        .execute(&mut *b)
        .await
        .unwrap();
    sqlx::query_scalar("INSERT INTO devis (numero, statut, jeton) VALUES (1, 'envoye', 'jeton-b') RETURNING id")
        .fetch_one(&mut *b)
        .await
        .unwrap()
}

#[sqlx::test]
async fn sans_organisation_la_base_ne_montre_rien_et_refuse_tout(_: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = db::options().connect_with(connexion).await.unwrap();
    let mut a = Base::ouvrir(&pool, 1).await.unwrap();
    sqlx::query("INSERT INTO entreprises (nom) VALUES ('Client A')").execute(&mut *a).await.unwrap();
    drop(a);
    let mut conn = pool.acquire().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM entreprises").fetch_one(&mut *conn).await.unwrap();
    assert_eq!(n, 0, "aucune ligne sans organisation");
    assert!(sqlx::query("INSERT INTO entreprises (nom) VALUES ('X')").execute(&mut *conn).await.is_err());
    assert_eq!(sqlx::query("UPDATE entreprises SET nom = 'Y'").execute(&mut *conn).await.unwrap().rows_affected(), 0);
    assert_eq!(sqlx::query("DELETE FROM entreprises").execute(&mut *conn).await.unwrap().rows_affected(), 0);
    let mut a = Base::ouvrir(&pool, 1).await.unwrap();
    let nom: String = sqlx::query_scalar("SELECT nom FROM entreprises").fetch_one(&mut *a).await.unwrap();
    assert_eq!(nom, "Client A", "rien n'a été touché");
}

#[sqlx::test]
async fn une_organisation_ne_lit_ni_n_ecrit_les_donnees_d_une_autre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let entreprise_a: i64 =
        sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Client A') RETURNING id").fetch_one(&pool).await.unwrap();
    organisation_b(&pool).await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();

    let visibles: i64 = sqlx::query_scalar("SELECT count(*) FROM entreprises").fetch_one(&mut *b).await.unwrap();
    assert_eq!(visibles, 0);
    let par_id: Option<String> =
        sqlx::query_scalar("SELECT nom FROM entreprises WHERE id = $1").bind(entreprise_a).fetch_optional(&mut *b).await.unwrap();
    assert_eq!(par_id, None, "même avec son numéro");
    let modifiees = sqlx::query("UPDATE entreprises SET nom = 'Piraté' WHERE id = $1").bind(entreprise_a).execute(&mut *b).await.unwrap();
    assert_eq!(modifiees.rows_affected(), 0);
    let ecrire_chez_a = sqlx::query("INSERT INTO entreprises (organisation_id, nom) VALUES (1, 'Intrus')").execute(&mut *b).await;
    assert!(ecrire_chez_a.is_err(), "pas d'écriture dans une autre organisation");
    let deplacer = sqlx::query("UPDATE devis SET organisation_id = 1").execute(&mut *b).await;
    assert!(deplacer.is_err(), "une ligne ne change pas d'organisation");
    // Un devis de B ne peut pas désigner le client de A (lien « même organisation »).
    let lien = sqlx::query("INSERT INTO devis (numero, client_entreprise_id) VALUES (2, $1)").bind(entreprise_a).execute(&mut *b).await;
    assert!(lien.is_err());

    let nom: String = sqlx::query_scalar("SELECT nom FROM entreprises WHERE id = $1").bind(entreprise_a).fetch_one(&pool).await.unwrap();
    assert_eq!(nom, "Client A");
    let devis_a: i64 = sqlx::query_scalar("SELECT count(*) FROM devis").fetch_one(&pool).await.unwrap();
    assert_eq!(devis_a, 0, "A ne voit pas le devis de B");
}

#[sqlx::test]
async fn chaque_organisation_a_sa_numerotation(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    organisation_b(&pool).await;
    // Le devis n° 1 existe déjà chez B ; A peut avoir le sien.
    sqlx::query("INSERT INTO devis (numero) VALUES (1)").execute(&pool).await.unwrap();
    assert!(sqlx::query("INSERT INTO devis (numero) VALUES (1)").execute(&pool).await.is_err(), "mais pas deux fois chez A");
}

#[sqlx::test]
async fn la_page_client_travaille_dans_l_organisation_du_lien(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('Société A', 'A', 'a@a.be', '0')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO societe_fichiers (usage, nom, type_mime, contenu) VALUES ('logo', 'a.png', 'image/png', 'logo de A')")
        .execute(&pool)
        .await
        .unwrap();
    organisation_b(&pool).await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    sqlx::query("INSERT INTO societe_fichiers (usage, nom, type_mime, contenu) VALUES ('logo', 'b.png', 'image/png', 'logo de B')")
        .execute(&mut *b)
        .await
        .unwrap();
    drop(b);

    let s = application(AppState { db: pool.clone() });
    let lire = |uri: &'static str| {
        let s = s.clone();
        async move {
            let r = s.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
            let statut = r.status();
            (statut, String::from_utf8_lossy(&r.into_body().collect().await.unwrap().to_bytes()).to_string())
        }
    };
    let (statut, page) = lire("/client/devis/jeton-b").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Société B") && !page.contains("Société A"), "la société de B, jamais celle de A");
    assert!(page.contains("/client/devis/jeton-b/logo"));
    assert_eq!(lire("/client/devis/jeton-b/logo").await.1, "logo de B");
    assert_eq!(lire("/client/devis/inconnu").await.0, StatusCode::NOT_FOUND);
    assert_eq!(lire("/client/devis/inconnu/logo").await.0, StatusCode::NOT_FOUND);
    assert_eq!(lire("/client/facture/jeton-b").await.0, StatusCode::NOT_FOUND, "un lien de devis n'ouvre pas une facture");
    let consultations: i64 = sqlx::query_scalar("SELECT count(*) FROM consultations_client").fetch_one(&pool).await.unwrap();
    assert_eq!(consultations, 0, "la consultation est notée chez B, pas chez A");
}

#[sqlx::test]
async fn l_utilisateur_de_la_base_ne_contourne_pas_les_regles(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (super_utilisateur, contourne): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user").fetch_one(&pool).await.unwrap();
    assert!(!super_utilisateur && !contourne, "l'application ne doit jamais se connecter avec un compte qui contourne la RLS");
    let sans_regle: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'organisation_id'
         WHERE c.relkind = 'r' AND c.relnamespace = 'public'::regnamespace AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
           AND c.relname NOT IN ('membres', 'sessions')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(sans_regle, Vec::<String>::new(), "toute table qui porte organisation_id est protégée");
}
