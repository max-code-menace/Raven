// Tests de l'étape 6-2 : avenants (un devis rattaché au chantier, numéroté dans le chantier).

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
    let disposition = r.headers().get(header::CONTENT_DISPOSITION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, lieu.or(disposition), String::from_utf8_lossy(&octets).to_string())
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

struct Base {
    devis: i64,
    chantier: i64,
    entreprise: i64,
    tva: i64,
    conditions: i64,
}

/// Le devis de base de l'exemple réel (10 384,13 € HT, TVA 6 %), accepté : il a donc son chantier.
async fn chantier_de_base(pool: &PgPool, s: &Router) -> Base {
    let entreprise: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, ville) VALUES ('Client A', 'Mons') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap();
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(pool).await.unwrap();
    let conditions: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement ORDER BY id LIMIT 1").fetch_one(pool).await.unwrap();
    sqlx::query("UPDATE compteurs SET valeur = 57, annee = EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int WHERE nom = 'devis'").execute(pool).await.unwrap();
    let devis: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, statut, reference, responsable, client_entreprise_id, taux_tva_id,
                conditions_paiement_id, montant_ht_cents, montant_ttc_cents)
         VALUES (57, '2026-10-01', 'envoye', 'PL45', 'Mahx', $1, $2, $3, 1038413, 1100718) RETURNING id",
    )
    .bind(entreprise)
    .bind(tva)
    .bind(conditions)
    .fetch_one(pool)
    .await
    .unwrap();
    let (statut, _, _) = poster(s, &format!("/devis/{devis}/statut/accepter"), "date=2026-10-05").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let chantier: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(pool).await.unwrap();
    Base { devis, chantier, entreprise, tva, conditions }
}

/// Crée un avenant depuis la fiche du chantier et renvoie son id.
async fn nouvel_avenant(s: &Router, chantier: i64) -> i64 {
    let (statut, lieu, _) = poster(s, &format!("/chantiers/{chantier}/avenants"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let lieu = lieu.unwrap();
    let id = lieu.strip_prefix("/devis/").and_then(|r| r.strip_suffix("?ok=avenant")).expect("redirection vers l'éditeur");
    id.parse().unwrap()
}

/// Ajoute à l'avenant un poste libre « quantité 1 × prix ».
async fn ligne(pool: &PgPool, s: &Router, avenant: i64, prix: &str) {
    poster(s, &format!("/devis/{avenant}/lignes"), "genre=poste_libre").await;
    let ligne: i64 = sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 ORDER BY position DESC LIMIT 1")
        .bind(avenant)
        .fetch_one(pool)
        .await
        .unwrap();
    let (statut, _, _) = poster(
        s,
        &format!("/devis/{avenant}/lignes/{ligne}"),
        &format!("description=AVENANT%20N%C3%A9gatif&quantite=1&prix_unitaire={prix}"),
    )
    .await;
    assert_eq!(statut, StatusCode::OK);
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date::text").fetch_one(pool).await.unwrap()
}

async fn compteur_devis(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT valeur FROM compteurs WHERE nom = 'devis'").fetch_one(pool).await.unwrap()
}

// ─── Création ───────────────────────────────────────────────────────────────

#[sqlx::test]
async fn un_avenant_reprend_l_en_tete_du_devis_et_porte_le_numero_2(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let av = nouvel_avenant(&s, b.chantier).await;

    let (numero, chantier, statut, reference, client, tva, lignes): (i64, Option<i64>, String, String, Option<i64>, Option<i64>, i64) =
        sqlx::query_as(
            "SELECT numero, chantier_id, statut::text, reference, client_entreprise_id, taux_tva_id,
                    (SELECT count(*) FROM devis_lignes WHERE devis_id = devis.id)
             FROM devis WHERE id = $1",
        )
        .bind(av)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((numero, chantier, statut.as_str()), (2, Some(b.chantier), "brouillon"));
    assert_eq!((reference.as_str(), client, tva, lignes), ("PL45", Some(b.entreprise), Some(b.tva), 0));
    // La numérotation des devis n'est pas touchée.
    assert_eq!(compteur_devis(&pool).await, 57);

    let (statut, page) = lire(&s, &format!("/devis/{av}?ok=avenant")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Avenant n° 2"));
    assert!(page.contains("L&#39;avenant est créé en brouillon") || page.contains("L'avenant est créé en brouillon"));
    assert!(page.contains("Jours ouvrables supplémentaires"));
    assert!(page.contains(&format!("href=\"/chantiers/{}/devis\"", b.chantier)), "le bouton Retour mène au chantier");
    // Le devis de base, lui, n'a pas de champ de jours ouvrables.
    let (_, page) = lire(&s, &format!("/devis/{}", b.devis)).await;
    assert!(!page.contains("Jours ouvrables supplémentaires") && page.contains("Devis n° 2026-57"));

    // Les avenants suivants : 3, 4…
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT numero FROM devis WHERE id = $1").bind(nouvel_avenant(&s, b.chantier).await).fetch_one(&pool).await.unwrap(), 3);
    // Un autre chantier recommence à 2.
    let (_, _, _) = poster(&s, "/devis", "").await;
    let autre: i64 = sqlx::query_scalar("SELECT max(id) FROM devis WHERE chantier_id IS NULL").fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE devis SET statut = 'envoye', date_devis = '2026-10-01' WHERE id = $1").bind(autre).execute(&pool).await.unwrap();
    poster(&s, &format!("/devis/{autre}/statut/accepter"), "date=2026-10-05").await;
    let autre_chantier: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(autre).fetch_one(&pool).await.unwrap();
    let av_autre = nouvel_avenant(&s, autre_chantier).await;
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT numero FROM devis WHERE id = $1").bind(av_autre).fetch_one(&pool).await.unwrap(), 2);
}

#[sqlx::test]
async fn un_avenant_pour_un_chantier_inconnu_donne_404(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, _, _) = poster(&site(&pool), "/chantiers/999/avenants", "").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn les_avenants_ne_sont_pas_dans_la_liste_des_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let av = nouvel_avenant(&s, b.chantier).await;
    sqlx::query("UPDATE devis SET reference = 'Travaux non exécutés' WHERE id = $1").bind(av).execute(&pool).await.unwrap();
    let (_, page) = lire(&s, "/devis").await;
    assert!(page.contains("PL45") && !page.contains("Travaux non exécutés") && page.contains("rv-num\">1</span> devis"));
}

// ─── L'exemple réel ─────────────────────────────────────────────────────────

#[sqlx::test]
async fn l_exemple_reel_10384_13_moins_3780_36_donne_6603_77(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let av = nouvel_avenant(&s, b.chantier).await;
    ligne(&pool, &s, av, "-3780,36").await;
    let ht: i64 = sqlx::query_scalar("SELECT montant_ht_cents FROM devis WHERE id = $1").bind(av).fetch_one(&pool).await.unwrap();
    assert_eq!(ht, -378_036);

    // Pas encore accepté : il est listé, mais ne compte pas dans le total.
    let (_, page) = lire(&s, &format!("/chantiers/{}/devis", b.chantier)).await;
    assert!(page.contains("Avenant n° 2") && page.contains("-3\u{a0}780,36\u{a0}€"));
    assert!(page.contains("Total devis/avenants HT</td><td class=\"text-right whitespace-nowrap\">10\u{a0}384,13\u{a0}€"));

    let (statut, lieu, _) = poster(&s, &format!("/devis/{av}/statut/accepter"), &format!("date={}", aujourdhui(&pool).await)).await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{av}?ok=accepte"))));
    let (_, page) = lire(&s, &format!("/devis/{av}?ok=accepte")).await;
    assert!(page.contains("il compte maintenant dans le total du chantier") && page.contains("Chantier n° 1"));

    // Accepter un avenant ne crée pas de second chantier.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM chantiers").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);

    let (_, page) = lire(&s, &format!("/chantiers/{}/devis", b.chantier)).await;
    assert!(page.contains("Total devis/avenants HT</td><td class=\"text-right whitespace-nowrap\">6\u{a0}603,77\u{a0}€"));
    let (_, page) = lire(&s, &format!("/chantiers/{}", b.chantier)).await;
    assert!(page.contains("6\u{a0}603,77\u{a0}€"));
    let (_, page) = lire(&s, "/chantiers").await;
    assert!(page.contains("6\u{a0}603,77\u{a0}€"), "la liste des chantiers compte aussi les avenants acceptés");
}

// ─── Jours ouvrables, duplication, PDF ──────────────────────────────────────

#[sqlx::test]
async fn les_jours_ouvrables_s_enregistrent_et_se_totalisent(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let av = nouvel_avenant(&s, b.chantier).await;
    let formulaire = |jours: &str| {
        format!(
            "reference=Extension&responsable=Mahx&date_devis={}&client=e{}&taux_tva_id={}&conditions_paiement_id={}&facturation_identique=on&chantier_identique=on&jours_ouvrables={jours}",
            "2026-10-15", b.entreprise, b.tva, b.conditions
        )
    };
    for faux in ["-1", "1000", "2,5", "beaucoup"] {
        let (statut, _, page) = poster(&s, &format!("/devis/{av}"), &formulaire(faux)).await;
        assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY, "{faux}");
        assert!(page.contains("entre 0 et 999"), "{faux}");
    }
    let (statut, lieu, _) = poster(&s, &format!("/devis/{av}"), &formulaire("5")).await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{av}?ok=1"))));
    let jours: i32 = sqlx::query_scalar("SELECT jours_ouvrables FROM devis WHERE id = $1").bind(av).fetch_one(&pool).await.unwrap();
    assert_eq!(jours, 5);

    // Le devis de base ignore un champ de jours envoyé quand même.
    let (statut, _, _) = poster(&s, &format!("/devis/{}", b.devis), &formulaire("9")).await;
    assert_eq!(statut, StatusCode::CONFLICT, "un devis accepté ne se modifie plus");

    sqlx::query("UPDATE devis SET statut = 'accepte' WHERE id = $1").bind(av).execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/chantiers/{}/devis", b.chantier)).await;
    assert!(page.contains("supplémentaires (avenants acceptés) : <strong>5</strong>"));
}

#[sqlx::test]
async fn un_devis_ordinaire_n_a_jamais_de_jours_ouvrables(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    poster(&s, "/devis", "").await;
    let d: i64 = sqlx::query_scalar("SELECT max(id) FROM devis WHERE chantier_id IS NULL").fetch_one(&pool).await.unwrap();
    let corps = format!(
        "reference=X&responsable=Mahx&date_devis=2026-10-15&client=e{}&taux_tva_id={}&conditions_paiement_id={}&facturation_identique=on&chantier_identique=on&jours_ouvrables=12",
        b.entreprise, b.tva, b.conditions
    );
    let (statut, _, _) = poster(&s, &format!("/devis/{d}"), &corps).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let jours: i32 = sqlx::query_scalar("SELECT jours_ouvrables FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(jours, 0);
}

#[sqlx::test]
async fn dupliquer_un_avenant_le_garde_dans_son_chantier(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let av = nouvel_avenant(&s, b.chantier).await;
    ligne(&pool, &s, av, "120").await;
    sqlx::query("UPDATE devis SET jours_ouvrables = 3 WHERE id = $1").bind(av).execute(&pool).await.unwrap();

    let (statut, lieu, _) = poster(&s, &format!("/devis/{av}/dupliquer"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let copie: i64 = lieu.unwrap().trim_start_matches("/devis/").parse().unwrap();
    let (numero, chantier, jours, ht): (i64, Option<i64>, i32, i64) =
        sqlx::query_as("SELECT numero, chantier_id, jours_ouvrables, montant_ht_cents FROM devis WHERE id = $1").bind(copie).fetch_one(&pool).await.unwrap();
    assert_eq!((numero, chantier, jours, ht), (3, Some(b.chantier), 3, 12_000));
    assert_eq!(compteur_devis(&pool).await, 57, "la copie d'un avenant ne prend pas de numéro de devis");

    let journal: Vec<String> = sqlx::query_scalar("SELECT detail FROM devis_evenements WHERE devis_id = $1 ORDER BY id").bind(copie).fetch_all(&pool).await.unwrap();
    assert_eq!(journal, vec!["Avenant créé par duplication de l'avenant 2".to_string()]);
}

#[sqlx::test]
async fn le_pdf_d_un_avenant_porte_son_nom(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let av = nouvel_avenant(&s, b.chantier).await;
    let (statut, disposition, _) = envoyer(&s, Request::builder().uri(format!("/devis/{av}/pdf")).body(Body::empty()).unwrap()).await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(disposition.as_deref(), Some("inline; filename=\"Avenant-2.pdf\""));
}

#[sqlx::test]
async fn deux_avenants_ne_peuvent_avoir_le_meme_numero_dans_un_chantier(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let b = chantier_de_base(&pool, &s).await;
    let insert = "INSERT INTO devis (numero, chantier_id) VALUES (2, $1)";
    sqlx::query(insert).bind(b.chantier).execute(&pool).await.unwrap();
    assert!(sqlx::query(insert).bind(b.chantier).execute(&pool).await.is_err());
    // Mais un avenant n° 2 n'empêche pas un devis n° 2.
    sqlx::query("INSERT INTO devis (numero) VALUES (2)").execute(&pool).await.unwrap();
    assert!(sqlx::query("INSERT INTO devis (numero) VALUES (2)").execute(&pool).await.is_err());
}
