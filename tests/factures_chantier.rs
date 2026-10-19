// Tests de l'étape 7-2 : factures d'acompte, d'avancement et finale, créées depuis le chantier.

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

async fn taux(pool: &PgPool, centiemes: i32) -> i64 {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(centiemes).fetch_one(pool).await.unwrap()
}

async fn ligne(pool: &PgPool, devis: i64, position: i32, genre: &str, description: &str, q: Option<i64>, p: Option<i64>, tva: i32) -> i64 {
    let chiffre = q.is_some();
    let t = taux(pool, tva).await;
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
    .bind(chiffre.then_some(t))
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Un devis envoyé, avec client et conditions de paiement (30 jours) : renvoie son id.
async fn devis_envoye(pool: &PgPool, montant_ht: i64) -> i64 {
    let client: i64 =
        sqlx::query_scalar("INSERT INTO entreprises (nom, profils) VALUES ('Client SA', '{client}') RETURNING id").fetch_one(pool).await.unwrap();
    let devis: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, statut, reference, responsable, montant_ht_cents, client_entreprise_id, conditions_paiement_id)
         VALUES (57, '2026-10-01', 'envoye', 'PL45', 'Max', $1, $2,
                 (SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30)) RETURNING id",
    )
    .bind(montant_ht)
    .bind(client)
    .fetch_one(pool)
    .await
    .unwrap();
    devis
}

async fn accepter(pool: &PgPool, s: &Router, devis: i64) -> i64 {
    let (statut, _, _) = poster(s, &format!("/devis/{devis}/statut/accepter"), "date=2026-10-05").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(pool).await.unwrap()
}

struct Exemple {
    chantier: i64,
    a: i64,
    b: i64,
}

/// L'exemple réel : devis de base 10 384,13 € HT (A : 6 000,00 € ; B : 4 384,13 €), avenant accepté −3 780,36 €,
/// le tout à 6 %. Total devis/avenants : 6 603,77 € HT.
async fn exemple(pool: &PgPool, s: &Router) -> Exemple {
    let devis = devis_envoye(pool, 1_038_413).await;
    ligne(pool, devis, 1, "titre", "Salle de bain", None, None, 600).await;
    let a = ligne(pool, devis, 2, "poste_libre", "Carrelage", Some(100_000), Some(600_000_000), 600).await;
    let b = ligne(pool, devis, 3, "fourniture", "Faïence", Some(200_000), Some(219_206_500), 600).await;
    let chantier = accepter(pool, s, devis).await;
    let avenant: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, chantier_id, date_devis, statut, montant_ht_cents) VALUES (2, $1, '2026-10-10', 'accepte', -378036) RETURNING id",
    )
    .bind(chantier)
    .fetch_one(pool)
    .await
    .unwrap();
    ligne(pool, avenant, 1, "poste_libre", "Travaux non exécutés", Some(100_000), Some(-378_036_000), 600).await;
    Exemple { chantier, a, b }
}

/// Suit une redirection « /factures/{id}?ok=creee » et renvoie l'id.
fn id_facture(lieu: Option<String>) -> i64 {
    let lieu = lieu.expect("redirection");
    lieu.strip_prefix("/factures/").unwrap_or_else(|| panic!("{lieu}")).split('?').next().unwrap().parse().unwrap()
}

async fn montants(pool: &PgPool, id: i64) -> (i64, i64, i64) {
    sqlx::query_as("SELECT montant_ht_cents, montant_tva_cents, montant_ttc_cents FROM factures WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn valider_facture(pool: &PgPool, s: &Router, id: i64) -> i64 {
    let (statut, lieu, page) = poster(s, &format!("/factures/{id}/valider"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER, "{page}");
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{id}?ok=validee").as_str()));
    sqlx::query_scalar::<_, Option<i64>>("SELECT numero FROM factures WHERE id = $1").bind(id).fetch_one(pool).await.unwrap().unwrap()
}

/// Un avancement validé : A à 50 % et B pour 634,47 € = 3 634,47 € (55,04 %).
async fn avancement_55(pool: &PgPool, s: &Router, ex: &Exemple) -> i64 {
    let (_, lieu, _) = poster(s, &format!("/chantiers/{}/avancements", ex.chantier), "").await;
    let lieu = lieu.unwrap();
    let av: i64 = lieu.rsplit('/').next().unwrap().split('?').next().unwrap().parse().unwrap();
    poster(s, &format!("/chantiers/{}/avancements/{av}/lignes/l{}", ex.chantier, ex.a), "pourcentage=50").await;
    poster(s, &format!("/chantiers/{}/avancements/{av}/lignes/l{}", ex.chantier, ex.b), "montant=634,47").await;
    let (statut, _, _) = poster(s, &format!("/chantiers/{}/avancements/{av}/valider", ex.chantier), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let m: i64 = sqlx::query_scalar("SELECT montant_ht_cents FROM avancements WHERE id = $1").bind(av).fetch_one(pool).await.unwrap();
    assert_eq!(m, 363_447);
    av
}

#[sqlx::test]
async fn l_exemple_reel_avancement_puis_facture_finale(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ex = exemple(&pool, &s).await;
    let av = avancement_55(&pool, &s, &ex).await;

    // Facture d'avancement : 3 634,47 € HT à 6 %, en-tête repris du devis.
    let (statut, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements/{av}/facturer", ex.chantier), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let fa = id_facture(lieu);
    let (genre, reference, client, avancement): (String, String, Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT genre::text, reference, client_entreprise_id, avancement_id FROM factures WHERE id = $1")
            .bind(fa)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((genre.as_str(), reference.as_str(), client.is_some(), avancement), ("avancement", "Avancement n° 1", true, Some(av)));
    assert_eq!(montants(&pool, fa).await, (363_447, 21_807, 385_254));
    let (_, page) = lire(&s, &format!("/factures/{fa}")).await;
    for attendu in ["Facture d&#39;avancement", "Devis 2026-57", "1.1 Carrelage", "Avancement de la période : 50,00 %", "Chantier n° 1"] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("&#39;", "'")), "{attendu}");
    }
    assert!(!page.contains("name=\"quantite\""), "les lignes d'avancement ne se saisissent pas");
    assert_eq!(poster(&s, &format!("/factures/{fa}/lignes"), "genre=poste").await.0, StatusCode::CONFLICT);

    // Facturer deux fois le même avancement renvoie vers la facture existante.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements/{av}/facturer", ex.chantier), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{fa}").as_str()));
    assert_eq!(valider_facture(&pool, &s, fa).await, 125);

    // Un avancement facturé ne se remet plus en brouillon.
    assert_eq!(poster(&s, &format!("/chantiers/{}/avancements/{av}/brouillon", ex.chantier), "").await.0, StatusCode::CONFLICT);

    // Facture finale : le reste à avancer, 2 969,30 € HT ; TVA 6 % 178,16 € ; TTC 3 147,46 €.
    let (statut, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/finale", ex.chantier), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let ff = id_facture(lieu);
    assert_eq!(montants(&pool, ff).await, (296_930, 17_816, 314_746));
    assert_eq!(valider_facture(&pool, &s, ff).await, 126);

    // Un avancement « Solde » validé porte le chantier à 100 % ; total facturé = total devis/avenants.
    let (reference, statut): (String, String) =
        sqlx::query_as("SELECT a.reference, a.statut::text FROM avancements a JOIN factures f ON f.avancement_id = a.id WHERE f.id = $1")
            .bind(ff)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((reference.as_str(), statut.as_str()), ("Solde (facture finale)", "valide"));
    let (_, page) = lire(&s, &format!("/chantiers/{}/finances", ex.chantier)).await;
    for attendu in ["Facture N° 125", "Facture N° 126", "6\u{a0}603,77\u{a0}€", "100,00 %", "Facture finale créée"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(page.contains("Reste à facturer HT <strong>0,00\u{a0}€</strong>"));

    // Après la finale : ni nouvel avancement, ni acompte, ni seconde finale.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements", ex.chantier), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/chantiers/{}/finances?erreur=finale", ex.chantier).as_str()));
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/acompte", ex.chantier), "mode=pourcentage&valeur=10").await;
    assert_eq!(lieu.as_deref(), Some(format!("/chantiers/{}/finances?erreur=finale", ex.chantier).as_str()));
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/finale", ex.chantier), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/chantiers/{}/finances?erreur=finale", ex.chantier).as_str()));
    let (_, page) = lire(&s, &format!("/chantiers/{}/finances?erreur=finale", ex.chantier)).await;
    assert!(page.contains("déjà sa facture finale"));
}

#[sqlx::test]
async fn l_acompte_est_deduit_de_la_facture_finale(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ex = exemple(&pool, &s).await;

    // 30 % de 6 603,77 € = 1 981,13 € HT.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/acompte", ex.chantier), "mode=pourcentage&valeur=30").await;
    let acompte = id_facture(lieu);
    assert_eq!(montants(&pool, acompte).await.0, 198_113);
    let (_, page) = lire(&s, &format!("/factures/{acompte}")).await;
    assert!(page.contains("Acompte de 30 % sur le devis 2026-57"));

    // Tant que l'acompte est en brouillon, la facture finale attend.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/finale", ex.chantier), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/chantiers/{}/finances?erreur=facture_brouillon", ex.chantier).as_str()));
    assert_eq!(valider_facture(&pool, &s, acompte).await, 125);

    // La finale facture tout le chantier (rien n'a été avancé) et déduit l'acompte : 6 603,77 − 1 981,13 = 4 622,64 € HT.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/finale", ex.chantier), "").await;
    let finale = id_facture(lieu);
    let (ht, tva, ttc) = montants(&pool, finale).await;
    assert_eq!(ht, 462_264);
    assert_eq!((tva, ttc), (27_736, 490_000));
    let (_, page) = lire(&s, &format!("/factures/{finale}")).await;
    assert!(page.contains("Déduction des acomptes (facture n° 125)"));
    assert_eq!(valider_facture(&pool, &s, finale).await, 126);
    // Total facturé HT = total du chantier.
    let total: i64 = sqlx::query_scalar("SELECT sum(montant_ht_cents)::bigint FROM factures WHERE chantier_id = $1")
        .bind(ex.chantier)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 660_377);
}

#[sqlx::test]
async fn un_acompte_refuse_et_une_finale_brouillon_supprimee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let ex = exemple(&pool, &s).await;
    for corps in ["mode=pourcentage&valeur=0", "mode=montant&valeur=999999", "mode=pourcentage&valeur=abc", "mode=x&valeur=10"] {
        let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/acompte", ex.chantier), corps).await;
        assert_eq!(lieu.as_deref(), Some(format!("/chantiers/{}/finances?erreur=acompte", ex.chantier).as_str()), "{corps}");
    }
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/acompte", ex.chantier), "mode=montant&valeur=1000").await;
    let acompte = id_facture(lieu);
    assert_eq!(montants(&pool, acompte).await.0, 100_000);
    poster(&s, &format!("/factures/{acompte}/supprimer"), "").await;

    // Un avancement en brouillon bloque la finale.
    poster(&s, &format!("/chantiers/{}/avancements", ex.chantier), "").await;
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/finale", ex.chantier), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/chantiers/{}/finances?erreur=avancement_brouillon", ex.chantier).as_str()));
    sqlx::query("DELETE FROM avancements WHERE chantier_id = $1").bind(ex.chantier).execute(&pool).await.unwrap();

    // Supprimer la finale en brouillon retire aussi son avancement de solde : on peut encore avancer.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/factures/finale", ex.chantier), "").await;
    let finale = id_facture(lieu);
    let (statut, lieu, _) = poster(&s, &format!("/factures/{finale}/supprimer"), "").await;
    assert_eq!((statut, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/chantiers/{}/finances", ex.chantier).as_str())));
    let restants: i64 = sqlx::query_scalar("SELECT count(*) FROM avancements WHERE chantier_id = $1").bind(ex.chantier).fetch_one(&pool).await.unwrap();
    assert_eq!(restants, 0);
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{}/avancements", ex.chantier), "").await;
    assert!(lieu.unwrap().contains("ok=cree"));
}

#[sqlx::test]
async fn un_acompte_se_repartit_entre_les_taux_et_suit_la_remise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    // 1 000 € à 21 % et 1 000 € à 6 %, remise de 200 € : 1 800 € HT, soit 900 € par taux.
    let devis = devis_envoye(&pool, 180_000).await;
    ligne(&pool, devis, 1, "poste_libre", "Électricité", Some(100_000), Some(100_000_000), 2100).await;
    ligne(&pool, devis, 2, "poste_libre", "Rénovation", Some(100_000), Some(100_000_000), 600).await;
    let chantier = accepter(&pool, &s, devis).await;
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{chantier}/factures/acompte"), "mode=pourcentage&valeur=50").await;
    let acompte = id_facture(lieu);
    let parts: Vec<(i32, i64)> =
        sqlx::query_as("SELECT taux_centiemes, total_cents FROM facture_lignes WHERE facture_id = $1 AND genre = 'poste' ORDER BY taux_centiemes")
            .bind(acompte)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(parts, vec![(600, 45_000), (2100, 45_000)]);
    // TVA : 27,00 + 94,50 = 121,50 €
    assert_eq!(montants(&pool, acompte).await, (90_000, 12_150, 102_150));
}
