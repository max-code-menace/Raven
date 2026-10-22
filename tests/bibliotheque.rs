// Tests de l'étape 9-3 : bibliothèque de prix (ouvrages) et insertion d'un ouvrage dans un devis.

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

const PANNEAUX: &str = "description=Panneaux+isolants+PIR+12+cm&genre=fourniture&categorie=Toiture&fournisseur=Brico+Pro&unite=m%C2%B2&prix_achat=28&marge=37%2C5&prix_vente=&remarque=Pose+comprise";

#[sqlx::test]
async fn creer_chercher_modifier_supprimer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (code, page) = lire(&s, "/bibliotheque").await;
    assert_eq!(code, StatusCode::OK);
    assert!(page.contains("Aucun ouvrage trouvé.") && page.contains("href=\"/bibliotheque\"") && !page.contains("Bibliothèque<small>"));
    assert_eq!(lire(&s, "/bibliotheque/nouveau").await.0, StatusCode::OK);

    // Prix de vente calculé depuis l'achat et la marge : 28 € avec 37,5 % de marge sur le prix de vente = 44,80 €.
    let (_, lieu, _) = poster(&s, "/bibliotheque", PANNEAUX).await;
    assert_eq!(lieu.as_deref(), Some("/bibliotheque?ok=cree"));
    let (id, vente, achat): (i64, i64, Option<i64>) =
        sqlx::query_as("SELECT id, prix_vente_e5, prix_achat_e5 FROM ouvrages").fetch_one(&pool).await.unwrap();
    assert_eq!((vente, achat), (4_480_000, Some(2_800_000)));
    poster(&s, "/bibliotheque", "description=Pose+de+goutti%C3%A8res&genre=main_oeuvre&unite=h&prix_vente=45").await;

    let (_, page) = lire(&s, "/bibliotheque?ok=cree").await;
    for attendu in ["Panneaux isolants PIR 12 cm", "Brico Pro", "28,00\u{a0}€", "37,5 %", "44,80\u{a0}€", "Toiture", "Sans catégorie", "2 ouvrage(s)", "L&#39;ouvrage est ajouté"] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("&#39;", "'")), "{attendu}");
    }
    // Filtre par catégorie et recherche (htmx : le tableau seul).
    let (_, page) = lire(&s, "/bibliotheque?categorie=Toiture").await;
    assert!(page.contains("1 ouvrage(s)") && page.contains("Panneaux"));
    let (_, page) = lire(&s, "/bibliotheque?categorie=-").await;
    assert!(page.contains("1 ouvrage(s)") && page.contains("gouttières"));
    let (_, _, tableau) =
        envoyer(&s, Request::builder().uri("/bibliotheque?q=goutti").header("hx-request", "true").body(Body::empty()).unwrap()).await;
    assert!(!tableau.contains("<html") && tableau.contains("1 ouvrage(s)") && !tableau.contains("Panneaux"));

    // Erreurs : tout est gardé, rien n'est enregistré.
    let (code, _, page) = poster(&s, "/bibliotheque", "description=&genre=fourniture&prix_achat=12").await;
    assert_eq!(code, StatusCode::OK);
    assert!(page.contains("La description est obligatoire") && page.contains("value=\"12\""));
    assert!(!page.contains("Indiquez le prix de vente"), "un prix vide est permis (« à compléter »)");
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ouvrages").fetch_one(&pool).await.unwrap(), 2);

    // Modifier : un prix de vente saisi recalcule la marge.
    let (code, page) = lire(&s, &format!("/bibliotheque/{id}")).await;
    assert!(code == StatusCode::OK && page.contains("value=\"44,8\"") || page.contains("value=\"44,80\""));
    let (_, lieu, _) = poster(&s, &format!("/bibliotheque/{id}"), &PANNEAUX.replace("prix_vente=", "prix_vente=42%2C50")).await;
    assert_eq!(lieu.as_deref(), Some("/bibliotheque?ok=modifie"));
    let (_, page) = lire(&s, "/bibliotheque").await;
    // (42,50 − 28) ÷ 42,50 = 34,12 % du prix de vente.
    assert!(page.contains("34,12 %") && page.contains("42,50\u{a0}€"));
    assert_eq!(lire(&s, "/bibliotheque/999999").await.0, StatusCode::NOT_FOUND);

    let (_, lieu, _) = poster(&s, &format!("/bibliotheque/{id}/supprimer"), "").await;
    assert_eq!(lieu.as_deref(), Some("/bibliotheque?ok=supprime"));
    assert_eq!(poster(&s, &format!("/bibliotheque/{id}/supprimer"), "").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn inserer_un_ouvrage_dans_un_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/bibliotheque", PANNEAUX).await;
    let ouvrage: i64 = sqlx::query_scalar("SELECT id FROM ouvrages").fetch_one(&pool).await.unwrap();
    let (_, lieu, _) = poster(&s, "/devis", "").await;
    let devis: i64 = lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();

    let (_, page) = lire(&s, &format!("/devis/{devis}/encodage")).await;
    assert!(page.contains("Depuis la bibliothèque") && !page.contains("Ouvrage depuis la bibliothèque <small>bientôt"));
    let (_, choix) = lire(&s, &format!("/bibliotheque/choisir?devis={devis}&q=PIR")).await;
    assert!(choix.contains("Panneaux isolants PIR 12 cm") && choix.contains(&format!("hx-post=\"/devis/{devis}/bibliotheque\"")));
    let (_, choix) = lire(&s, &format!("/bibliotheque/choisir?devis={devis}&q=introuvable")).await;
    assert!(choix.contains("Aucun ouvrage ne correspond."));

    // Sans taux de TVA par défaut, le devis demande d'abord de le choisir.
    sqlx::query("UPDATE devis SET taux_tva_id = NULL WHERE id = $1").bind(devis).execute(&pool).await.unwrap();
    let (_, _, bloc) = poster(&s, &format!("/devis/{devis}/bibliotheque"), &format!("ouvrage={ouvrage}")).await;
    assert!(bloc.contains("Choisissez d&#39;abord le taux de TVA par défaut") || bloc.contains("Choisissez d'abord le taux de TVA par défaut"));

    sqlx::query("UPDATE devis SET taux_tva_id = (SELECT id FROM taux_tva WHERE taux_centiemes = 600) WHERE id = $1").bind(devis).execute(&pool).await.unwrap();
    let (code, _, bloc) = poster(&s, &format!("/devis/{devis}/bibliotheque"), &format!("ouvrage={ouvrage}")).await;
    assert!(code == StatusCode::OK && bloc.contains("id=\"encodage\"") && !bloc.contains("<html"));
    let (genre, description, quantite, unite, prix, achat, remarque): (String, String, Option<i64>, String, Option<i64>, Option<i64>, String) = sqlx::query_as(
        "SELECT genre::text, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, remarque FROM devis_lignes WHERE devis_id = $1",
    )
    .bind(devis)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (genre.as_str(), description.as_str(), quantite, unite.as_str(), prix, achat, remarque.as_str()),
        ("fourniture", "Panneaux isolants PIR 12 cm", Some(100_000), "m²", Some(4_480_000), Some(2_800_000), "Pose comprise")
    );
    let montant: i64 = sqlx::query_scalar("SELECT montant_ht_cents FROM devis WHERE id = $1").bind(devis).fetch_one(&pool).await.unwrap();
    assert_eq!(montant, 4_480, "les totaux du devis sont recalculés");

    // Changer la bibliothèque ne change pas le devis.
    sqlx::query("UPDATE ouvrages SET prix_vente_e5 = 9_900_000").execute(&pool).await.unwrap();
    let prix: Option<i64> = sqlx::query_scalar("SELECT prix_unitaire_e5 FROM devis_lignes WHERE devis_id = $1").bind(devis).fetch_one(&pool).await.unwrap();
    assert_eq!(prix, Some(4_480_000));

    // Un devis qui n'est plus un brouillon ne reçoit plus de ligne.
    poster(&s, &format!("/devis/{devis}/statut/envoye"), "").await;
    assert_eq!(poster(&s, &format!("/devis/{devis}/bibliotheque"), &format!("ouvrage={ouvrage}")).await.0, StatusCode::CONFLICT);
}
