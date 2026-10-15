// Tests de l'étape 6-1 : le chantier naît de l'acceptation du devis ; liste et fiche.

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
    (statut, lieu, String::from_utf8_lossy(&octets).to_string())
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, corps)
}

async fn lire_htmx(site: &Router, uri: &str) -> String {
    let requete = Request::builder().uri(uri).header("hx-request", "true").body(Body::empty()).unwrap();
    envoyer(site, requete).await.2
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let (s, lieu, _) = envoyer(site, requete).await;
    (s, lieu)
}

/// Un devis envoyé, daté du 1er octobre 2026, pour un client entreprise avec une adresse.
async fn devis(pool: &PgPool, numero: i64, reference: &str, client: &str) -> i64 {
    let entreprise: i64 = sqlx::query_scalar(
        "INSERT INTO entreprises (nom, rue, numero, code_postal, ville) VALUES ($1, 'Rue des Tests', '12', '1000', 'Bruxelles') RETURNING id",
    )
    .bind(client)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, statut, reference, client_entreprise_id, montant_ht_cents, montant_ttc_cents)
         VALUES ($1, '2026-10-01', 'envoye', $2, $3, 1038413, 1100718) RETURNING id",
    )
    .bind(numero)
    .bind(reference)
    .bind(entreprise)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn accepter(s: &Router, devis: i64, date: &str) {
    let (statut, lieu) = poster(s, &format!("/devis/{devis}/statut/accepter"), &format!("date={date}")).await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{devis}?ok=accepte"))));
}

async fn chantier_de(pool: &PgPool, devis: i64) -> Option<(i64, i64, String, String)> {
    sqlx::query_as("SELECT id, numero, statut::text, date_debut::text FROM chantiers WHERE devis_id = $1")
        .bind(devis)
        .fetch_optional(pool)
        .await
        .unwrap()
}

// ─── Création à l'acceptation ───────────────────────────────────────────────

#[sqlx::test]
async fn accepter_un_devis_cree_son_chantier(pool: PgPool) {
    let d = devis(&pool, 57, "L63", "Client A").await;
    let s = site(&pool);
    assert_eq!(chantier_de(&pool, d).await, None);

    accepter(&s, d, "2026-10-05").await;
    let (id, numero, statut, debut) = chantier_de(&pool, d).await.expect("le chantier existe");
    assert_eq!((numero, statut.as_str(), debut.as_str()), (1, "en_cours", "2026-10-05"));

    // Le devis montre le lien vers son chantier.
    let (_, page) = lire(&s, &format!("/devis/{d}")).await;
    assert!(page.contains(&format!("href=\"/chantiers/{id}\"")) && page.contains("Chantier n° 1"));
}

#[sqlx::test]
async fn les_numeros_de_chantier_se_suivent(pool: PgPool) {
    let s = site(&pool);
    let a = devis(&pool, 1, "A", "Client A").await;
    let b = devis(&pool, 2, "B", "Client B").await;
    accepter(&s, b, "2026-10-02").await;
    accepter(&s, a, "2026-10-03").await;
    assert_eq!(chantier_de(&pool, b).await.unwrap().1, 1);
    assert_eq!(chantier_de(&pool, a).await.unwrap().1, 2);
    let (compteur,): (i64,) = sqlx::query_as("SELECT valeur FROM compteurs WHERE nom = 'chantier'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, 2);
}

#[sqlx::test]
async fn refuser_ou_une_date_invalide_ne_cree_pas_de_chantier(pool: PgPool) {
    let s = site(&pool);
    let a = devis(&pool, 1, "A", "Client A").await;
    let (_, lieu) = poster(&s, &format!("/devis/{a}/statut/accepter"), "date=2026-09-01").await;
    assert_eq!(lieu, Some(format!("/devis/{a}?erreur=date")));
    poster(&s, &format!("/devis/{a}/statut/refuser"), "date=2026-10-02").await;
    assert_eq!(chantier_de(&pool, a).await, None);
    // Aucun numéro consommé.
    let (compteur,): (i64,) = sqlx::query_as("SELECT valeur FROM compteurs WHERE nom = 'chantier'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, 0);
}

#[sqlx::test]
async fn un_devis_ne_donne_qu_un_seul_chantier(pool: PgPool) {
    let d = devis(&pool, 1, "A", "Client A").await;
    sqlx::query("INSERT INTO chantiers (numero, devis_id, date_debut) VALUES (1, $1, '2026-10-01')").bind(d).execute(&pool).await.unwrap();
    let res = sqlx::query("INSERT INTO chantiers (numero, devis_id, date_debut) VALUES (2, $1, '2026-10-01')").bind(d).execute(&pool).await;
    assert!(res.is_err());
}

// ─── Liste ──────────────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_liste_montre_les_chantiers_en_cours_par_defaut(pool: PgPool) {
    let s = site(&pool);
    let a = devis(&pool, 1, "PL45", "Client A").await;
    let b = devis(&pool, 2, "C44", "Client B").await;
    accepter(&s, a, "2026-10-02").await;
    accepter(&s, b, "2026-10-03").await;
    let (id_b, ..) = chantier_de(&pool, b).await.unwrap();
    poster(&s, &format!("/chantiers/{id_b}"), "statut=termine&date_debut=2026-10-03").await;

    let (statut, page) = lire(&s, "/chantiers").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("PL45") && page.contains("Client A") && page.contains("Rue des Tests 12, 1000 Bruxelles"));
    assert!(page.contains("02/10/2026") && page.contains("10\u{a0}384,13\u{a0}€"));
    assert!(!page.contains("C44"), "un chantier terminé n'est pas dans le filtre par défaut");

    let page = lire_htmx(&s, "/chantiers?statut=termine").await;
    assert!(page.contains("C44") && !page.contains("PL45"));
    assert!(!page.contains("<html"), "htmx ne reçoit que le tableau");

    let page = lire_htmx(&s, "/chantiers?statut=tous").await;
    assert!(page.contains("C44") && page.contains("PL45") && page.contains("2 chantiers"));

    let page = lire_htmx(&s, "/chantiers?statut=tous&q=client%20b").await;
    assert!(page.contains("C44") && !page.contains("PL45"));
    let page = lire_htmx(&s, "/chantiers?statut=tous&q=2").await;
    assert!(page.contains("C44") && !page.contains("PL45"), "recherche par numéro de chantier");
}

#[sqlx::test]
async fn la_liste_vide_le_dit(pool: PgPool) {
    let (statut, page) = lire(&site(&pool), "/chantiers?statut=bidon&q=%25").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Aucun chantier trouvé."));
}

// ─── Fiche ──────────────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_fiche_reprend_le_devis(pool: PgPool) {
    let s = site(&pool);
    let d = devis(&pool, 57, "L63", "Client A").await;
    accepter(&s, d, "2026-10-05").await;
    let (id, ..) = chantier_de(&pool, d).await.unwrap();

    let (statut, page) = lire(&s, &format!("/chantiers/{id}")).await;
    assert_eq!(statut, StatusCode::OK);
    for attendu in ["Chantier N° 1", "En cours", "L63", "Client A", "Rue des Tests 12, 1000 Bruxelles", "N° 2026-57", "accepté le 05/10/2026", "10\u{a0}384,13\u{a0}€"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(page.contains(&format!("href=\"/devis/{d}\"")));

    let (statut, page) = lire(&s, &format!("/chantiers/{id}/devis")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Devis de base") && page.contains("Total devis/avenants HT") && page.contains("11\u{a0}007,18\u{a0}€"));
}

#[sqlx::test]
async fn l_adresse_propre_au_devis_l_emporte(pool: PgPool) {
    let s = site(&pool);
    let d = devis(&pool, 1, "A", "Client A").await;
    sqlx::query("UPDATE devis SET chantier_identique = false, chantier_rue = 'Chaussée de Mons', chantier_numero = '3', chantier_code_postal = '7000', chantier_ville = 'Mons' WHERE id = $1")
        .bind(d)
        .execute(&pool)
        .await
        .unwrap();
    accepter(&s, d, "2026-10-05").await;
    let (id, ..) = chantier_de(&pool, d).await.unwrap();
    let (_, page) = lire(&s, &format!("/chantiers/{id}")).await;
    assert!(page.contains("Chaussée de Mons 3, 7000 Mons") && !page.contains("Rue des Tests"));
}

#[sqlx::test]
async fn changer_le_statut_et_la_date_de_debut(pool: PgPool) {
    let s = site(&pool);
    let d = devis(&pool, 1, "A", "Client A").await;
    accepter(&s, d, "2026-10-05").await;
    let (id, ..) = chantier_de(&pool, d).await.unwrap();

    let (statut, lieu) = poster(&s, &format!("/chantiers/{id}"), "statut=termine&date_debut=2026-10-12").await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/chantiers/{id}?ok=1"))));
    let (_, _, st, debut) = chantier_de(&pool, d).await.unwrap();
    assert_eq!((st.as_str(), debut.as_str()), ("termine", "2026-10-12"));
    let (_, page) = lire(&s, &format!("/chantiers/{id}?ok=1")).await;
    assert!(page.contains("Le chantier est enregistré.") && page.contains("Terminé") && page.contains("12/10/2026"));

    // Valeurs refusées : rien ne change.
    let (_, lieu) = poster(&s, &format!("/chantiers/{id}"), "statut=perdu&date_debut=2026-10-12").await;
    assert_eq!(lieu, Some(format!("/chantiers/{id}?erreur=statut")));
    let (_, lieu) = poster(&s, &format!("/chantiers/{id}"), "statut=en_cours&date_debut=2026-13-45").await;
    assert_eq!(lieu, Some(format!("/chantiers/{id}?erreur=date")));
    let (_, _, st, debut) = chantier_de(&pool, d).await.unwrap();
    assert_eq!((st.as_str(), debut.as_str()), ("termine", "2026-10-12"));
    let (_, page) = lire(&s, &format!("/chantiers/{id}?erreur=date")).await;
    assert!(page.contains("La date de début n&#39;est pas valide.") || page.contains("La date de début n'est pas valide."));
}

#[sqlx::test]
async fn un_chantier_inconnu_donne_404(pool: PgPool) {
    let s = site(&pool);
    assert_eq!(lire(&s, "/chantiers/999").await.0, StatusCode::NOT_FOUND);
    assert_eq!(lire(&s, "/chantiers/999/devis").await.0, StatusCode::NOT_FOUND);
    assert_eq!(poster(&s, "/chantiers/999", "statut=termine&date_debut=2026-10-12").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn le_menu_mene_aux_chantiers(pool: PgPool) {
    let (_, page) = lire(&site(&pool), "/devis").await;
    assert!(page.contains("href=\"/chantiers\""));
}
