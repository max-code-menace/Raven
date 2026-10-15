// Tests des onglets Totaux (ajustement) et Visualisation (aperçu).

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

/// Un brouillon avec deux postes : 600 € à 6 % et 400 € à 21 %.
async fn devis_a_deux_taux(pool: &PgPool) -> i64 {
    let (reduit, normal) = (taux(pool, 600).await, taux(pool, 2100).await);
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero, taux_tva_id) VALUES (1, $1) RETURNING id").bind(reduit).fetch_one(pool).await.unwrap();
    for (position, prix, t) in [(1, 6_000_000_000_i64 / 100, reduit), (2, 4_000_000_000_i64 / 100, normal)] {
        sqlx::query(
            "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, prix_unitaire_e5, taux_tva_id)
             VALUES ($1, $2, 'poste_libre', 'Poste', 100000, $3, $4)",
        )
        .bind(d)
        .bind(position)
        .bind(prix)
        .bind(t)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE devis SET montant_ht_cents = 100000, montant_ttc_cents = 100000 + 3600 + 8400 WHERE id = $1").bind(d).execute(pool).await.unwrap();
    d
}

async fn montants(pool: &PgPool, devis: i64) -> (i64, i64) {
    sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(devis).fetch_one(pool).await.unwrap()
}

// ─── Onglet Totaux ──────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_page_totaux_montre_les_montants_par_taux(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/totaux")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Sous-total HT") && page.contains("1\u{a0}000,00\u{a0}€"));
    assert!(page.contains("600,00\u{a0}€") && page.contains("36,00\u{a0}€") && page.contains("84,00\u{a0}€"));
    assert!(page.contains("1\u{a0}120,00\u{a0}€"));
    assert!(page.contains(&format!("tab tab-active\" href=\"/devis/{d}/totaux\"")));
    assert_eq!(lire(&site(&pool), "/devis/999/totaux").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn une_remise_en_pourcentage_se_repartit_et_met_le_devis_a_jour(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    let (statut, lieu, _) = poster(&s, &format!("/devis/{d}/ajustement"), "libelle=Remise+fid%C3%A9lit%C3%A9&sens=remise&mode=pourcentage&valeur=10").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu, Some(format!("/devis/{d}/totaux")));

    // 1 000 € − 10 % = 900 € : bases 540 € (6 %) et 360 € (21 %) ; TVA 32,40 + 75,60 ; TTC 1 008 €
    assert_eq!(montants(&pool, d).await, (90_000, 90_000 + 3_240 + 7_560));
    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    assert!(page.contains("Remise fidélité") && page.contains("-100,00\u{a0}€"));
    assert!(page.contains("540,00\u{a0}€") && page.contains("360,00\u{a0}€") && page.contains("1\u{a0}008,00\u{a0}€"));
    // Le formulaire montre l'ajustement enregistré.
    assert!(page.contains("value=\"Remise fidélité\"") && page.contains("value=\"10\""));
    assert!(page.contains("Retirer l'ajustement"));
}

#[sqlx::test]
async fn une_majoration_en_montant(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/ajustement"), "libelle=Frais&sens=majoration&mode=montant&valeur=50").await;
    // +50 € répartis 30 € / 20 € : bases 630 € et 420 € ; TVA 37,80 + 88,20 = 126 €
    assert_eq!(montants(&pool, d).await, (105_000, 105_000 + 3_780 + 8_820));
}

#[sqlx::test]
async fn l_ajustement_suit_les_changements_de_lignes(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/ajustement"), "sens=remise&mode=pourcentage&valeur=10").await;
    // On change un prix dans l'encodage : la remise de 10 % s'applique au nouveau sous-total.
    let ligne: i64 = sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 ORDER BY position LIMIT 1").bind(d).fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "quantite=1&prix_unitaire=1600").await;
    // sous-total 2 000 € (1 600 + 400) ; remise 200 € ; HT 1 800 €
    assert_eq!(montants(&pool, d).await.0, 180_000);
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("Sous-total HT") && page.contains("Ajustement") && page.contains("-200,00\u{a0}€"));
}

#[sqlx::test]
async fn retirer_l_ajustement_remet_les_totaux(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/ajustement"), "sens=remise&mode=montant&valeur=100").await;
    assert_eq!(montants(&pool, d).await.0, 90_000);
    let (statut, _, _) = poster(&s, &format!("/devis/{d}/ajustement/retirer"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(montants(&pool, d).await, (100_000, 112_000));
    let (mode, valeur): (String, i64) = sqlx::query_as("SELECT ajustement_mode, ajustement_valeur FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((mode.as_str(), valeur), ("aucun", 0));
}

#[sqlx::test]
async fn un_ajustement_invalide_est_explique_et_rien_n_est_enregistre(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    for (corps, message) in [
        ("sens=remise&mode=pourcentage&valeur=150", "Une remise ne peut pas dépasser 100 %."),
        ("sens=remise&mode=montant&valeur=1500", "La remise dépasse le sous-total des lignes"),
        ("sens=remise&mode=montant&valeur=abc", "supérieur à 0"),
        ("sens=remise&mode=montant&valeur=0", "supérieur à 0"),
        ("sens=remise&mode=montant&valeur=5,123", "2 décimales"),
        ("sens=nimporte&mode=montant&valeur=5", "Choisis remise ou majoration."),
        ("sens=remise&mode=autre&valeur=5", "Choisis pourcentage ou montant."),
    ] {
        let (statut, _, page) = poster(&s, &format!("/devis/{d}/ajustement"), corps).await;
        assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY, "{corps}");
        assert!(page.contains(message), "{corps}");
    }
    assert_eq!(montants(&pool, d).await, (100_000, 112_000));
    let (mode,): (String,) = sqlx::query_as("SELECT ajustement_mode FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(mode, "aucun");
}

#[sqlx::test]
async fn la_valeur_tapee_revient_apres_une_erreur(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let (_, _, page) = poster(&site(&pool), &format!("/devis/{d}/ajustement"), "libelle=Mon+geste&sens=majoration&mode=montant&valeur=abc").await;
    assert!(page.contains("value=\"Mon geste\"") && page.contains("value=\"abc\""));
    assert!(page.contains("<option value=\"majoration\" selected>") && page.contains("<option value=\"montant\" selected>"));
}

#[sqlx::test]
async fn un_devis_envoye_n_a_plus_d_ajustement_modifiable(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let s = site(&pool);
    sqlx::query("UPDATE devis SET statut = 'accepte' WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/devis/{d}/ajustement"), "sens=remise&mode=montant&valeur=100").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/devis/{d}/ajustement/retirer"), "").await.0, StatusCode::CONFLICT);
    assert_eq!(montants(&pool, d).await, (100_000, 112_000));
    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    assert!(page.contains("Ce devis n'est plus un brouillon") && !page.contains("Enregistrer l'ajustement"));
}

#[sqlx::test]
async fn la_base_refuse_un_ajustement_incoherent(pool: PgPool) {
    let d = devis_a_deux_taux(&pool).await;
    let res = sqlx::query("UPDATE devis SET ajustement_mode = 'aucun', ajustement_valeur = 500 WHERE id = $1").bind(d).execute(&pool).await;
    assert!(res.is_err());
    let res = sqlx::query("UPDATE devis SET ajustement_mode = 'inconnu' WHERE id = $1").bind(d).execute(&pool).await;
    assert!(res.is_err());
}

// ─── Onglet Visualisation ───────────────────────────────────────────────────

#[sqlx::test]
async fn l_apercu_reprend_les_informations_et_les_lignes(pool: PgPool) {
    let (reduit, _) = (taux(&pool, 600).await, ());
    let conditions: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7").fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO societe (id, nom, nom_abrege, rue, numero, code_postal, ville, email, telephone, numero_tva) VALUES (1, 'Ubique SRL', 'Ubique', 'Rue du Bâtiment', '5', '4000', 'Liège', 'info@ubique.be', '04 123 45 67', 'BE0123456789')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ('BE68 5390 0754 7034', true)").execute(&pool).await.unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, rue, numero, code_postal, ville, numero_tva) VALUES ('Dupont SRL', 'Rue Neuve', '12', '1000', 'Bruxelles', 'BE0987654321') RETURNING id").fetch_one(&pool).await.unwrap();
    let d: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, reference, responsable, client_entreprise_id, taux_tva_id, conditions_paiement_id, remarque_1, remarque_2)
         VALUES (57, '2026-10-15', 'C44 Volet', 'Marc', $1, $2, $3, 'Acompte de 30 %', 'Chantier accessible') RETURNING id",
    )
    .bind(client).bind(reduit).bind(conditions)
    .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description) VALUES ($1, 1, 'titre', 'Fourniture des matériaux')").bind(d).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id) VALUES ($1, 2, 'poste_libre', 'Volet roulant', 250000, 'pce', 1999000, $2)")
        .bind(d).bind(reduit).execute(&pool).await.unwrap();

    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/apercu")).await;
    assert_eq!(statut, StatusCode::OK);
    for texte in [
        "DEVIS N° 2026-57", "C44 Volet", "15/10/2026", "Marc",
        "Ubique SRL", "Rue du Bâtiment 5", "4000 Liège", "TVA BE0123456789", "BE68 5390 0754 7034",
        "Dupont SRL", "Rue Neuve 12", "1000 Bruxelles", "TVA BE0987654321",
        "Fourniture des matériaux", "Volet roulant", "2,5", "pce", "19,99\u{a0}€", "6 %", "49,98\u{a0}€",
        "7 jour(s) après date de facturation", "Acompte de 30 %", "Chantier accessible",
        "Total TTC à payer", "52,98\u{a0}€",
    ] {
        assert!(page.contains(texte), "{texte}");
    }
    // Pas de logo ni de « facturé à » quand ils n'existent pas ; le chantier reprend l'adresse du client.
    assert!(!page.contains("alt=\"Logo\"") && !page.contains("Facturé à"));
    assert!(page.contains("Rue Neuve 12, 1000 Bruxelles"));
    assert!(page.contains(&format!("tab tab-active\" href=\"/devis/{d}/apercu\"")));
}

#[sqlx::test]
async fn l_apercu_montre_un_autre_client_de_facturation_et_une_adresse_de_chantier(pool: PgPool) {
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Client SA') RETURNING id").fetch_one(&pool).await.unwrap();
    let assurance: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, rue, numero, code_postal, ville, pays) VALUES ('Assurance Belfius', 'Boulevard Pachéco', '44', '1000', 'Bruxelles', 'BE') RETURNING id").fetch_one(&pool).await.unwrap();
    let d: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, client_entreprise_id, facturation_identique, facturation_entreprise_id, chantier_identique,
                            chantier_rue, chantier_numero, chantier_code_postal, chantier_ville, chantier_pays)
         VALUES (1, $1, false, $2, false, 'Rue de Paris', '3', '75001', 'Paris', 'FR') RETURNING id",
    )
    .bind(client).bind(assurance).fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}/apercu")).await;
    assert!(page.contains("Facturé à") && page.contains("Assurance Belfius") && page.contains("Boulevard Pachéco 44"));
    assert!(page.contains("Rue de Paris 3, 75001 Paris, France"));
}

#[sqlx::test]
async fn l_apercu_d_un_devis_vide_ne_plante_pas_et_previent(pool: PgPool) {
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/apercu")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Aucune ligne pour l'instant") && page.contains("Client à choisir"));
    assert!(page.contains("pas encore renseignées"));
    assert_eq!(lire(&site(&pool), "/devis/999/apercu").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn l_apercu_neutralise_les_textes_tapes(pool: PgPool) {
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero, reference, remarque_1) VALUES (1, '<script>alert(1)</script>', '<b>gras</b>') RETURNING id").fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}/apercu")).await;
    assert!(!page.contains("<script>alert(1)") && !page.contains("<b>gras"));
}

#[sqlx::test]
async fn les_quatre_onglets_sont_relies(pool: PgPool) {
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}")).await;
    for lien in [format!("href=\"/devis/{d}\""), format!("href=\"/devis/{d}/encodage\""), format!("href=\"/devis/{d}/totaux\""), format!("href=\"/devis/{d}/apercu\"")] {
        assert!(page.contains(&lien), "{lien}");
    }
}
