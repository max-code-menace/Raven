// Tests de la liste des devis et de la numérotation.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
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
    (statut, lieu, String::from_utf8(octets.to_vec()).unwrap())
}

async fn lire(site: &Router, uri: &str) -> String {
    let (statut, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(statut, StatusCode::OK, "GET {uri}");
    corps
}

async fn lire_htmx(site: &Router, uri: &str) -> String {
    let requete = Request::builder().uri(uri).header("hx-request", "true").body(Body::empty()).unwrap();
    envoyer(site, requete).await.2
}

async fn nouveau(site: &Router) -> (StatusCode, Option<String>) {
    let requete = Request::builder()
        .method("POST")
        .uri("/devis")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();
    let (s, lieu, _) = envoyer(site, requete).await;
    (s, lieu)
}

struct Devis<'a> {
    numero: i64,
    date: &'a str,
    reference: &'a str,
    statut: &'a str,
    entreprise: Option<i64>,
    contact: Option<i64>,
    ht: i64,
    ttc: i64,
}

impl Default for Devis<'_> {
    fn default() -> Self {
        Devis { numero: 1, date: "2026-01-15", reference: "", statut: "brouillon", entreprise: None, contact: None, ht: 0, ttc: 0 }
    }
}

async fn inserer(pool: &PgPool, d: Devis<'_>) {
    sqlx::query(
        "INSERT INTO devis (numero, date_devis, reference, statut, client_entreprise_id, client_contact_id,
                            montant_ht_cents, montant_ttc_cents)
         VALUES ($1, $2::date, $3, $4::text::statut_devis, $5, $6, $7, $8)",
    )
    .bind(d.numero)
    .bind(d.date)
    .bind(d.reference)
    .bind(d.statut)
    .bind(d.entreprise)
    .bind(d.contact)
    .bind(d.ht)
    .bind(d.ttc)
    .execute(pool)
    .await
    .unwrap();
}

async fn numeros(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar("SELECT numero FROM devis ORDER BY numero").fetch_all(pool).await.unwrap()
}

// ─── Création et numérotation ───────────────────────────────────────────────

#[sqlx::test]
async fn la_liste_est_vide_au_depart(pool: PgPool) {
    let page = lire(&site(&pool), "/devis").await;
    assert!(page.contains("Aucun devis trouvé."));
    assert!(page.contains("0 devis"));
    assert!(page.contains("0,00\u{a0}€"));
    assert!(page.contains("href=\"/devis\""));
}

#[sqlx::test]
async fn nouveau_cree_un_brouillon_numero_un(pool: PgPool) {
    let s = site(&pool);
    let (statut, lieu) = nouveau(&s).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu.as_deref(), Some("/devis"));

    let (numero, statut_devis, annee, reference): (i64, String, i32, String) = sqlx::query_as(
        "SELECT numero, statut::text, extract(year FROM date_devis)::int, reference FROM devis",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((numero, statut_devis.as_str(), reference.as_str()), (1, "brouillon", ""));

    let page = lire(&s, "/devis").await;
    assert!(page.contains(&format!("whitespace-nowrap\">{annee}-1</td>")));
    assert!(page.contains("Brouillon"));
    assert!(page.contains("1 devis"));
    // Un brouillon n'affiche pas de date de statut.
    assert!(!page.contains("text-xs opacity-70"));
}

#[sqlx::test]
async fn les_numeros_se_suivent_et_ne_sont_jamais_reutilises(pool: PgPool) {
    let s = site(&pool);
    for _ in 0..3 {
        nouveau(&s).await;
    }
    assert_eq!(numeros(&pool).await, vec![1, 2, 3]);

    // On supprime le dernier : le prochain devis a le numéro 4, pas 3.
    sqlx::query("DELETE FROM devis WHERE numero = 3").execute(&pool).await.unwrap();
    nouveau(&s).await;
    assert_eq!(numeros(&pool).await, vec![1, 2, 4]);
    let compteur: i64 = sqlx::query_scalar("SELECT valeur FROM compteurs WHERE nom = 'devis'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, 4);
}

#[sqlx::test]
async fn douze_creations_en_meme_temps_donnent_douze_numeros_differents(pool: PgPool) {
    let s = site(&pool);
    let mut taches = Vec::new();
    for _ in 0..12 {
        let s = s.clone();
        taches.push(tokio::spawn(async move { nouveau(&s).await.0 }));
    }
    for t in taches {
        assert_eq!(t.await.unwrap(), StatusCode::SEE_OTHER);
    }
    assert_eq!(numeros(&pool).await, (1..=12).collect::<Vec<i64>>());
}

#[sqlx::test]
async fn une_creation_annulee_ne_consomme_pas_de_numero(pool: PgPool) {
    // Même mécanisme que le site : si la transaction est annulée, le compteur revient en arrière.
    let mut tx = pool.begin().await.unwrap();
    let n: i64 = sqlx::query_scalar("UPDATE compteurs SET valeur = valeur + 1 WHERE nom = 'devis' RETURNING valeur")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 1);
    tx.rollback().await.unwrap();

    nouveau(&site(&pool)).await;
    assert_eq!(numeros(&pool).await, vec![1]);
}

#[sqlx::test]
async fn un_devis_ne_peut_pas_avoir_deux_clients(pool: PgPool) {
    let (e,): (i64,) = sqlx::query_as("INSERT INTO entreprises (nom) VALUES ('E') RETURNING id").fetch_one(&pool).await.unwrap();
    let (c,): (i64,) = sqlx::query_as("INSERT INTO contacts (nom) VALUES ('C') RETURNING id").fetch_one(&pool).await.unwrap();
    let res = sqlx::query("INSERT INTO devis (numero, client_entreprise_id, client_contact_id) VALUES (1, $1, $2)")
        .bind(e)
        .bind(c)
        .execute(&pool)
        .await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn deux_devis_ne_peuvent_pas_avoir_le_meme_numero(pool: PgPool) {
    inserer(&pool, Devis { numero: 5, ..Default::default() }).await;
    let res = sqlx::query("INSERT INTO devis (numero) VALUES (5)").execute(&pool).await;
    assert!(res.is_err());
}

// ─── Affichage ──────────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_liste_montre_client_montant_statut_et_date(pool: PgPool) {
    let (e,): (i64,) = sqlx::query_as("INSERT INTO entreprises (nom) VALUES ('Dupont SRL') RETURNING id").fetch_one(&pool).await.unwrap();
    let (c,): (i64,) = sqlx::query_as("INSERT INTO contacts (prenom, nom) VALUES ('Jean', 'Martin') RETURNING id").fetch_one(&pool).await.unwrap();
    inserer(&pool, Devis { numero: 1, date: "2026-10-14", reference: "F28", statut: "accepte", entreprise: Some(e), ht: 443_184, ttc: 469_775, ..Default::default() }).await;
    inserer(&pool, Devis { numero: 2, date: "2026-10-15", reference: "C44 Volet", statut: "envoye", contact: Some(c), ht: 373_126, ttc: 395_513, ..Default::default() }).await;

    let page = lire(&site(&pool), "/devis").await;
    assert!(page.contains("Dupont SRL") && page.contains("Jean Martin"));
    assert!(page.contains("F28") && page.contains("C44 Volet"));
    assert!(page.contains("4\u{a0}431,84\u{a0}€") && page.contains("3\u{a0}731,26\u{a0}€"));
    assert!(page.contains("14/10/2026") && page.contains("15/10/2026"));
    assert!(page.contains("Accepté") && page.contains("Envoyé"));
    assert!(page.contains("2026-1</td>") && page.contains("2026-2</td>"));

    // Sous le statut : la date du changement de statut (aujourd'hui, puisqu'on vient de l'insérer).
    let aujourdhui: String = sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'Europe/Brussels', 'DD/MM/YYYY')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(page.contains(&format!("<span class=\"text-xs opacity-70\">{aujourdhui}</span>")));
}

#[sqlx::test]
async fn les_plus_recents_sont_en_premier(pool: PgPool) {
    inserer(&pool, Devis { numero: 1, date: "2026-03-01", reference: "ANCIEN", ..Default::default() }).await;
    inserer(&pool, Devis { numero: 2, date: "2026-05-01", reference: "RECENT", ..Default::default() }).await;
    inserer(&pool, Devis { numero: 3, date: "2026-05-01", reference: "RECENT-BIS", ..Default::default() }).await;
    let page = lire(&site(&pool), "/devis").await;
    let place = |m: &str| page.find(m).unwrap_or_else(|| panic!("{m} absent"));
    assert!(place("RECENT-BIS") < place("RECENT</td>"));
    assert!(place("RECENT</td>") < place("ANCIEN"));
}

#[sqlx::test]
async fn la_reference_tapee_est_neutralisee(pool: PgPool) {
    inserer(&pool, Devis { reference: "<script>alert(1)</script>", ..Default::default() }).await;
    let page = lire(&site(&pool), "/devis").await;
    assert!(!page.contains("<script>alert(1)"));
    assert!(page.contains("&#60;script&#62;alert(1)"));
}

// ─── Totaux en pied de liste ────────────────────────────────────────────────

#[sqlx::test]
async fn les_totaux_suivent_les_filtres(pool: PgPool) {
    inserer(&pool, Devis { numero: 1, statut: "accepte", ht: 100_000, ttc: 106_000, ..Default::default() }).await;
    inserer(&pool, Devis { numero: 2, statut: "envoye", ht: 200_000, ttc: 242_000, ..Default::default() }).await;
    let s = site(&pool);

    let tout = lire(&s, "/devis").await;
    assert!(tout.contains("<strong>3\u{a0}000,00\u{a0}€</strong>"));
    assert!(tout.contains("<strong>3\u{a0}480,00\u{a0}€</strong>"));
    assert!(tout.contains("2 devis"));

    let acceptes = lire(&s, "/devis?statut=accepte").await;
    assert!(acceptes.contains("<strong>1\u{a0}000,00\u{a0}€</strong>"));
    assert!(acceptes.contains("<strong>1\u{a0}060,00\u{a0}€</strong>"));
    assert!(acceptes.contains("1 devis"));
}

// ─── Filtres et recherche ───────────────────────────────────────────────────

#[sqlx::test]
async fn filtre_par_un_ou_plusieurs_statuts(pool: PgPool) {
    for (n, st) in [(1, "brouillon"), (2, "accepte"), (3, "envoye"), (4, "refuse")] {
        inserer(&pool, Devis { numero: n, statut: st, reference: &format!("REF{n}"), ..Default::default() }).await;
    }
    let s = site(&pool);
    let page = lire(&s, "/devis?statut=accepte").await;
    assert!(page.contains("REF2") && !page.contains("REF1") && !page.contains("REF3"));
    assert!(page.contains("value=\"accepte\" checked"));

    let page = lire(&s, "/devis?statut=accepte&statut=refuse").await;
    assert!(page.contains("REF2") && page.contains("REF4") && !page.contains("REF3"));

    // Un statut inconnu est ignoré : on voit tout.
    let page = lire(&s, "/devis?statut=nimporte").await;
    assert!(page.contains("REF1") && page.contains("REF2") && page.contains("REF3") && page.contains("REF4"));
}

#[sqlx::test]
async fn filtre_par_dates(pool: PgPool) {
    inserer(&pool, Devis { numero: 1, date: "2026-01-10", reference: "JANVIER", ..Default::default() }).await;
    inserer(&pool, Devis { numero: 2, date: "2026-02-10", reference: "FEVRIER", ..Default::default() }).await;
    inserer(&pool, Devis { numero: 3, date: "2026-03-10", reference: "MARS", ..Default::default() }).await;
    let s = site(&pool);

    let page = lire(&s, "/devis?du=2026-02-01&au=2026-02-28").await;
    assert!(page.contains("FEVRIER") && !page.contains("JANVIER") && !page.contains("MARS"));
    let page = lire(&s, "/devis?du=2026-02-10").await;
    assert!(page.contains("FEVRIER") && page.contains("MARS") && !page.contains("JANVIER"));
    let page = lire(&s, "/devis?au=2026-02-10").await;
    assert!(page.contains("JANVIER") && page.contains("FEVRIER") && !page.contains("MARS"));
    // Les bornes sont incluses, et une date absurde est ignorée.
    let page = lire(&s, "/devis?du=pas-une-date&au=2026-13-45").await;
    assert!(page.contains("JANVIER") && page.contains("MARS"));
}

#[sqlx::test]
async fn recherche_par_reference_client_et_numero(pool: PgPool) {
    let (e,): (i64,) = sqlx::query_as("INSERT INTO entreprises (nom) VALUES ('Batiroc SA') RETURNING id").fetch_one(&pool).await.unwrap();
    let (c,): (i64,) = sqlx::query_as("INSERT INTO contacts (prenom, nom) VALUES ('Anne', 'Lambert') RETURNING id").fetch_one(&pool).await.unwrap();
    inserer(&pool, Devis { numero: 1, reference: "TOITURE", entreprise: Some(e), ..Default::default() }).await;
    inserer(&pool, Devis { numero: 2, reference: "FACADE", contact: Some(c), ..Default::default() }).await;
    inserer(&pool, Devis { numero: 3, reference: "CAVE", ..Default::default() }).await;
    let s = site(&pool);

    let page = lire(&s, "/devis?q=toit").await;
    assert!(page.contains("TOITURE") && !page.contains("FACADE") && !page.contains("CAVE"));
    let page = lire(&s, "/devis?q=BATIROC").await;
    assert!(page.contains("TOITURE") && !page.contains("FACADE"));
    let page = lire(&s, "/devis?q=anne+lambert").await;
    assert!(page.contains("FACADE") && !page.contains("TOITURE"));
    let page = lire(&s, "/devis?q=2026-3").await;
    assert!(page.contains("CAVE") && !page.contains("TOITURE") && !page.contains("FACADE"));
    let page = lire(&s, "/devis?q=zzz").await;
    assert!(page.contains("Aucun devis trouvé."));
}

#[sqlx::test]
async fn les_jokers_de_la_recherche_sont_neutralises(pool: PgPool) {
    inserer(&pool, Devis { reference: "ABC", ..Default::default() }).await;
    let s = site(&pool);
    assert!(lire(&s, "/devis?q=%25").await.contains("Aucun devis trouvé."));
    assert!(lire(&s, "/devis?q=A_C").await.contains("Aucun devis trouvé."));
}

#[sqlx::test]
async fn recherche_et_filtres_se_combinent(pool: PgPool) {
    inserer(&pool, Devis { numero: 1, reference: "TOIT-A", statut: "accepte", date: "2026-02-01", ..Default::default() }).await;
    inserer(&pool, Devis { numero: 2, reference: "TOIT-B", statut: "envoye", date: "2026-02-01", ..Default::default() }).await;
    inserer(&pool, Devis { numero: 3, reference: "CAVE", statut: "accepte", date: "2026-02-01", ..Default::default() }).await;
    let page = lire(&site(&pool), "/devis?q=toit&statut=accepte&du=2026-01-01").await;
    assert!(page.contains("TOIT-A") && !page.contains("TOIT-B") && !page.contains("CAVE"));
}

// ─── Pagination et htmx ─────────────────────────────────────────────────────

async fn vingt_cinq_devis(pool: &PgPool) {
    sqlx::query("UPDATE compteurs SET valeur = 25 WHERE nom = 'devis'").execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO devis (numero, date_devis, statut, montant_ht_cents, montant_ttc_cents)
         SELECT n, '2026-01-15', 'accepte', 100000, 121000 FROM generate_series(1, 25) n",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test]
async fn pagination_vingt_par_page_et_totaux_sur_tout_le_filtre(pool: PgPool) {
    vingt_cinq_devis(&pool).await;
    let s = site(&pool);
    let cellule = |n: i64| format!("whitespace-nowrap\">2026-{n}</td>");

    let p1 = lire(&s, "/devis").await;
    assert!(p1.contains(&cellule(25)) && p1.contains(&cellule(6)) && !p1.contains(&cellule(5)));
    assert!(p1.contains("25 devis") && p1.contains("Page 1 / 2"));
    // Les totaux couvrent les 25 devis, pas seulement la page affichée.
    assert!(p1.contains("<strong>25\u{a0}000,00\u{a0}€</strong>") && p1.contains("<strong>30\u{a0}250,00\u{a0}€</strong>"));

    let p2 = lire(&s, "/devis?page=2").await;
    assert!(p2.contains(&cellule(5)) && p2.contains(&cellule(1)) && !p2.contains(&cellule(6)));
    assert!(p2.contains("Page 2 / 2"));

    assert!(lire(&s, "/devis?page=99").await.contains("Page 2 / 2"));
    assert!(lire(&s, "/devis?page=0").await.contains("Page 1 / 2"));
    assert!(lire(&s, "/devis?page=abc").await.contains("Page 1 / 2"));
}

#[sqlx::test]
async fn les_liens_de_page_gardent_les_filtres(pool: PgPool) {
    vingt_cinq_devis(&pool).await;
    let page = lire(&site(&pool), "/devis?statut=accepte&du=2026-01-01").await;
    // (Askama écrit « & » sous la forme « &#38; » dans la partie variable ; le navigateur lit la même chose.)
    assert!(page.contains("href=\"/devis?q=&#38;statut=accepte&#38;du=2026-01-01&#38;au=&page=2\""));
    assert!(!page.contains("page=1\""), "pas de lien « page précédente » sur la première page");
}

#[sqlx::test]
async fn htmx_recoit_seulement_le_tableau(pool: PgPool) {
    inserer(&pool, Devis { reference: "UNIQUE", ..Default::default() }).await;
    let s = site(&pool);
    let complet = lire(&s, "/devis").await;
    let partiel = lire_htmx(&s, "/devis").await;
    assert!(complet.contains("<html") && complet.contains("name=\"q\""));
    assert!(!partiel.contains("<html") && !partiel.contains("name=\"q\""));
    assert!(partiel.contains("UNIQUE") && partiel.contains("Total HT"));
}
