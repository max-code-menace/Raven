// Tests de l'étape 5b-4 : types de ligne avancés, remarque par ligne, prix d'achat, marge.

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

async fn brouillon(pool: &PgPool) -> i64 {
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(pool).await.unwrap();
    sqlx::query_scalar("INSERT INTO devis (numero, taux_tva_id) VALUES (1, $1) RETURNING id").bind(tva).fetch_one(pool).await.unwrap()
}

async fn derniere_ligne(pool: &PgPool, devis: i64) -> i64 {
    sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 ORDER BY position DESC LIMIT 1").bind(devis).fetch_one(pool).await.unwrap()
}

async fn ajouter_ligne(pool: &PgPool, s: &Router, devis: i64, genre: &str) -> i64 {
    let (statut, _) = poster(s, &format!("/devis/{devis}/lignes"), &format!("genre={genre}")).await;
    assert_eq!(statut, StatusCode::OK, "{genre}");
    derniere_ligne(pool, devis).await
}

async fn remplir(s: &Router, devis: i64, ligne: i64, corps: &str) -> String {
    let (statut, corps) = poster(s, &format!("/devis/{devis}/lignes/{ligne}"), corps).await;
    assert_eq!(statut, StatusCode::OK);
    corps
}

// ─── Nouveaux types de ligne ────────────────────────────────────────────────

#[sqlx::test]
async fn tous_les_types_peuvent_etre_ajoutes(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    for genre in ["titre", "sous_titre", "texte", "poste_libre", "ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant", "saut_de_page"] {
        ajouter_ligne(&pool, &s, d, genre).await;
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM devis_lignes WHERE devis_id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 10);

    // Les six types chiffrés naissent avec la quantité 1 et la TVA par défaut ; les autres sans rien.
    let chiffres: Vec<(String, Option<i64>, bool)> = sqlx::query_as(
        "SELECT genre::text, quantite_e5, taux_tva_id IS NOT NULL FROM devis_lignes WHERE devis_id = $1 ORDER BY position",
    )
    .bind(d)
    .fetch_all(&pool)
    .await
    .unwrap();
    for (genre, q, tva) in chiffres {
        let attendu = ["poste_libre", "ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant"].contains(&genre.as_str());
        assert_eq!((q.is_some(), tva), (attendu, attendu), "{genre}");
    }
    // La main d'œuvre se compte en heures par défaut.
    let (unite,): (String,) = sqlx::query_as("SELECT unite FROM devis_lignes WHERE genre = 'main_oeuvre'").fetch_one(&pool).await.unwrap();
    assert_eq!(unite, "h");
}

#[sqlx::test]
async fn le_menu_propose_tous_les_types_et_le_saut_de_page_n_a_pas_de_numero(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    for genre in ["titre", "fourniture", "saut_de_page", "main_oeuvre"] {
        ajouter_ligne(&pool, &s, d, genre).await;
    }
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    for valeur in ["sous_titre", "ouvrage", "equipement", "main_oeuvre", "sous_traitant", "saut_de_page"] {
        assert!(page.contains(&format!("\"genre\": \"{valeur}\"")), "{valeur}");
    }
    // Numérotation : 1.0 (titre), 1.1 (fourniture), pas de numéro pour le saut, 1.2 (main d'œuvre).
    assert!(page.contains("rv-qt-ref\">1.0</span>") && page.contains("rv-qt-ref\">1.1</span>") && page.contains("rv-qt-ref\">1.2</span>"));
    assert!(!page.contains("rv-qt-ref\">1.3</span>"));
    assert!(page.contains("— Saut de page —"));
}

#[sqlx::test]
async fn un_type_chiffre_demande_la_tva_par_defaut(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let s = site(&pool);
    for genre in ["ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant"] {
        let (_, page) = poster(&s, &format!("/devis/{d}/lignes"), &format!("genre={genre}")).await;
        assert!(page.contains("Choisissez d&#39;abord le taux de TVA"), "{genre}");
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM devis_lignes").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
    // Saut de page et texte n'en ont pas besoin.
    ajouter_ligne(&pool, &s, d, "saut_de_page").await;
    ajouter_ligne(&pool, &s, d, "texte").await;
}

#[sqlx::test]
async fn tous_les_types_chiffres_comptent_dans_les_totaux(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    for genre in ["ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant"] {
        let l = ajouter_ligne(&pool, &s, d, genre).await;
        remplir(&s, d, l, "quantite=1&prix_unitaire=100").await;
    }
    let (ht, ttc): (i64, i64) = sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((ht, ttc), (50_000, 53_000));
}

#[sqlx::test]
async fn la_base_n_autorise_un_prix_que_sur_les_types_chiffres(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    for (position, genre) in ["ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant"].into_iter().enumerate() {
        let res = sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, prix_unitaire_e5, prix_achat_unitaire_e5) VALUES ($1, $2, $3::text::genre_ligne, 100, 50)")
            .bind(d)
            .bind(position as i32 + 1)
            .bind(genre)
            .execute(&pool)
            .await;
        assert!(res.is_ok(), "{genre}");
    }
    for (position, genre) in ["titre", "sous_titre", "texte", "saut_de_page"].into_iter().enumerate() {
        let res = sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, prix_achat_unitaire_e5) VALUES ($1, $2, $3::text::genre_ligne, 50)")
            .bind(d)
            .bind(position as i32 + 100)
            .bind(genre)
            .execute(&pool)
            .await;
        assert!(res.is_err(), "{genre}");
    }
}

// ─── Remarque par ligne ─────────────────────────────────────────────────────

#[sqlx::test]
async fn la_remarque_s_enregistre_sur_tous_les_types_et_apparait_dans_le_pdf(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    let titre = ajouter_ligne(&pool, &s, d, "titre").await;
    let fourniture = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, titre, "description=Toiture&remarque=Selon+plan+A").await;
    remplir(&s, d, fourniture, "description=Tuiles&remarque=Couleur+%C3%A0+confirmer&quantite=1&prix_unitaire=10").await;

    let (_, enc) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(enc.contains("value=\"Selon plan A\"") && enc.contains("value=\"Couleur à confirmer\""));
    let pdf = typst(&pool, d).await;
    assert!(pdf.contains("Selon plan A") && pdf.contains("Couleur à confirmer"));

    let (statut, _) = poster(&s, &format!("/devis/{d}/lignes/{titre}"), &format!("remarque={}", "x".repeat(2001))).await;
    assert_eq!(statut, StatusCode::OK);
    let (r,): (String,) = sqlx::query_as("SELECT remarque FROM devis_lignes WHERE id = $1").bind(titre).fetch_one(&pool).await.unwrap();
    assert_eq!(r, "Selon plan A", "une remarque trop longue n'est pas enregistrée");
}

// ─── Prix d'achat et marge de la ligne ──────────────────────────────────────

#[sqlx::test]
async fn le_prix_d_achat_s_enregistre_et_la_marge_de_la_ligne_est_renvoyee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    let l = ajouter_ligne(&pool, &s, d, "fourniture").await;

    // Vendu 100 € l'unité, acheté 80 € : bénéfice 20 €, soit 20 % du prix de vente, coefficient 1,25.
    let maj = remplir(&s, d, l, "quantite=1&prix_unitaire=100&prix_achat=80").await;
    assert!(maj.contains(&format!("<span id=\"marge-{l}\" hx-swap-oob=\"true\">20,00\u{a0}€ (20 %)</span>")), "{maj}");
    assert!(maj.contains("<span id=\"marge-totale\" hx-swap-oob=\"true\">20,00\u{a0}€</span>"));
    assert!(maj.contains("<span id=\"marge-vente\" hx-swap-oob=\"true\">20 %</span>"));
    assert!(maj.contains("<span id=\"coefficient-moyen\" hx-swap-oob=\"true\">1,25</span>"));
    assert!(maj.contains(&format!("id=\"coef-{l}\" hx-swap-oob=\"true\"")) && maj.contains("name=\"coefficient\" value=\"\" placeholder=\"1,25\""));
    // Le prix de vente n'est pas renvoyé (le champ pourrait être en cours de saisie).
    assert!(!maj.contains(&format!("id=\"prix-{l}\"")));
    let (achat,): (Option<i64>,) = sqlx::query_as("SELECT prix_achat_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(l).fetch_one(&pool).await.unwrap();
    assert_eq!(achat, Some(8_000_000));

    // Un champ vidé retire le prix d'achat.
    remplir(&s, d, l, "prix_achat=").await;
    let (achat,): (Option<i64>,) = sqlx::query_as("SELECT prix_achat_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(l).fetch_one(&pool).await.unwrap();
    assert_eq!(achat, None);

    // Un prix d'achat invalide est expliqué et refusé.
    let maj = remplir(&s, d, l, "prix_achat=douze").await;
    assert!(maj.contains("rv-banner--bad") && maj.contains("douze"));
}

#[sqlx::test]
async fn l_interrupteur_montre_ou_cache_les_colonnes_sans_perdre_les_prix_d_achat(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    let l = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, l, "quantite=1&prix_unitaire=100&prix_achat=80").await;

    // Éteint par défaut : ni colonne d'achat, ni frais généraux, ni rentabilité.
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(!page.contains("Achat U.") && !page.contains("name=\"prix_achat\"") && !page.contains("Frais généraux"));
    assert!(!lire(&s, &format!("/devis/{d}/totaux")).await.1.contains("Rentabilité"));
    // La réponse à une modification ne parle pas de marge.
    assert!(!remplir(&s, d, l, "quantite=2").await.contains("marge-"));

    // Allumé : les colonnes apparaissent, avec le prix d'achat déjà encodé.
    let (statut, bloc) = poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(bloc.contains("id=\"encodage\"") && bloc.contains("Achat U.") && bloc.contains("name=\"prix_achat\" value=\"80\""));
    assert!(bloc.contains("Frais généraux (% des achats)") && bloc.contains("Bénéfice"));
    let (actif,): (bool,) = sqlx::query_as("SELECT marge_activee FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert!(actif);
    assert!(lire(&s, &format!("/devis/{d}/totaux")).await.1.contains("Rentabilité"));

    // Éteint : les colonnes disparaissent, le prix d'achat reste en base.
    let (_, bloc) = poster(&s, &format!("/devis/{d}/marge"), "").await;
    assert!(!bloc.contains("Achat U."));
    let (achat,): (Option<i64>,) = sqlx::query_as("SELECT prix_achat_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(l).fetch_one(&pool).await.unwrap();
    assert_eq!(achat, Some(8_000_000));
}

// ─── Frais généraux ─────────────────────────────────────────────────────────

#[sqlx::test]
async fn les_frais_generaux_s_enregistrent_au_centieme_pres(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;

    let (statut, bloc) = poster(&s, &format!("/devis/{d}/frais-generaux"), "frais_generaux=7,5").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(bloc.contains("name=\"frais_generaux\" value=\"7,5\""));
    let (f,): (i32,) = sqlx::query_as("SELECT frais_generaux_centiemes FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(f, 750);

    // Vide : retour à 0. 100 est permis.
    poster(&s, &format!("/devis/{d}/frais-generaux"), "frais_generaux=").await;
    let (f,): (i32,) = sqlx::query_as("SELECT frais_generaux_centiemes FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(f, 0);
    poster(&s, &format!("/devis/{d}/frais-generaux"), "frais_generaux=100").await;
    let (f,): (i32,) = sqlx::query_as("SELECT frais_generaux_centiemes FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(f, 10_000);

    for (corps, message) in [("frais_generaux=100,01", "100 %"), ("frais_generaux=abc", "pourcentage"), ("frais_generaux=-5", "pourcentage"), ("frais_generaux=1,234", "2 décimales")] {
        let (_, bloc) = poster(&s, &format!("/devis/{d}/frais-generaux"), corps).await;
        assert!(bloc.contains("rv-banner--bad") && bloc.contains(message), "{corps}");
    }
    let (f,): (i32,) = sqlx::query_as("SELECT frais_generaux_centiemes FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(f, 10_000, "les valeurs refusées ne changent rien");
}

// ─── Rentabilité ────────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_page_totaux_montre_la_rentabilite_par_type_de_poste(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    poster(&s, &format!("/devis/{d}/frais-generaux"), "frais_generaux=10").await;
    let fourniture = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, fourniture, "quantite=10&prix_unitaire=20&prix_achat=12").await;
    let main_d_oeuvre = ajouter_ligne(&pool, &s, d, "main_oeuvre").await;
    remplir(&s, d, main_d_oeuvre, "quantite=16&unite=h&prix_unitaire=50&prix_achat=30").await;

    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    // Fournitures : 200 € vendus, 120 € achetés, frais 12 €, marge 68 €, 51,52 % de 132 €.
    for texte in ["Fournitures", "Main d&#39;œuvre", "120,00\u{a0}€", "12,00\u{a0}€", "68,00\u{a0}€", "51,52 %", "200,00\u{a0}€"] {
        assert!(page.contains(texte), "{texte}");
    }
    // Main d'œuvre : 800 € vendus, 480 € achetés, frais 48 €, marge 272 €, 16 h.
    for texte in ["480,00\u{a0}€", "48,00\u{a0}€", "272,00\u{a0}€", "800,00\u{a0}€", "16 h"] {
        assert!(page.contains(texte), "{texte}");
    }
    // Total : marge 340 € ; ventes 1 000 € (= le total HT du devis).
    assert!(page.contains("340,00\u{a0}€") && page.contains("1\u{a0}000,00\u{a0}€"));
    assert!(!page.contains("sans prix d'achat"));
    assert!(!page.contains("Postes libres / Ouvrages"), "seules les catégories utilisées apparaissent");
}

#[sqlx::test]
async fn les_lignes_sans_prix_d_achat_sont_signalees(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    let l = ajouter_ligne(&pool, &s, d, "ouvrage").await;
    remplir(&s, d, l, "quantite=1&prix_unitaire=100").await;
    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    assert!(page.contains("Postes libres / Ouvrages") && page.contains("1 ligne(s) chiffrée(s) sans prix d'achat"));
}

#[sqlx::test]
async fn l_ajustement_apparait_dans_la_rentabilite_et_les_ventes_valent_le_total_ht(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    let l = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, l, "quantite=1&prix_unitaire=1000&prix_achat=600").await;
    poster(&s, &format!("/devis/{d}/ajustement"), "libelle=Geste&sens=remise&mode=montant&valeur=50").await;

    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    // Ventes 1 000 € − 50 € = 950 € ; marge 400 € − 50 € = 350 €.
    assert!(page.contains("Geste") && page.contains("-50,00\u{a0}€"));
    assert!(page.contains("350,00\u{a0}€") && page.contains("950,00\u{a0}€"));
    let (ht,): (i64,) = sqlx::query_as("SELECT montant_ht_cents FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(ht, 95_000);
}

#[sqlx::test]
async fn les_heures_ne_comptent_que_pour_la_main_d_oeuvre_en_heures(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    let heures = ajouter_ligne(&pool, &s, d, "main_oeuvre").await;
    remplir(&s, d, heures, "quantite=8&unite=Heures&prix_unitaire=50").await;
    let jours = ajouter_ligne(&pool, &s, d, "main_oeuvre").await;
    remplir(&s, d, jours, "quantite=2&unite=jour(s)&prix_unitaire=400").await;
    let fourniture = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, fourniture, "quantite=5&unite=h&prix_unitaire=10").await;
    let (_, page) = lire(&s, &format!("/devis/{d}/totaux")).await;
    assert!(page.contains("8 h") && !page.contains("10 h") && !page.contains("13 h"));
}

// ─── Aperçu, lecture seule ──────────────────────────────────────────────────

#[sqlx::test]
async fn le_pdf_du_client_ne_montre_jamais_les_prix_d_achat_ni_la_marge(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    let l = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, l, "description=Tuiles&quantite=1&prix_unitaire=100&prix_achat=73,21").await;
    ajouter_ligne(&pool, &s, d, "saut_de_page").await;
    let pdf = typst(&pool, d).await;
    assert!(pdf.contains("Tuiles"));
    // Le saut de page coupe le tableau en deux segments.
    assert!(pdf.contains("segments: ((") && pdf.matches("(type: ").count() == 1);
    assert!(!pdf.contains("73,21") && !pdf.contains("marge") && !pdf.contains("achat"));
    // Et le PDF lui-même se génère.
    let (statut, octets) = lire_octets(&s, &format!("/devis/{d}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
}

#[sqlx::test]
async fn un_devis_envoye_ne_change_plus_ni_la_marge_ni_les_frais(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/devis/{d}/frais-generaux"), "frais_generaux=10").await.0, StatusCode::CONFLICT);
    let (actif, f): (bool, i32) = sqlx::query_as("SELECT marge_activee, frais_generaux_centiemes FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((actif, f), (false, 0));
}

#[sqlx::test]
async fn la_base_refuse_des_frais_generaux_hors_limites(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    for valeur in [-1, 10_001] {
        let res = sqlx::query("UPDATE devis SET frais_generaux_centiemes = $1 WHERE id = $2").bind(valeur).bind(d).execute(&pool).await;
        assert!(res.is_err(), "{valeur}");
    }
}

// ─── Outils pour lire le PDF ────────────────────────────────────────────────

/// Le texte Typst qui sert à fabriquer le PDF du devis : tout ce que le client verra y figure.
async fn typst(pool: &PgPool, devis: i64) -> String {
    match ubifacture::devis_pdf::charger_donnees(&mut *pool.acquire().await.unwrap(), devis).await {
        Ok(donnees) => ubifacture::devis_pdf::source_typst(&donnees).0,
        Err(_) => panic!("devis introuvable"),
    }
}

async fn lire_octets(site: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let r = site.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let statut = r.status();
    (statut, r.into_body().collect().await.unwrap().to_bytes().to_vec())
}

// ─── Coefficient et marge sur prix de vente (refonte, étape 4-3) ───────────

#[sqlx::test]
async fn le_coefficient_calcule_le_prix_de_vente(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = brouillon(&pool).await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/marge"), "marge_activee=on").await;
    let l = ajouter_ligne(&pool, &s, d, "fourniture").await;
    remplir(&s, d, l, "quantite=2&prix_achat=12,34").await;

    // 12,34 € × 1,35 = 16,659 € → 16,66 € ; total 2 × 16,66 = 33,32 €.
    let (statut, maj) = poster(&s, &format!("/devis/{d}/lignes/{l}/coefficient"), "coefficient=1,35").await;
    assert_eq!(statut, StatusCode::OK);
    let (prix,): (Option<i64>,) = sqlx::query_as("SELECT prix_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(l).fetch_one(&pool).await.unwrap();
    assert_eq!(prix, Some(1_666_000));
    assert!(maj.contains(&format!("id=\"prix-{l}\" hx-swap-oob=\"true\"")) && maj.contains("name=\"prix_unitaire\" value=\"16,66\""), "{maj}");
    assert!(maj.contains("name=\"coefficient\" value=\"\" placeholder=\"1,35\""));
    let (ht,): (i64,) = sqlx::query_as("SELECT montant_ht_cents FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(ht, 3_332);
    // Bénéfice 2 × (16,66 − 12,34) = 8,64 € ; 8,64 ÷ 33,32 = 25,93 % du prix de vente.
    assert!(maj.contains("8,64\u{a0}€ (25,93 %)"));

    // Un coefficient invalide est expliqué et rien ne change.
    for faux in ["zéro", "0", "-1", "250"] {
        let (_, maj) = poster(&s, &format!("/devis/{d}/lignes/{l}/coefficient"), &format!("coefficient={faux}")).await;
        assert!(maj.contains("rv-banner--bad") && maj.contains("coefficient"), "{faux}");
    }
    let (prix,): (Option<i64>,) = sqlx::query_as("SELECT prix_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(l).fetch_one(&pool).await.unwrap();
    assert_eq!(prix, Some(1_666_000));

    // Un champ vidé ne change rien et n'affiche pas d'erreur.
    let (_, maj) = poster(&s, &format!("/devis/{d}/lignes/{l}/coefficient"), "coefficient=").await;
    assert!(!maj.contains("rv-banner--bad"));

    // Sans prix d'achat, le coefficient ne peut rien calculer.
    let autre = ajouter_ligne(&pool, &s, d, "fourniture").await;
    let (_, maj) = poster(&s, &format!("/devis/{d}/lignes/{autre}/coefficient"), "coefficient=1,5").await;
    assert!(maj.contains("Saisissez d") && maj.contains("abord le prix d"), "{maj}");

    // La page montre la colonne, le coefficient de la ligne et le coefficient moyen.
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains(">Coef.<") && page.contains("Marge sur le prix de vente") && page.contains("Coefficient moyen"));

    // Un devis qui n'est plus un brouillon refuse le coefficient.
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    let (statut, _) = poster(&s, &format!("/devis/{d}/lignes/{l}/coefficient"), "coefficient=2").await;
    assert_eq!(statut, StatusCode::CONFLICT);
}
