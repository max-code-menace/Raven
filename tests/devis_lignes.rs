// Tests de l'éditeur de devis, onglet Encodage.

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

async fn taux(pool: &PgPool, centiemes: i32) -> i64 {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(centiemes).fetch_one(pool).await.unwrap()
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

// ─── Page et ajout ──────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_page_est_vide_au_depart(pool: PgPool) {
    let d = brouillon(&pool).await;
    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/encodage")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Aucune ligne.") && page.contains("+ Poste libre") && page.contains("Total TTC"));
    // L'onglet Encodage est actif et l'onglet Informations y renvoie.
    assert!(page.contains(&format!("tab tab-active\" href=\"/devis/{d}/encodage\"")));
    assert!(page.contains(&format!("href=\"/devis/{d}\">Informations")));
    assert_eq!(lire(&site(&pool), "/devis/999/encodage").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn ajouter_des_lignes_les_numerote_par_lot(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    for genre in ["titre", "poste_libre", "texte", "titre", "poste_libre"] {
        assert_eq!(ajouter(&s, d, genre).await, StatusCode::OK);
    }
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    for numero in ["1.0", "1.1", "1.2", "2.0", "2.1"] {
        assert!(page.contains(&format!("whitespace-nowrap\">{numero}</td>")), "{numero}");
    }
    // Un poste libre naît avec la quantité 1 et la TVA par défaut du devis.
    let (q, tva): (Option<i64>, Option<i64>) = sqlx::query_as("SELECT quantite_e5, taux_tva_id FROM devis_lignes WHERE genre = 'poste_libre' LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((q, tva), (Some(100_000), Some(taux(&pool, 600).await)));
    // Un titre n'a ni quantité ni TVA.
    let (q, tva): (Option<i64>, Option<i64>) = sqlx::query_as("SELECT quantite_e5, taux_tva_id FROM devis_lignes WHERE genre = 'titre' LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((q, tva), (None, None));
}

#[sqlx::test]
async fn un_poste_libre_demande_une_tva_par_defaut(pool: PgPool) {
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let s = site(&pool);
    let (statut, page) = poster(&s, &format!("/devis/{d}/lignes"), "genre=poste_libre").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Choisis d&#39;abord le taux de TVA par défaut"));
    assert!(ids(&pool, d).await.is_empty());
    // Un titre ne demande rien.
    assert_eq!(ajouter(&s, d, "titre").await, StatusCode::OK);
    assert_eq!(ids(&pool, d).await.len(), 1);
}

#[sqlx::test]
async fn un_type_inconnu_n_ajoute_rien(pool: PgPool) {
    let d = brouillon(&pool).await;
    let (statut, page) = poster(&site(&pool), &format!("/devis/{d}/lignes"), "genre=inconnu").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Ce type de ligne n&#39;existe pas."));
    assert!(ids(&pool, d).await.is_empty());
}

// ─── Modification et totaux ─────────────────────────────────────────────────

#[sqlx::test]
async fn modifier_une_ligne_recalcule_les_totaux_du_devis(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "titre").await;
    ajouter(&s, d, "poste_libre").await;
    let ligne = ids(&pool, d).await[1];

    // 16 h à 50 € = 800 € HT ; TVA 6 % = 48 € ; TTC = 848 €.
    let (statut, maj) = poster(
        &s,
        &format!("/devis/{d}/lignes/{ligne}"),
        "description=Main+d%27%C5%93uvre&quantite=16&unite=h&prix_unitaire=50",
    )
    .await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(montants(&pool, d).await, (80_000, 84_800));
    // La réponse met à jour la ligne, le sous-total du lot et les totaux, sans toucher aux champs.
    assert!(maj.contains(&format!("id=\"total-{ligne}\"")) && maj.contains("800,00\u{a0}€"));
    assert!(maj.contains("id=\"soustotal-") && maj.contains("id=\"totaux\""));
    assert!(maj.contains("848,00\u{a0}€") && maj.contains("TVA 6 % (sur 800,00\u{a0}€)"));
    assert!(!maj.contains("<textarea"));

    let (description, q, unite, p): (String, Option<i64>, String, Option<i64>) =
        sqlx::query_as("SELECT description, quantite_e5, unite, prix_unitaire_e5 FROM devis_lignes WHERE id = $1")
            .bind(ligne)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((description.as_str(), q, unite.as_str(), p), ("Main d'œuvre", Some(1_600_000), "h", Some(5_000_000)));

    // Le tableau rechargé montre les valeurs avec la virgule belge.
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("name=\"quantite\" value=\"16\"") && page.contains("name=\"prix_unitaire\" value=\"50\""));
}

#[sqlx::test]
async fn la_liste_des_devis_montre_le_total_calcule(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "poste_libre").await;
    let ligne = ids(&pool, d).await[0];
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "quantite=1&prix_unitaire=6603,77").await;
    assert_eq!(montants(&pool, d).await, (660_377, 700_000));
    let (_, liste) = lire(&s, "/devis").await;
    assert!(liste.contains("6\u{a0}603,77\u{a0}€") && liste.contains("7\u{a0}000,00\u{a0}€"));
}

#[sqlx::test]
async fn plusieurs_taux_de_tva_et_montant_negatif(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    let (reduit, normal) = (taux(&pool, 600).await, taux(&pool, 2100).await);
    for _ in 0..3 {
        ajouter(&s, d, "poste_libre").await;
    }
    let l = ids(&pool, d).await;
    poster(&s, &format!("/devis/{d}/lignes/{}", l[0]), "quantite=1&prix_unitaire=10384,13").await;
    // Avenant en moins : prix négatif.
    poster(&s, &format!("/devis/{d}/lignes/{}", l[1]), "quantite=1&prix_unitaire=-3780,36").await;
    poster(&s, &format!("/devis/{d}/lignes/{}", l[2]), &format!("quantite=2,5&prix_unitaire=19,99&taux_tva_id={normal}")).await;
    // 10 384,13 − 3 780,36 = 6 603,77 à 6 % → 396,23 ; 49,975 → 49,98 à 21 % → 10,50
    assert_eq!(montants(&pool, d).await, (660_377 + 4_998, 660_377 + 39_623 + 4_998 + 1_050));
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("TVA 6 % (sur 6\u{a0}603,77\u{a0}€)") && page.contains("TVA 21 % (sur 49,98\u{a0}€)"));
    let _ = reduit;
}

#[sqlx::test]
async fn un_nombre_invalide_n_enregistre_rien_et_explique(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "poste_libre").await;
    let ligne = ids(&pool, d).await[0];
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "quantite=2&prix_unitaire=10").await;

    let (statut, maj) = poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "quantite=2&prix_unitaire=dix+euros").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(maj.contains("alert-warning") && maj.contains("dix euros"));
    let (p,): (Option<i64>,) = sqlx::query_as("SELECT prix_unitaire_e5 FROM devis_lignes WHERE id = $1").bind(ligne).fetch_one(&pool).await.unwrap();
    assert_eq!(p, Some(1_000_000));
    assert_eq!(montants(&pool, d).await.0, 2_000);

    // Six décimales, trop de caractères et taux inconnu sont refusés aussi.
    for corps in ["quantite=1,000001".to_string(), format!("unite={}", "x".repeat(21)), "taux_tva_id=99999".to_string()] {
        let (_, maj) = poster(&s, &format!("/devis/{d}/lignes/{ligne}"), &corps).await;
        assert!(maj.contains("alert-warning"), "{corps}");
    }
    // Un champ vidé remet la valeur à « vide » : le total repasse à zéro.
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "prix_unitaire=").await;
    assert_eq!(montants(&pool, d).await, (0, 0));
}

#[sqlx::test]
async fn un_titre_ne_porte_ni_prix_ni_quantite(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "titre").await;
    let ligne = ids(&pool, d).await[0];
    let (statut, _) = poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "description=Toiture&quantite=5&prix_unitaire=100&unite=m2").await;
    assert_eq!(statut, StatusCode::OK);
    let (description, q, p, unite): (String, Option<i64>, Option<i64>, String) =
        sqlx::query_as("SELECT description, quantite_e5, prix_unitaire_e5, unite FROM devis_lignes WHERE id = $1").bind(ligne).fetch_one(&pool).await.unwrap();
    assert_eq!((description.as_str(), q, p, unite.as_str()), ("Toiture", None, None, ""));
    assert_eq!(montants(&pool, d).await, (0, 0));
}

#[sqlx::test]
async fn le_sous_total_d_un_lot_s_arrete_au_titre_suivant(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    for genre in ["titre", "poste_libre", "poste_libre", "titre", "poste_libre"] {
        ajouter(&s, d, genre).await;
    }
    let l = ids(&pool, d).await;
    for (i, prix) in [(1, "100"), (2, "50"), (4, "7")] {
        poster(&s, &format!("/devis/{d}/lignes/{}", l[i]), &format!("quantite=1&prix_unitaire={prix}")).await;
    }
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains(&format!("<span id=\"soustotal-{}\">150,00\u{a0}€</span>", l[0])));
    assert!(page.contains(&format!("<span id=\"soustotal-{}\">7,00\u{a0}€</span>", l[3])));
}

// ─── Ordre et suppression ───────────────────────────────────────────────────

#[sqlx::test]
async fn monter_et_descendre_echangent_deux_lignes(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    for genre in ["titre", "texte", "poste_libre"] {
        ajouter(&s, d, genre).await;
    }
    let l = ids(&pool, d).await;

    assert_eq!(poster(&s, &format!("/devis/{d}/lignes/{}/monter", l[2]), "").await.0, StatusCode::OK);
    assert_eq!(ids(&pool, d).await, vec![l[0], l[2], l[1]]);
    poster(&s, &format!("/devis/{d}/lignes/{}/descendre", l[0]), "").await;
    assert_eq!(ids(&pool, d).await, vec![l[2], l[0], l[1]]);

    // Monter la première ou descendre la dernière ne change rien (et ne plante pas).
    poster(&s, &format!("/devis/{d}/lignes/{}/monter", l[2]), "").await;
    poster(&s, &format!("/devis/{d}/lignes/{}/descendre", l[1]), "").await;
    assert_eq!(ids(&pool, d).await, vec![l[2], l[0], l[1]]);
}

#[sqlx::test]
async fn supprimer_une_ligne_recalcule_et_laisse_ajouter_ensuite(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "poste_libre").await;
    ajouter(&s, d, "poste_libre").await;
    let l = ids(&pool, d).await;
    poster(&s, &format!("/devis/{d}/lignes/{}", l[0]), "quantite=1&prix_unitaire=100").await;
    poster(&s, &format!("/devis/{d}/lignes/{}", l[1]), "quantite=1&prix_unitaire=40").await;
    assert_eq!(montants(&pool, d).await.0, 14_000);

    assert_eq!(poster(&s, &format!("/devis/{d}/lignes/{}/supprimer", l[0]), "").await.0, StatusCode::OK);
    assert_eq!(ids(&pool, d).await, vec![l[1]]);
    assert_eq!(montants(&pool, d).await, (4_000, 4_240));

    // Après un trou dans les positions, une nouvelle ligne arrive bien à la fin.
    ajouter(&s, d, "texte").await;
    let apres = ids(&pool, d).await;
    assert_eq!(apres.len(), 2);
    assert_eq!(apres[0], l[1]);
}

#[sqlx::test]
async fn on_ne_touche_pas_aux_lignes_d_un_autre_devis(pool: PgPool) {
    let a = brouillon(&pool).await;
    let b: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (2) RETURNING id").fetch_one(&pool).await.unwrap();
    let s = site(&pool);
    ajouter(&s, a, "texte").await;
    let ligne = ids(&pool, a).await[0];

    assert_eq!(poster(&s, &format!("/devis/{b}/lignes/{ligne}"), "description=piratage").await.0, StatusCode::NOT_FOUND);
    assert_eq!(poster(&s, &format!("/devis/{b}/lignes/{ligne}/supprimer"), "").await.0, StatusCode::OK);
    assert_eq!(poster(&s, &format!("/devis/{b}/lignes/{ligne}/monter"), "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(ids(&pool, a).await, vec![ligne]);
    let (description,): (String,) = sqlx::query_as("SELECT description FROM devis_lignes WHERE id = $1").bind(ligne).fetch_one(&pool).await.unwrap();
    assert_eq!(description, "");
}

#[sqlx::test]
async fn deux_ajouts_en_meme_temps_ont_deux_positions_differentes(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    let mut taches = Vec::new();
    for _ in 0..8 {
        let s = s.clone();
        taches.push(tokio::spawn(async move { ajouter(&s, d, "texte").await }));
    }
    for t in taches {
        assert_eq!(t.await.unwrap(), StatusCode::OK);
    }
    let positions: Vec<i32> = sqlx::query_scalar("SELECT position FROM devis_lignes WHERE devis_id = $1 ORDER BY position").bind(d).fetch_all(&pool).await.unwrap();
    assert_eq!(positions, (1..=8).collect::<Vec<i32>>());
}

// ─── Devis non modifiable, sécurité ─────────────────────────────────────────

#[sqlx::test]
async fn un_devis_envoye_est_en_lecture_seule(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "poste_libre").await;
    let ligne = ids(&pool, d).await[0];
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "quantite=1&prix_unitaire=100").await;
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(d).execute(&pool).await.unwrap();

    for (uri, corps) in [
        (format!("/devis/{d}/lignes"), "genre=texte"),
        (format!("/devis/{d}/lignes/{ligne}"), "quantite=99"),
        (format!("/devis/{d}/lignes/{ligne}/supprimer"), ""),
        (format!("/devis/{d}/lignes/{ligne}/monter"), ""),
    ] {
        assert_eq!(poster(&s, &uri, corps).await.0, StatusCode::CONFLICT, "{uri}");
    }
    assert_eq!(ids(&pool, d).await, vec![ligne]);
    assert_eq!(montants(&pool, d).await.0, 10_000);

    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(page.contains("Ce devis n'est plus un brouillon"));
    assert!(!page.contains("+ Poste libre") && !page.contains("hx-post"));
    assert!(page.contains("name=\"quantite\" value=\"1\" disabled"));
}

#[sqlx::test]
async fn les_textes_tapes_sont_neutralises(pool: PgPool) {
    let d = brouillon(&pool).await;
    let s = site(&pool);
    ajouter(&s, d, "texte").await;
    let ligne = ids(&pool, d).await[0];
    poster(&s, &format!("/devis/{d}/lignes/{ligne}"), "description=%3C%2Ftextarea%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E").await;
    let (_, page) = lire(&s, &format!("/devis/{d}/encodage")).await;
    assert!(!page.contains("<script>alert(1)"));
    assert!(page.contains("&#60;/textarea&#62;&#60;script&#62;alert(1)"));
}

#[sqlx::test]
async fn la_base_refuse_un_prix_sur_un_titre_et_deux_lignes_a_la_meme_place(pool: PgPool) {
    let d = brouillon(&pool).await;
    let res = sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, prix_unitaire_e5) VALUES ($1, 1, 'titre', 100)")
        .bind(d)
        .execute(&pool)
        .await;
    assert!(res.is_err());
    sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre) VALUES ($1, 1, 'texte')").bind(d).execute(&pool).await.unwrap();
    let res = sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre) VALUES ($1, 1, 'texte')").bind(d).execute(&pool).await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn supprimer_un_devis_supprime_ses_lignes(pool: PgPool) {
    let d = brouillon(&pool).await;
    ajouter(&site(&pool), d, "texte").await;
    sqlx::query("DELETE FROM devis WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM devis_lignes").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}
