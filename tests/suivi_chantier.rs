// Tests de l'étape 9-2 : suivi de chantier complet (rentabilité prévue et actuelle, budget d'achat, heures,
// facturation et encaissements, achats saisis à la main).

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

const E: &str = "\u{a0}€";

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

async fn lire(site: &Router, uri: &str) -> String {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{uri}");
    corps
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

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

/// Un chantier né d'un devis de 2 695,00 € HT : 24 h à 45 € (achat 28 €) et 38 m² à 42,50 € (achat 29 €),
/// frais généraux 10 %. Achats prévus 1 774,00 €, frais généraux 177,40 €, bénéfice prévu 743,60 €.
async fn chantier(pool: &PgPool, s: &Router) -> i64 {
    let (_, lieu) = poster(s, "/devis", "").await;
    let devis: i64 = lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils) VALUES ('Client SA', '{client}') RETURNING id").fetch_one(pool).await.unwrap();
    let conditions: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_one(pool).await.unwrap();
    sqlx::query(
        "UPDATE devis SET client_entreprise_id = $1, conditions_paiement_id = $2, reference = 'L63 Toiture', frais_generaux_centiemes = 1000,
                          montant_ht_cents = 269500, montant_ttc_cents = 285670 WHERE id = $3",
    )
    .bind(client)
    .bind(conditions)
    .bind(devis)
    .execute(pool)
    .await
    .unwrap();
    for (position, genre, description, quantite, unite, prix, achat) in [
        (1, "main_oeuvre", "Pose", 2_400_000_i64, "h", 4_500_000_i64, 2_800_000_i64),
        (2, "fourniture", "Panneaux", 3_800_000, "m²", 4_250_000, 2_900_000),
    ] {
        sqlx::query(
            "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, taux_tva_id)
             VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8, (SELECT id FROM taux_tva WHERE taux_centiemes = 600))",
        )
        .bind(devis)
        .bind(position)
        .bind(genre)
        .bind(description)
        .bind(quantite)
        .bind(unite)
        .bind(prix)
        .bind(achat)
        .execute(pool)
        .await
        .unwrap();
    }
    poster(s, &format!("/devis/{devis}/statut/accepter"), &format!("date={}", aujourdhui(pool).await)).await;
    sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn rentabilite_budget_et_heures_prevus(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let id = chantier(&pool, &s).await;
    let page = lire(&s, &format!("/chantiers/{id}")).await;
    for attendu in [
        "Rentabilité",
        &format!("2\u{a0}695,00{E}"),
        &format!("1\u{a0}774,00{E}"),
        &format!("177,40{E}"),
        &format!("743,60{E}"),
        "Budget d&#39;achat",
        "Dépensé : 0,00\u{a0}€ sur 1\u{a0}774,00\u{a0}€ prévus (0 %)",
        "24 h",
        "Reste à recevoir TTC",
    ] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("&#39;", "'")), "{attendu}");
    }
    assert!(!page.contains("bientôt (étape 9)"), "plus de mention « bientôt »");
}

#[sqlx::test]
async fn les_achats_du_chantier(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let id = chantier(&pool, &s).await;
    let jour = aujourdhui(&pool).await;
    let (_, lieu) = poster(&s, &format!("/chantiers/{id}/achats"), &format!("date={jour}&categorie=fourniture&fournisseur=Brico+SA&description=Facture+F-12&montant=1000")).await;
    assert_eq!(lieu, Some(format!("/chantiers/{id}/finances?ok=achat#achats")));
    let page = lire(&s, &format!("/chantiers/{id}/finances?ok=achat")).await;
    for attendu in ["Achats du chantier", "Brico SA", "Facture F-12", "Fournitures", &format!("Disponible HT <strong>774,00{E}</strong>")] {
        assert!(page.contains(attendu), "{attendu}");
    }
    let page = lire(&s, &format!("/chantiers/{id}")).await;
    assert!(page.contains("Dépensé : 1\u{a0}000,00\u{a0}€ sur 1\u{a0}774,00\u{a0}€ prévus (56 %)") && page.contains("width:56%"));

    // Au-delà du budget : dépassé.
    poster(&s, &format!("/chantiers/{id}/achats"), &format!("date={jour}&categorie=sous_traitant&montant=1000")).await;
    let page = lire(&s, &format!("/chantiers/{id}")).await;
    assert!(page.contains("budget dépassé") && page.contains(&format!("-226,00{E}")) && page.contains("width:100%"));

    // Saisies refusées.
    for corps in [format!("date={jour}&categorie=fourniture&montant=0"), format!("date={jour}&categorie=inconnue&montant=10"), "date=&categorie=fourniture&montant=10".to_string()] {
        assert_eq!(poster(&s, &format!("/chantiers/{id}/achats"), &corps).await.1, Some(format!("/chantiers/{id}/finances?erreur=achat#achats")), "{corps}");
    }
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM chantier_achats").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 2);

    // Retirer un achat.
    let achat: i64 = sqlx::query_scalar("SELECT max(id) FROM chantier_achats").fetch_one(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/chantiers/{id}/achats/{achat}/supprimer"), "").await.1, Some(format!("/chantiers/{id}/finances?ok=achat_retire#achats")));
    assert_eq!(poster(&s, &format!("/chantiers/{id}/achats/{achat}/supprimer"), "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(poster(&s, "/chantiers/999999/achats", &format!("date={jour}&categorie=fourniture&montant=10")).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn facture_encaisse_et_benefice_actuel(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let id = chantier(&pool, &s).await;
    // Un acompte de 1 000 € HT validé (TTC 1 060 €), payé 500 €.
    let (_, lieu) = poster(&s, &format!("/chantiers/{id}/factures/acompte"), "mode=montant&valeur=1000").await;
    let facture: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/factures/{facture}/valider"), "").await;
    let methode: i64 = sqlx::query_scalar("SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").fetch_one(&pool).await.unwrap();
    let jour = aujourdhui(&pool).await;
    poster(&s, &format!("/factures/{facture}/paiements"), &format!("genre=partiel&date={jour}&montant=500&methode_id={methode}")).await;
    poster(&s, &format!("/chantiers/{id}/achats"), &format!("date={jour}&categorie=fourniture&montant=400")).await;

    let page = lire(&s, &format!("/chantiers/{id}")).await;
    for attendu in [&format!("Encaissé</span><span style=\"color:#1C6B3C\">500,00{E}"), &format!("Reste à recevoir TTC</span><span>560,00{E}"), &format!("600,00{E}")] {
        assert!(page.contains(attendu), "{attendu}");
    }
    // La liste des chantiers totalise le reste à facturer et le reste à recevoir.
    let liste = lire(&s, "/chantiers").await;
    assert!(liste.contains(&format!("Reste à facturer HT<b>1\u{a0}695,00{E}")) && liste.contains(&format!("560,00{E}")));
}
