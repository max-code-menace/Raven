// Test de la reprise des devis, avenants et chantiers de Vertuoza (données fictives).

mod commun;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use ubifacture::reprise;

/// Un devis de base accepté (lignes, TVA 6 %), son avenant en moins, un devis envoyé sans ligne, un refusé,
/// et deux factures reprises du chantier « PL45 ».
fn fichier() -> String {
    r#"{
  "devis": [
    {"numero": 41, "date": "2026-03-02", "statut": "accepte", "date_decision": "2026-03-10", "chantier_debut": "2026-04-01",
     "client": {"nom": "Laurence Exemple", "type": "contact", "email": "laurence@exemple.be"},
     "reference": "PL45", "chantier": "PL45", "responsable": "Mathieu Draux", "conditions_jours": 7, "tva_defaut": 600,
     "adresse_chantier": {"rue": "Rue du Chantier", "numero": "12", "code_postal": "1390", "ville": "Grez-Doiceau"},
     "remarque_1": "Acompte de 30 %",
     "lignes": [
       {"genre": "titre", "description": "Toiture"},
       {"genre": "poste", "description": "Dépose de la couverture", "quantite": "100", "unite": "m²", "prix_unitaire": "20,50"},
       {"genre": "fourniture", "description": "Ardoises", "quantite": "100", "unite": "m²", "prix_unitaire": "45", "prix_achat": "30", "remarque": "Naturelles"},
       {"genre": "titre", "description": "Électricité"},
       {"genre": "main_oeuvre", "description": "Tableau électrique", "quantite": "8", "unite": "h", "prix_unitaire": "55", "tva": 2100}
     ],
     "totaux": {"ht": "7 490,00", "tva": "529,50", "ttc": "8 019,50"}},
    {"genre": "avenant", "numero": 2, "devis_base": 41, "date": "2026-05-04", "statut": "accepte", "date_decision": "2026-05-06",
     "client": {"nom": "Laurence Exemple", "type": "contact"}, "reference": "PL45", "chantier": "PL45", "tva_defaut": 600,
     "jours_ouvrables": 3,
     "lignes": [{"genre": "poste", "description": "Fenêtre en moins", "quantite": "1", "unite": "pce", "prix_unitaire": "-1000"}],
     "totaux": {"ht": "-1 000,00", "tva": "-60,00", "ttc": "-1 060,00"}},
    {"numero": 45, "date": "2026-06-01", "statut": "envoye", "client": {"nom": "Client SA"}, "reference": "Rénovation cuisine",
     "totaux": {"ht": "10 000,00", "tva": "600,00", "ttc": "10 600,00"}},
    {"numero": 47, "date": "2026-06-10", "statut": "refuse", "date_decision": "2026-06-20", "motif_refus": "Trop cher",
     "client": {"nom": "Client SA"}, "reference": "Terrasse", "tva_defaut": 2100,
     "ajustement": {"libelle": "Geste commercial", "mode": "pourcentage", "valeur": "-10"},
     "lignes": [{"description": "Terrasse en bois", "quantite": "1", "unite": "fft", "prix_unitaire": "1000"}],
     "totaux": {"ht": "900,00", "tva": "189,00", "ttc": "1 089,00"}}
  ],
  "factures": [
    {"genre": "acompte", "numero": 120, "date": "2026-03-15", "echeance": "2026-03-22", "client": {"nom": "Laurence Exemple", "type": "contact"},
     "reference": "PL45", "chantier": "PL45", "conditions_jours": 7,
     "lignes": [{"description": "Acompte de 30 %", "quantite": "1", "prix_unitaire": "1947,00", "tva": 600, "total": "1947,00"}],
     "totaux": {"ht": "1 947,00", "tva": "116,82", "ttc": "2 063,82"}},
    {"genre": "simple", "numero": 121, "date": "2026-03-16", "echeance": "2026-03-23", "client": {"nom": "Client SA"}, "chantier": "AUTRE",
     "lignes": [{"description": "Dépannage", "quantite": "1", "prix_unitaire": "100", "tva": 2100, "total": "100"}],
     "totaux": {"ht": "100", "tva": "21", "ttc": "121"}}
  ]
}"#
    .to_string()
}

#[sqlx::test]
async fn reprendre_les_devis_avenants_et_chantiers(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let texte = fichier();

    // Essai : le rapport, mais rien n'est gardé.
    let r = reprise::importer(&pool, 1, &texte, false).await.unwrap();
    assert_eq!((r.devis, r.avenants, r.chantiers, r.devis_deja, r.enregistre), (3, 1, 1, 0, false));
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM devis").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 0, "un essai n'enregistre rien");

    // Enregistrer.
    let r = reprise::importer(&pool, 1, &texte, true).await.unwrap();
    let rapport = r.to_string();
    assert!(rapport.contains("REPRISE ENREGISTRÉE"), "{rapport}");
    assert_eq!((r.devis, r.avenants, r.chantiers, r.factures_rattachees), (3, 1, 1, 1), "{rapport}");
    // 7 490,00 : 100 × 20,50 + 100 × 45 + 8 × 55 = 2 050 + 4 500 + 440 = 6 990 ; aucune alerte de total n'est attendue.
    // (Le total du fichier ci-dessus est volontairement faux : l'alerte doit le dire.)
    let attendu = format!("Devis n° 2026-41 : les lignes donnent {} HT", ubifacture::commun::format_euros(699_000));
    assert!(rapport.contains(&attendu), "{rapport}");

    // Le devis de base : statut, décision, client, lignes, totaux calculés par l'application.
    let (id, statut, acceptation, ht, ttc, repris, ch_vertuoza, entreprise, contact): (i64, String, Option<chrono::NaiveDate>, i64, i64, bool, String, Option<i64>, Option<i64>) =
        sqlx::query_as(
            "SELECT id, statut::text, date_acceptation, montant_ht_cents, montant_ttc_cents, reprise, chantier_vertuoza, client_entreprise_id, client_contact_id
             FROM devis WHERE numero = 41 AND chantier_id IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((statut.as_str(), repris, ch_vertuoza.as_str()), ("accepte", true, "PL45"));
    assert_eq!(acceptation, chrono::NaiveDate::from_ymd_opt(2026, 3, 10));
    assert!(entreprise.is_none() && contact.is_some(), "Laurence est un contact");
    // 6 990 € HT : 6 550 à 6 % + 440 à 21 % → TVA 393,00 + 92,40 = 485,40.
    assert_eq!((ht, ttc), (699_000, 747_540));
    let lignes: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_lignes WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(lignes, 5);
    let (genre, achat, remarque): (String, Option<i64>, String) =
        sqlx::query_as("SELECT genre::text, prix_achat_unitaire_e5, remarque FROM devis_lignes WHERE devis_id = $1 AND position = 3").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((genre.as_str(), achat, remarque.as_str()), ("fourniture", Some(3_000_000), "Naturelles"));
    let adresse: (bool, String, String) =
        sqlx::query_as("SELECT chantier_identique, chantier_rue, chantier_ville FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(adresse, (false, "Rue du Chantier".to_string(), "Grez-Doiceau".to_string()));

    // Le chantier : créé, début donné par le fichier, factures de « PL45 » rattachées (pas celle d'« AUTRE »).
    let (chantier, debut, ch_statut): (i64, chrono::NaiveDate, String) =
        sqlx::query_as("SELECT id, date_debut, statut::text FROM chantiers WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((debut, ch_statut.as_str()), (chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(), "en_cours"));
    let rattachees: Vec<(i64, Option<i64>)> = sqlx::query_as("SELECT numero, chantier_id FROM factures ORDER BY numero").fetch_all(&pool).await.unwrap();
    assert_eq!(rattachees, vec![(120, Some(chantier)), (121, None)]);
    assert!(rapport.contains("AUTRE"), "la facture sans chantier est signalée : {rapport}");

    // L'avenant : dans le chantier, n° 2, 3 jours, compte dans le total du chantier.
    let (numero, jours, statut, ht): (i64, i32, String, i64) =
        sqlx::query_as("SELECT numero, jours_ouvrables, statut::text, montant_ht_cents FROM devis WHERE chantier_id = $1").bind(chantier).fetch_one(&pool).await.unwrap();
    assert_eq!((numero, jours, statut.as_str(), ht), (2, 3, "accepte", -100_000));
    let recap = ubifacture::avenants::recap(&mut *pool.acquire().await.unwrap(), chantier).await.unwrap();
    assert_eq!((recap.acceptes_ht_cents, recap.jours_acceptes), (-100_000, 3));

    // Devis envoyé sans ligne : le total de Vertuoza est gardé, aucun chantier.
    let (statut, ht, ttc): (String, i64, i64) =
        sqlx::query_as("SELECT statut::text, montant_ht_cents, montant_ttc_cents FROM devis WHERE numero = 45").fetch_one(&pool).await.unwrap();
    assert_eq!((statut.as_str(), ht, ttc), ("envoye", 1_000_000, 1_060_000));
    // Devis refusé : motif, remise de 10 % appliquée par l'application (1 000 → 900 HT, TVA 21 % = 189).
    let (statut, refus, motif, ht, ttc): (String, Option<chrono::NaiveDate>, String, i64, i64) =
        sqlx::query_as("SELECT statut::text, date_refus, motif_refus, montant_ht_cents, montant_ttc_cents FROM devis WHERE numero = 47")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((statut.as_str(), refus, motif.as_str(), ht, ttc), ("refuse", chrono::NaiveDate::from_ymd_opt(2026, 6, 20), "Trop cher", 90_000, 108_900));
    let chantiers: i64 = sqlx::query_scalar("SELECT count(*) FROM chantiers").fetch_one(&pool).await.unwrap();
    assert_eq!(chantiers, 1, "seul un devis accepté a un chantier");

    // L'historique du devis de base.
    let journal: Vec<(String, String)> = sqlx::query_as("SELECT genre, detail FROM devis_evenements WHERE devis_id = $1 ORDER BY id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(journal.len(), 3, "{journal:?}");
    assert!(journal[0].1.contains("Repris de Vertuoza"));
    assert_eq!(journal[2], ("chantier".to_string(), "Chantier n° 1 créé".to_string()));

    // Le compteur de devis monte au plus grand numéro repris.
    let compteur: i64 = sqlx::query_scalar("SELECT valeur FROM compteurs WHERE nom = 'devis'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, 47);

    // Relancer : rien en double.
    let r = reprise::importer(&pool, 1, &texte, true).await.unwrap();
    assert_eq!((r.devis, r.avenants, r.chantiers, r.devis_deja), (0, 0, 0, 4));
    let (devis, chantiers): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM devis), (SELECT count(*) FROM chantiers)").fetch_one(&pool).await.unwrap();
    assert_eq!((devis, chantiers), (4, 1));
}

#[sqlx::test]
async fn un_devis_cree_dans_ubifacture_avec_le_meme_numero_bloque_tout(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO devis (numero) VALUES (41)").execute(&pool).await.unwrap();
    let erreur = reprise::importer(&pool, 1, &fichier(), true).await.unwrap_err();
    assert!(erreur.contains("existe déjà dans Ubifacture"), "{erreur}");
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM devis WHERE reprise").fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 0, "rien n'est importé");
}

#[sqlx::test]
async fn les_fichiers_invalides_sont_refuses_sans_rien_enregistrer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    // Un avenant dont le devis de base n'est pas dans la reprise.
    let sans_base = r#"{"devis": [{"genre": "avenant", "numero": 2, "devis_base": 99, "date": "2026-05-04", "client": {"nom": "X"},
                                   "totaux": {"ht": "0", "tva": "0", "ttc": "0"}}]}"#;
    let e = reprise::importer(&pool, 1, sans_base, true).await.unwrap_err();
    assert!(e.contains("n'est pas dans la reprise"), "{e}");
    // Un devis de base qui n'est pas accepté ne porte pas d'avenant.
    let non_accepte = r#"{"devis": [
        {"numero": 1, "date": "2026-05-01", "statut": "envoye", "client": {"nom": "X"}, "totaux": {"ht": "0", "tva": "0", "ttc": "0"}},
        {"genre": "avenant", "numero": 2, "devis_base": 1, "date": "2026-05-04", "client": {"nom": "X"}, "totaux": {"ht": "0", "tva": "0", "ttc": "0"}}]}"#;
    let e = reprise::importer(&pool, 1, non_accepte, true).await.unwrap_err();
    assert!(e.contains("n'est pas accepté"), "{e}");
    // Statut, type de ligne et montants inconnus.
    for (morceau, attendu) in [
        (r#""statut": "perdu""#, "statut inconnu"),
        (r#""lignes": [{"genre": "magie"}]"#, "type de ligne inconnu"),
        (r#""lignes": [{"quantite": "beaucoup"}]"#, "n'est pas un nombre"),
        (r#""totaux": {"ht": "x", "tva": "0", "ttc": "0"}"#, "ne sont pas des montants"),
    ] {
        // Les totaux valides, sauf si le cas teste justement les totaux.
        let totaux = if morceau.starts_with(r#""totaux""#) { "" } else { r#", "totaux": {"ht": "0", "tva": "0", "ttc": "0"}"# };
        let texte = format!(r#"{{"devis": [{{"numero": 1, "date": "2026-05-01", "client": {{"nom": "X"}}, {morceau}{totaux}}}]}}"#);
        let e = reprise::importer(&pool, 1, &texte, true).await.unwrap_err();
        assert!(e.contains(attendu), "{morceau} → {e}");
    }
    let (devis, clients): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM devis), (SELECT count(*) FROM entreprises)").fetch_one(&pool).await.unwrap();
    assert_eq!((devis, clients), (0, 0), "une erreur annule tout");
}

/// Les écrans existants ouvrent les devis, avenants et chantiers repris ; un avancement peut être commencé
/// sur les lignes reprises (avec l'avenant accepté).
#[sqlx::test]
async fn les_ecrans_ouvrent_les_devis_et_chantiers_repris(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use ubifacture::{AppState, app};

    reprise::importer(&pool, 1, &fichier(), true).await.unwrap();
    let (devis, avenant, chantier): (i64, i64, i64) = sqlx::query_as(
        "SELECT d.id, (SELECT id FROM devis WHERE chantier_id = ch.id), ch.id FROM chantiers ch JOIN devis d ON d.id = ch.devis_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let appel = |methode: &'static str, chemin: String| {
        let pool = pool.clone();
        async move {
            let r = app(AppState { db: pool })
                .oneshot(
                    Request::builder()
                        .method(methode)
                        .uri(chemin.clone())
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let statut = r.status();
            let texte = String::from_utf8_lossy(&r.into_body().collect().await.unwrap().to_bytes()).to_string();
            (statut, texte, chemin)
        }
    };
    for chemin in [
        "/devis".to_string(),
        format!("/devis/{devis}"),
        format!("/devis/{devis}/encodage"),
        format!("/devis/{devis}/totaux"),
        format!("/devis/{devis}/apercu"),
        format!("/devis/{devis}/pdf"),
        format!("/devis/{avenant}"),
        format!("/devis/{avenant}/pdf"),
        "/chantiers".to_string(),
        format!("/chantiers/{chantier}"),
        format!("/chantiers/{chantier}/devis"),
        format!("/chantiers/{chantier}/finances"),
        "/factures".to_string(),
    ] {
        let (statut, _, chemin) = appel("GET", chemin).await;
        assert_eq!(statut, StatusCode::OK, "{chemin}");
    }
    // Le devis repris est verrouillé comme tout devis accepté : pas de nouvelle ligne.
    let (statut, _, _) = appel("POST", format!("/devis/{devis}/lignes")).await;
    assert!(statut.is_client_error(), "un devis accepté ne se modifie pas : {statut}");
    // Un avancement démarre, et sa page montre les lignes du devis repris et celles de l'avenant accepté.
    let (statut, _, _) = appel("POST", format!("/chantiers/{chantier}/avancements")).await;
    assert!(statut.is_redirection(), "{statut}");
    let numero: i64 = sqlx::query_scalar("SELECT numero::bigint FROM avancements WHERE chantier_id = $1").bind(chantier).fetch_one(&pool).await.unwrap();
    let (statut, page, _) = appel("GET", format!("/chantiers/{chantier}/avancements/{numero}")).await;
    assert_eq!(statut, StatusCode::OK);
    for texte in ["Dépose de la couverture", "Ardoises", "Tableau électrique", "Fenêtre en moins"] {
        assert!(page.contains(texte), "la page d'avancement doit montrer « {texte} »");
    }
}

/// La numérotation est annuelle : le n° 41 de 2025 et le n° 41 de 2026 sont deux devis ; le compteur suit l'année la plus récente.
#[sqlx::test]
async fn deux_annees_peuvent_avoir_le_meme_numero(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let texte = r#"{"devis": [
        {"numero": 41, "date": "2025-11-02", "statut": "envoye", "client": {"nom": "X"}, "totaux": {"ht": "100", "tva": "6", "ttc": "106"}},
        {"numero": 41, "date": "2026-03-02", "statut": "envoye", "client": {"nom": "X"}, "totaux": {"ht": "200", "tva": "12", "ttc": "212"}},
        {"numero": 3, "date": "2026-04-02", "statut": "accepte", "date_decision": "2026-04-10", "client": {"nom": "X"}, "chantier": "PL1",
         "totaux": {"ht": "300", "tva": "18", "ttc": "318"}},
        {"genre": "avenant", "numero": 2, "devis_base": 3, "date": "2026-05-02", "client": {"nom": "X"}, "totaux": {"ht": "50", "tva": "3", "ttc": "53"}}
    ]}"#;
    let r = reprise::importer(&pool, 1, texte, true).await.unwrap();
    assert_eq!((r.devis, r.avenants, r.chantiers), (3, 1, 1), "{r}");
    let compteur: (i64, Option<i32>) = sqlx::query_as("SELECT valeur, annee FROM compteurs WHERE nom = 'devis'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, (41, Some(2026)), "le plus grand numéro de la dernière année reprise");
    // Relancer : rien en double, même avec deux devis n° 41.
    let r = reprise::importer(&pool, 1, texte, true).await.unwrap();
    assert_eq!((r.devis, r.avenants, r.devis_deja), (0, 0, 4), "{r}");
}
