// Refonte du design, étape 4-2 : type de quantité (QF, QP, Fft) et métré dans la ligne.

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

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, String) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, String::from_utf8(octets.to_vec()).unwrap())
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from(corps.to_string()))
        .unwrap();
    envoyer(site, requete).await
}

/// Un brouillon avec un taux de TVA par défaut (6 %), comme après l'onglet Informations.
async fn brouillon(pool: &PgPool) -> i64 {
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(pool).await.unwrap();
    sqlx::query_scalar("INSERT INTO devis (numero, taux_tva_id) VALUES (1, $1) RETURNING id").bind(tva).fetch_one(pool).await.unwrap()
}

async fn ajouter(s: &Router, devis: i64, genre: &str) -> StatusCode {
    poster(s, &format!("/devis/{devis}/lignes"), &format!("genre={genre}")).await.0
}

async fn ids(pool: &PgPool, devis: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 ORDER BY position").bind(devis).fetch_all(pool).await.unwrap()
}

async fn montants(pool: &PgPool, devis: i64) -> (i64, i64) {
    sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(devis).fetch_one(pool).await.unwrap()
}

async fn mesures(pool: &PgPool, ligne: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT id FROM devis_metres WHERE ligne_id = $1 ORDER BY position").bind(ligne).fetch_all(pool).await.unwrap()
}

async fn quantite(pool: &PgPool, ligne: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT quantite_e5 FROM devis_lignes WHERE id = $1").bind(ligne).fetch_one(pool).await.unwrap()
}

async fn type_quantite(pool: &PgPool, ligne: i64) -> String {
    sqlx::query_scalar("SELECT type_quantite::text FROM devis_lignes WHERE id = $1").bind(ligne).fetch_one(pool).await.unwrap()
}

/// Un brouillon avec un poste libre : 10 × 25,00 € à 6 %.
async fn devis_avec_poste(pool: &PgPool, s: &Router) -> (i64, i64) {
    let d = brouillon(pool).await;
    assert_eq!(ajouter(s, d, "poste_libre").await, StatusCode::OK);
    let l = ids(pool, d).await[0];
    poster(s, &format!("/devis/{d}/lignes/{l}"), "quantite=10&prix_unitaire=25").await;
    (d, l)
}

#[sqlx::test]
async fn une_ligne_nait_en_quantite_fixe_et_change_de_type(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (d, l) = devis_avec_poste(&pool, &s).await;
    assert_eq!(type_quantite(&pool, l).await, "fixe");
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("name=\"type_quantite\""));
    assert!(page.contains("<option value=\"fixe\" title=\"Quantité fixe : La quantité du devis est celle qui sera facturée.\" selected>QF</option>"));
    assert!(page.contains(">QP</option>") && page.contains(">Fft</option>"));

    let (statut, _) = poster(&s, &format!("/devis/{d}/lignes/{l}"), "type_quantite=estimee").await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(type_quantite(&pool, l).await, "estimee");
    // Le type ne change aucun montant.
    assert_eq!(montants(&pool, d).await, (25_000, 26_500));

    let (_, reponse) = poster(&s, &format!("/devis/{d}/lignes/{l}"), "type_quantite=autre").await;
    assert!(reponse.contains("Ce type de quantité n&#39;existe pas."));
    assert_eq!(type_quantite(&pool, l).await, "estimee");
}

#[sqlx::test]
async fn le_metre_devient_la_quantite_de_la_ligne(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (d, l) = devis_avec_poste(&pool, &s).await;

    // La première mesure reprend la quantité : rien ne change.
    let (statut, bloc) = poster(&s, &format!("/devis/{d}/lignes/{l}/mesures"), "").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(bloc.contains("rv-qt-msr") && bloc.contains("aria-expanded=\"true\"") && bloc.contains("reprise du métré"));
    let m = mesures(&pool, l).await;
    assert_eq!(m.len(), 1);
    assert_eq!(quantite(&pool, l).await, Some(1_000_000));
    assert_eq!(montants(&pool, d).await, (25_000, 26_500));

    // Mur A 2,48 × 2,50 = 6,2 ; mur B 2,50 × 2,50 = 6,25 ; fenêtre à déduire 0,60 × 0,58 = 0,35.
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}", m[0]), "libelle=Mur+A&longueur=2,48&largeur=2,5").await;
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures"), "").await;
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures"), "").await;
    let m = mesures(&pool, l).await;
    assert_eq!(m.len(), 3);
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}", m[1]), "libelle=Mur+B&longueur=2,5&largeur=2,5").await;
    let (statut, maj) =
        poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}", m[2]), "libelle=Fen%C3%AAtre&longueur=0,6&largeur=0,58&deduction=on").await;
    assert_eq!(statut, StatusCode::OK);
    // 6,2 + 6,25 − 0,35 = 12,1 m² × 25,00 € = 302,50 € HT, TVA 6 % 18,15 €.
    assert_eq!(quantite(&pool, l).await, Some(1_210_000));
    assert_eq!(montants(&pool, d).await, (30_250, 32_065));
    assert!(maj.contains(&format!("<span id=\"resultat-{}\" hx-swap-oob=\"true\">-0,35</span>", m[2])));
    assert!(maj.contains(&format!("<span id=\"qte-{l}\" hx-swap-oob=\"true\">12,1</span>")));
    assert!(maj.contains(&format!("<span id=\"metre-somme-{l}\" hx-swap-oob=\"true\">12,1</span>")));

    // Avec un métré, la quantité tapée à la main n'est pas prise.
    poster(&s, &format!("/devis/{d}/lignes/{l}"), "quantite=99").await;
    assert_eq!(quantite(&pool, l).await, Some(1_210_000));

    // Une mesure fausse n'est pas enregistrée et le dit.
    let (_, maj) = poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}", m[1]), "longueur=-2&largeur=2,5").await;
    assert!(maj.contains("cochez « À déduire »"));
    assert_eq!(quantite(&pool, l).await, Some(1_210_000));

    // Replié, la quantité reste un bouton qui rouvre le métré ; déplié par l'adresse, seul le bloc revient à htmx.
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains(&format!("hx-get=\"/devis/{d}/encodage?metre={l}\"")) && !page.contains("reprise du métré"));
    let requete = Request::builder().uri(format!("/devis/{d}/encodage?metre={l}")).header("hx-request", "true").body(Body::empty()).unwrap();
    let (_, bloc) = envoyer(&s, requete).await;
    assert!(bloc.starts_with("<div id=\"encodage\"") || bloc.trim_start().starts_with("<div id=\"encodage\""));
    assert!(bloc.contains("Mur A") && bloc.contains("rv-qt-msr-sum") && !bloc.contains("<html"));

    // Retirer une mesure recalcule ; retirer la dernière garde la quantité.
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}/supprimer", m[2]), "").await;
    assert_eq!(quantite(&pool, l).await, Some(1_245_000));
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}/supprimer", m[1]), "").await;
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{}/supprimer", m[0]), "").await;
    assert!(mesures(&pool, l).await.is_empty());
    assert_eq!(quantite(&pool, l).await, Some(620_000));
    poster(&s, &format!("/devis/{d}/lignes/{l}"), "quantite=3").await;
    assert_eq!(quantite(&pool, l).await, Some(300_000));
}

#[sqlx::test]
async fn un_titre_n_a_pas_de_metre_et_un_devis_envoye_ne_se_mesure_plus(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (d, l) = devis_avec_poste(&pool, &s).await;
    ajouter(&s, d, "titre").await;
    let titre = ids(&pool, d).await[1];
    assert_eq!(poster(&s, &format!("/devis/{d}/lignes/{titre}/mesures"), "").await.0, StatusCode::CONFLICT);
    // Une ligne d'un autre devis : introuvable.
    let autre: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (2) RETURNING id").fetch_one(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/devis/{autre}/lignes/{l}/mesures"), "").await.0, StatusCode::NOT_FOUND);

    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures"), "").await;
    let m = mesures(&pool, l).await[0];
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/devis/{d}/lignes/{l}/mesures"), "").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{m}"), "longueur=5").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{m}/supprimer"), "").await.0, StatusCode::CONFLICT);
    assert_eq!(quantite(&pool, l).await, Some(1_000_000));
    // Le métré se lit encore, sans champ modifiable ni bouton.
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage?metre={l}")).await;
    assert!(page.contains("reprise du métré") && !page.contains("/mesures\""));
    assert!(page.contains(&format!("id=\"mesure-{m}-longueur\"")) && page.contains("name=\"longueur\" value=\"10\" disabled"));
}

#[sqlx::test]
async fn dupliquer_garde_le_type_et_le_metre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (d, l) = devis_avec_poste(&pool, &s).await;
    poster(&s, &format!("/devis/{d}/lignes/{l}"), "type_quantite=forfait").await;
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures"), "").await;
    let m = mesures(&pool, l).await[0];
    poster(&s, &format!("/devis/{d}/lignes/{l}/mesures/{m}"), "libelle=Sol&longueur=4&largeur=3").await;
    // Le compteur des devis n'a pas servi pour ce brouillon d'essai : la copie prend le n° 1, l'original passe au n° 500.
    sqlx::query("UPDATE devis SET numero = 500 WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/devis/{d}/dupliquer"), "").await.0, StatusCode::SEE_OTHER);
    let copie: i64 = sqlx::query_scalar("SELECT max(id) FROM devis").fetch_one(&pool).await.unwrap();
    assert_ne!(copie, d);
    let nl = ids(&pool, copie).await[0];
    assert_eq!(type_quantite(&pool, nl).await, "forfait");
    assert_eq!(quantite(&pool, nl).await, Some(1_200_000));
    let copies: Vec<(String, i64, Option<i64>, bool)> =
        sqlx::query_as("SELECT libelle, longueur_e5, largeur_e5, deduction FROM devis_metres WHERE ligne_id = $1").bind(nl).fetch_all(&pool).await.unwrap();
    assert_eq!(copies, vec![("Sol".to_string(), 400_000, Some(300_000), false)]);
    // Supprimer la ligne emporte son métré.
    poster(&s, &format!("/devis/{copie}/lignes/{nl}/supprimer"), "").await;
    assert!(mesures(&pool, nl).await.is_empty());
}

#[sqlx::test]
async fn les_unites_proposees_suivent_le_metier(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (d, _) = devis_avec_poste(&pool, &s).await;
    sqlx::query("UPDATE organisations SET profil_metier = 'batiment' WHERE id = 1").execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains(&format!("list=\"unites-{d}\"")) && page.contains(&format!("<datalist id=\"unites-{d}\">")));
    assert!(page.contains("<option value=\"m³\">") && page.contains("<option value=\"sac\">") && !page.contains("<option value=\"page\">"));
    sqlx::query("UPDATE organisations SET profil_metier = 'graphisme' WHERE id = 1").execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("<option value=\"page\">") && !page.contains("<option value=\"m³\">"));
}
