// Tests de l'étape 6-3 : avancements (saisie en quantité, en % ou en €, cumul entre 0 et 100 %, validation).

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
use ubifacture::{AppState, app};

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, String) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, lieu, String::from_utf8_lossy(&octets).to_string())
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, corps)
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    envoyer(site, requete).await
}

const E: &str = "\u{a0}€";

struct Chantier {
    id: i64,
    devis: i64,
    /// Lignes du devis de base : A (1 × 6 000,00 €) et B (2 × 2 192,065 € = 4 384,13 €).
    a: i64,
    b: i64,
    /// Ligne de l'avenant accepté : C (1 × −3 780,36 €).
    c: i64,
}

async fn ligne(pool: &PgPool, devis: i64, position: i32, genre: &str, description: &str, q: Option<i64>, p: Option<i64>) -> i64 {
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(pool).await.unwrap();
    let chiffre = q.is_some();
    sqlx::query_scalar(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id)
         VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(devis)
    .bind(position)
    .bind(genre)
    .bind(description)
    .bind(q)
    .bind(if chiffre { "m²" } else { "" })
    .bind(p)
    .bind(chiffre.then_some(tva))
    .fetch_one(pool)
    .await
    .unwrap()
}

/// L'exemple réel : devis de base 10 384,13 € HT, avenant accepté −3 780,36 € → 6 603,77 € HT.
/// `montant_base` permet de simuler une remise sur le devis de base (ajustement).
async fn chantier(pool: &PgPool, s: &Router, montant_base: i64) -> Chantier {
    let devis: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, statut, reference, montant_ht_cents) VALUES (57, '2026-10-01', 'envoye', 'PL45', $1) RETURNING id",
    )
    .bind(montant_base)
    .fetch_one(pool)
    .await
    .unwrap();
    ligne(pool, devis, 1, "titre", "Salle de bain", None, None).await;
    let a = ligne(pool, devis, 2, "poste_libre", "Carrelage", Some(100_000), Some(600_000_000)).await;
    let b = ligne(pool, devis, 3, "fourniture", "Faïence", Some(200_000), Some(219_206_500)).await;
    let (statut, _, _) = poster(s, &format!("/devis/{devis}/statut/accepter"), "date=2026-10-05").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let id: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(pool).await.unwrap();
    let avenant: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, chantier_id, date_devis, statut, montant_ht_cents) VALUES (2, $1, '2026-10-10', 'accepte', -378036) RETURNING id",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    let c = ligne(pool, avenant, 1, "poste_libre", "Travaux non exécutés", Some(100_000), Some(-378_036_000)).await;
    Chantier { id, devis, a, b, c }
}

/// Crée l'avancement suivant et renvoie son id.
async fn nouvel_avancement(s: &Router, chantier: i64) -> i64 {
    let (statut, lieu, _) = poster(s, &format!("/chantiers/{chantier}/avancements"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let lieu = lieu.unwrap();
    let reste = lieu.strip_prefix(&format!("/chantiers/{chantier}/avancements/")).unwrap();
    reste.split('?').next().unwrap().parse().unwrap()
}

async fn saisir(s: &Router, ch: i64, av: i64, cle: &str, champ: &str) -> (StatusCode, String) {
    let (statut, _, page) = poster(s, &format!("/chantiers/{ch}/avancements/{av}/lignes/{cle}"), champ).await;
    (statut, page)
}

async fn montant(pool: &PgPool, av: i64) -> i64 {
    sqlx::query_scalar("SELECT montant_ht_cents FROM avancements WHERE id = $1").bind(av).fetch_one(pool).await.unwrap()
}

// ─── L'exemple réel ─────────────────────────────────────────────────────────

#[sqlx::test]
async fn l_exemple_reel_55_04_pour_cent_puis_la_fin_du_chantier(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let (numero, reference, statut): (i32, String, String) =
        sqlx::query_as("SELECT numero, reference, statut::text FROM avancements WHERE id = $1").bind(av).fetch_one(&pool).await.unwrap();
    assert_eq!((numero, reference.as_str(), statut.as_str()), (1, "Avancement n° 1", "brouillon"));

    // La page reprend le devis de base puis l'avenant accepté.
    let (code, page) = lire(&s, &format!("/chantiers/{}/avancements/{av}?ok=cree", ch.id)).await;
    assert_eq!(code, StatusCode::OK);
    for attendu in ["Devis 2026-57", "Avenant n° 2", "Carrelage", "Faïence", "Travaux non exécutés", "1.0", "1.1", "1.2", "6\u{a0}603,77\u{a0}€"] {
        assert!(page.contains(attendu), "{attendu}");
    }

    // A à 50 % (3 000,00 €), B en euros (634,47 €) : 3 634,47 € sur 6 603,77 € = 55,04 %.
    let (code, grille) = saisir(&s, ch.id, av, &format!("l{}", ch.a), "pourcentage=50").await;
    assert_eq!(code, StatusCode::OK);
    assert!(grille.contains("id=\"grille\"") && !grille.contains("<html"), "htmx ne reçoit que la grille");
    let (_, grille) = saisir(&s, ch.id, av, &format!("l{}", ch.b), "montant=634,47").await;
    assert_eq!(montant(&pool, av).await, 363_447);
    assert!(grille.contains(&format!("3\u{a0}634,47{E}")) && grille.contains("(55,04 %)") && grille.contains(&format!("2\u{a0}969,30{E}")));

    // Validé : figé, et visible dans l'onglet Finances.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements/{av}/valider", ch.id), "").await;
    assert_eq!(lieu, Some(format!("/chantiers/{}/avancements/{av}?ok=valide", ch.id)));
    let (_, page) = lire(&s, &format!("/chantiers/{}/finances", ch.id)).await;
    assert!(page.contains("Avancement n° 1") && page.contains("55,04 %") && page.contains(&format!("2\u{a0}969,30{E}")) && page.contains("Validé"));
    let (_, page) = lire(&s, &format!("/chantiers/{}", ch.id)).await;
    assert!(page.contains("Reste à avancer HT") && page.contains(&format!("2\u{a0}969,30{E}")));

    // Avancement n° 2 : on termine tout. Le précédent est repris.
    let av2 = nouvel_avancement(&s, ch.id).await;
    let (_, page) = lire(&s, &format!("/chantiers/{}/avancements/{av2}", ch.id)).await;
    assert!(page.contains("Avancement n° 2") && page.contains(&format!("3\u{a0}000,00{E}")));
    // A est déjà à 50 % : 100 % de plus dépasserait. On saisit le reste en euros.
    let (_, grille) = saisir(&s, ch.id, av2, &format!("l{}", ch.a), "pourcentage=100").await;
    assert!(grille.contains("le cumul doit rester entre 0 et 100 %"));
    saisir(&s, ch.id, av2, &format!("l{}", ch.a), "montant=3000").await;
    saisir(&s, ch.id, av2, &format!("l{}", ch.b), "montant=3749,66").await;
    saisir(&s, ch.id, av2, &format!("l{}", ch.c), "pourcentage=100").await;
    assert_eq!(montant(&pool, av2).await, 296_930, "le dernier avancement reprend le reste à avancer");
    let (_, grille) = saisir(&s, ch.id, av2, &format!("l{}", ch.c), "pourcentage=100").await;
    assert!(grille.contains(&format!("Reste à avancer HT</td><td class=\"text-right whitespace-nowrap\">0,00{E}")));
}

#[sqlx::test]
async fn le_cumul_ne_depasse_jamais_100_pour_cent(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let a = format!("l{}", ch.a);

    let (code, grille) = saisir(&s, ch.id, av, &a, "pourcentage=100,01").await;
    assert_eq!(code, StatusCode::OK);
    assert!(grille.contains("le cumul doit rester entre 0 et 100 %"));
    assert_eq!(montant(&pool, av).await, 0);
    let (_, grille) = saisir(&s, ch.id, av, &a, "montant=-1").await;
    assert!(grille.contains("le cumul doit rester entre 0 et 100 %"), "pas de cumul négatif");

    // La ligne négative de l'avenant s'avance vers son total négatif, pas au-delà.
    let c = format!("l{}", ch.c);
    let (_, grille) = saisir(&s, ch.id, av, &c, "montant=100").await;
    assert!(grille.contains("le cumul doit rester entre 0 et 100 %"));
    let (_, grille) = saisir(&s, ch.id, av, &c, "montant=-3780,36").await;
    assert!(!grille.contains("alert-warning"));
    assert_eq!(montant(&pool, av).await, -378_036);

    // Une correction négative reste permise si le cumul ne passe pas sous 0.
    saisir(&s, ch.id, av, &a, "pourcentage=50").await;
    poster(&s, &format!("/chantiers/{}/avancements/{av}/valider", ch.id), "").await;
    let av2 = nouvel_avancement(&s, ch.id).await;
    let (_, grille) = saisir(&s, ch.id, av2, &a, "montant=-1000").await;
    assert!(!grille.contains("alert-warning"));
    assert_eq!(montant(&pool, av2).await, -100_000);
    let (_, grille) = saisir(&s, ch.id, av2, &a, "montant=-3000,01").await;
    assert!(grille.contains("le cumul doit rester entre 0 et 100 %"));
}

#[sqlx::test]
async fn saisir_en_quantite_ou_effacer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let b = format!("l{}", ch.b);
    // 1 × 2 192,065 € = 2 192,07 € (arrondi au centime)
    let (_, grille) = saisir(&s, ch.id, av, &b, "quantite=1").await;
    assert_eq!(montant(&pool, av).await, 219_207);
    assert!(grille.contains(&format!("2\u{a0}192,07{E}")) && grille.contains("value=\"1\""));
    // Un nombre illisible ne change rien.
    let (_, grille) = saisir(&s, ch.id, av, &b, "quantite=beaucoup").await;
    assert!(grille.contains("« beaucoup » n&#39;est pas un nombre") || grille.contains("« beaucoup » n'est pas un nombre"));
    assert_eq!(montant(&pool, av).await, 219_207);
    // Un champ vidé remet la ligne à 0.
    saisir(&s, ch.id, av, &b, "montant=").await;
    assert_eq!(montant(&pool, av).await, 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM avancement_lignes").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0, "une ligne à 0 n'est pas gardée");
}

#[sqlx::test]
async fn l_ajustement_du_devis_s_avance_comme_une_ligne(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    // Remise de 384,13 € sur le devis de base : 10 000,00 € HT.
    let ch = chantier(&pool, &s, 1_000_000).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let (_, page) = lire(&s, &format!("/chantiers/{}/avancements/{av}", ch.id)).await;
    assert!(page.contains("Ajustement") && page.contains(&format!("-384,13{E}")));
    let (_, grille) = saisir(&s, ch.id, av, &format!("a{}", ch.devis), "pourcentage=100").await;
    assert!(!grille.contains("alert-warning"));
    assert_eq!(montant(&pool, av).await, -38_413);
    // L'ajustement n'a pas de quantité.
    let (code, _) = saisir(&s, ch.id, av, &format!("a{}", ch.devis), "quantite=1").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(montant(&pool, av).await, -38_413);
}

// ─── Brouillon, validation, suppression ─────────────────────────────────────

#[sqlx::test]
async fn un_seul_brouillon_a_la_fois(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements", ch.id), "").await;
    assert_eq!(lieu, Some(format!("/chantiers/{}/avancements/{av}?erreur=brouillon", ch.id)));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM avancements").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);
    let (_, page) = lire(&s, &format!("/chantiers/{}/finances", ch.id)).await;
    assert!(page.contains("Reprendre le brouillon") && !page.contains("Nouvel avancement"));
}

#[sqlx::test]
async fn valider_figer_remettre_en_brouillon_supprimer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let base = format!("/chantiers/{}/avancements/{av}", ch.id);

    // Vide : on ne valide pas.
    let (_, lieu, _) = poster(&s, &format!("{base}/valider"), "").await;
    assert_eq!(lieu, Some(format!("{base}?erreur=vide")));

    // Informations : date et référence.
    let (_, lieu, _) = poster(&s, &format!("{base}/infos"), "date=2026-10-20&reference=Situation%20octobre").await;
    assert_eq!(lieu, Some(format!("{base}?ok=infos")));
    for (corps, code) in [("date=2026-13-01&reference=X", "erreur=date"), ("date=2026-10-20&reference=%20", "erreur=reference")] {
        let (_, lieu, _) = poster(&s, &format!("{base}/infos"), corps).await;
        assert_eq!(lieu, Some(format!("{base}?{code}")), "{corps}");
    }
    let (_, page) = lire(&s, &base).await;
    assert!(page.contains("Situation octobre") && page.contains("2026-10-20"));

    saisir(&s, ch.id, av, &format!("l{}", ch.a), "pourcentage=10").await;
    poster(&s, &format!("{base}/valider"), "").await;
    // Validé : plus de saisie, plus d'informations, pas de suppression.
    assert_eq!(saisir(&s, ch.id, av, &format!("l{}", ch.a), "pourcentage=20").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("{base}/infos"), "date=2026-10-21&reference=X").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("{base}/supprimer"), "").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("{base}/valider"), "").await.0, StatusCode::CONFLICT);
    let (_, page) = lire(&s, &base).await;
    assert!(page.contains("Remettre en brouillon") && !page.contains("name=\"pourcentage\""));

    // Un second avancement : le premier n'est plus le dernier, il ne redevient pas brouillon.
    let av2 = nouvel_avancement(&s, ch.id).await;
    assert_eq!(poster(&s, &format!("{base}/brouillon"), "").await.0, StatusCode::CONFLICT);
    // Le brouillon n° 2 se supprime ; le n° 1 redevient alors le dernier.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements/{av2}/supprimer", ch.id), "").await;
    assert_eq!(lieu, Some(format!("/chantiers/{}/finances", ch.id)));
    let (_, lieu, _) = poster(&s, &format!("{base}/brouillon"), "").await;
    assert_eq!(lieu, Some(format!("{base}?ok=brouillon")));
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM avancements WHERE id = $1").bind(av).fetch_one(&pool).await.unwrap();
    assert_eq!(statut, "brouillon");
}

#[sqlx::test]
async fn un_avenant_non_accepte_n_est_pas_dans_l_avancement(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let brouillon: i64 = sqlx::query_scalar("INSERT INTO devis (numero, chantier_id, montant_ht_cents) VALUES (3, $1, 50000) RETURNING id")
        .bind(ch.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let l = ligne(&pool, brouillon, 1, "poste_libre", "Extension pas encore acceptée", Some(100_000), Some(50_000_000)).await;
    let av = nouvel_avancement(&s, ch.id).await;
    let (_, page) = lire(&s, &format!("/chantiers/{}/avancements/{av}", ch.id)).await;
    assert!(!page.contains("Extension pas encore acceptée") && !page.contains("Avenant n° 3"));
    assert_eq!(saisir(&s, ch.id, av, &format!("l{l}"), "pourcentage=10").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn adresses_inconnues(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ch = chantier(&pool, &s, 1_038_413).await;
    let av = nouvel_avancement(&s, ch.id).await;
    assert_eq!(poster(&s, "/chantiers/999/avancements", "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(lire(&s, &format!("/chantiers/{}/avancements/999", ch.id)).await.0, StatusCode::NOT_FOUND);
    // Un avancement n'est lisible que sous son propre chantier.
    assert_eq!(lire(&s, &format!("/chantiers/999/avancements/{av}")).await.0, StatusCode::NOT_FOUND);
    // Une ligne d'un autre chantier, un titre ou une clé bidon : refusés.
    let titre: i64 = sqlx::query_scalar("SELECT id FROM devis_lignes WHERE genre = 'titre'").fetch_one(&pool).await.unwrap();
    for cle in [format!("l{titre}"), "l999".to_string(), "x1".to_string()] {
        assert_eq!(saisir(&s, ch.id, av, &cle, "pourcentage=1").await.0, StatusCode::NOT_FOUND, "{cle}");
    }
}
