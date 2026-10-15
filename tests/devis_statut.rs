// Tests de l'étape 5c-2 et 5c-3 : accepter / refuser, remettre en brouillon, dupliquer, historique.

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

/// Un devis daté du 1er octobre 2026, dans le statut demandé.
async fn devis(pool: &PgPool, numero: i64, statut: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO devis (numero, date_devis, statut) VALUES ($1, '2026-10-01', $2::text::statut_devis) RETURNING id")
        .bind(numero)
        .bind(statut)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn statut_de(pool: &PgPool, id: i64) -> String {
    sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1").bind(id).fetch_one(pool).await.unwrap()
}

async fn evenements(pool: &PgPool, id: i64) -> Vec<(String, String)> {
    sqlx::query_as("SELECT genre, detail FROM devis_evenements WHERE devis_id = $1 ORDER BY id").bind(id).fetch_all(pool).await.unwrap()
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date::text").fetch_one(pool).await.unwrap()
}

// ─── Marquer comme envoyé, remettre en brouillon ────────────────────────────

#[sqlx::test]
async fn marquer_comme_envoye_puis_remettre_en_brouillon(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let s = site(&pool);

    let (statut, lieu) = poster(&s, &format!("/devis/{d}/statut/envoye"), "").await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{d}?ok=envoye"))));
    assert_eq!(statut_de(&pool, d).await, "envoye");
    let (_, page) = lire(&s, &format!("/devis/{d}?ok=envoye")).await;
    assert!(page.contains("Le devis est marqué comme envoyé.") && page.contains("Remettre en brouillon"));
    // Envoyé : on ne peut plus le marquer envoyé une seconde fois.
    assert!(!page.contains("Marquer comme envoyé</button>"));

    let (statut, _) = poster(&s, &format!("/devis/{d}/statut/envoye"), "").await;
    assert_eq!(statut, StatusCode::CONFLICT);

    poster(&s, &format!("/devis/{d}/statut/brouillon"), "").await;
    assert_eq!(statut_de(&pool, d).await, "brouillon");

    let journal = evenements(&pool, d).await;
    assert_eq!(journal.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(), vec!["statut", "statut"]);
    assert_eq!(journal[0].1, "Brouillon → Envoyé");
    assert_eq!(journal[1].1, "Envoyé → Brouillon");
}

// ─── Accepter ───────────────────────────────────────────────────────────────

#[sqlx::test]
async fn accepter_enregistre_la_date_et_verrouille(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    let s = site(&pool);
    let (statut, lieu) = poster(&s, &format!("/devis/{d}/statut/accepter"), "date=2026-10-05").await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{d}?ok=accepte"))));

    let (st, date, refus): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT statut::text, date_acceptation::text, date_refus::text FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((st.as_str(), date.as_deref(), refus), ("accepte", Some("2026-10-05"), None));

    let (_, page) = lire(&s, &format!("/devis/{d}?ok=accepte")).await;
    assert!(page.contains("Le devis est accepté et son chantier est créé.") && page.contains("Accepté le 05/10/2026"));
    assert!(page.contains("Chantier n° 1"));
    // Plus aucune décision possible ; seulement dupliquer.
    for absent in ["Marquer comme envoyé</button>", "Accepter…", "Refuser…", "Remettre en brouillon"] {
        assert!(!page.contains(absent), "{absent}");
    }
    assert!(page.contains("Dupliquer"));

    for action in ["envoye", "accepter", "refuser", "brouillon"] {
        let (statut, _) = poster(&s, &format!("/devis/{d}/statut/{action}"), "date=2026-10-06").await;
        assert_eq!(statut, StatusCode::CONFLICT, "{action}");
    }
    assert_eq!(statut_de(&pool, d).await, "accepte");
    assert_eq!(
        evenements(&pool, d).await,
        vec![
            ("statut".to_string(), "Envoyé → Accepté (décision du 05/10/2026)".to_string()),
            ("chantier".to_string(), "Chantier n° 1 créé".to_string()),
        ]
    );
}

#[sqlx::test]
async fn un_brouillon_peut_etre_accepte_directement(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let (statut, _) = poster(&site(&pool), &format!("/devis/{d}/statut/accepter"), "date=2026-10-01").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(statut_de(&pool, d).await, "accepte");
}

#[sqlx::test]
async fn la_date_de_decision_est_verifiee(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    let s = site(&pool);
    let demain: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date + 1)::text").fetch_one(&pool).await.unwrap();
    for date in ["2026-09-30", demain.as_str(), "pas-une-date", "", "2026-13-45"] {
        let (statut, lieu) = poster(&s, &format!("/devis/{d}/statut/accepter"), &format!("date={date}")).await;
        assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{d}?erreur=date"))), "{date}");
        assert_eq!(statut_de(&pool, d).await, "envoye", "{date}");
    }
    let (_, page) = lire(&s, &format!("/devis/{d}?erreur=date")).await;
    assert!(page.contains("comprise entre la date du devis et aujourd&#39;hui") || page.contains("comprise entre la date du devis et aujourd'hui"));
    assert!(evenements(&pool, d).await.is_empty(), "un refus de date n'écrit rien dans le journal");
    // Le jour même est permis.
    let (statut, _) = poster(&s, &format!("/devis/{d}/statut/accepter"), &format!("date={}", aujourdhui(&pool).await)).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    // Le changement de statut, puis la création du chantier.
    assert_eq!(evenements(&pool, d).await.len(), 2);
}

// ─── Refuser ────────────────────────────────────────────────────────────────

#[sqlx::test]
async fn refuser_enregistre_la_date_et_le_motif(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    let s = site(&pool);
    let (statut, lieu) = poster(&s, &format!("/devis/{d}/statut/refuser"), "date=2026-10-07&motif=Trop+cher").await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{d}?ok=refuse"))));

    let (st, date, motif): (String, Option<String>, String) =
        sqlx::query_as("SELECT statut::text, date_refus::text, motif_refus FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((st.as_str(), date.as_deref(), motif.as_str()), ("refuse", Some("2026-10-07"), "Trop cher"));
    let (_, page) = lire(&s, &format!("/devis/{d}")).await;
    assert!(page.contains("Refusé le 07/10/2026 : Trop cher"));
    assert_eq!(evenements(&pool, d).await[0].1, "Envoyé → Refusé (décision du 07/10/2026) : Trop cher");
}

#[sqlx::test]
async fn un_motif_trop_long_est_refuse(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    let (statut, lieu) = poster(&site(&pool), &format!("/devis/{d}/statut/refuser"), &format!("date=2026-10-07&motif={}", "x".repeat(501))).await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{d}?erreur=motif"))));
    assert_eq!(statut_de(&pool, d).await, "envoye");
}

#[sqlx::test]
async fn un_refus_peut_etre_remis_en_brouillon_et_efface_la_decision(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    let s = site(&pool);
    poster(&s, &format!("/devis/{d}/statut/refuser"), "date=2026-10-07&motif=Trop+cher").await;
    poster(&s, &format!("/devis/{d}/statut/brouillon"), "").await;
    let (st, date, motif): (String, Option<String>, String) =
        sqlx::query_as("SELECT statut::text, date_refus::text, motif_refus FROM devis WHERE id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!((st.as_str(), date, motif.as_str()), ("brouillon", None, ""));
    // Mais l'historique garde la trace du refus.
    let (_, page) = lire(&s, &format!("/devis/{d}/historique")).await;
    assert!(page.contains("Trop cher") && page.contains("Refusé"));
}

#[sqlx::test]
async fn un_devis_signe_ne_peut_etre_qu_accepte(pool: PgPool) {
    let d = devis(&pool, 1, "signe").await;
    let s = site(&pool);
    assert_eq!(poster(&s, &format!("/devis/{d}/statut/refuser"), "date=2026-10-07").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/devis/{d}/statut/accepter"), "date=2026-10-07").await.0, StatusCode::SEE_OTHER);
}

#[sqlx::test]
async fn un_devis_inconnu_est_introuvable(pool: PgPool) {
    let s = site(&pool);
    for action in ["envoye", "accepter", "refuser", "brouillon"] {
        assert_eq!(poster(&s, &format!("/devis/999/statut/{action}"), "date=2026-10-07").await.0, StatusCode::NOT_FOUND, "{action}");
    }
    assert_eq!(poster(&s, "/devis/999/dupliquer", "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(lire(&s, "/devis/999/historique").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn les_messages_de_l_adresse_ne_sont_pas_affiches_tels_quels(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}?ok=%3Cscript%3Ealert(1)%3C%2Fscript%3E&erreur=%3Cb%3E")).await;
    assert!(!page.contains("<script>alert(1)") && !page.contains("<b>"));
}

#[sqlx::test]
async fn les_boutons_d_un_brouillon(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}")).await;
    for present in ["Marquer comme envoyé", "Accepter…", "Refuser…", "Dupliquer"] {
        assert!(page.contains(present), "{present}");
    }
    assert!(!page.contains("Remettre en brouillon"));
    // La date proposée est bornée par la date du devis.
    assert!(page.contains("min=\"2026-10-01\""));
}

// ─── Dupliquer ──────────────────────────────────────────────────────────────

#[sqlx::test]
async fn dupliquer_copie_le_devis_en_brouillon_avec_le_numero_suivant(pool: PgPool) {
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(&pool).await.unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Dupont SRL') RETURNING id").fetch_one(&pool).await.unwrap();
    let source: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, reference, responsable, statut, client_entreprise_id, taux_tva_id, remarque_1,
                            ajustement_libelle, ajustement_mode, ajustement_valeur, marge_activee, frais_generaux_centiemes,
                            montant_ht_cents, montant_ttc_cents, date_acceptation)
         VALUES (5, '2026-10-01', 'C44 Volet', 'Marc', 'accepte', $1, $2, 'Acompte 30 %', 'Geste', 'montant', -1000, true, 750,
                 12345, 13086, '2026-10-05') RETURNING id",
    )
    .bind(client)
    .bind(tva)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE compteurs SET valeur = 5 WHERE nom = 'devis'").execute(&pool).await.unwrap();
    for (position, genre, texte) in [(1, "titre", "Lot A"), (2, "poste_libre", "Volet")] {
        let chiffre = genre == "poste_libre";
        sqlx::query(
            "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, taux_tva_id, remarque)
             VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(source)
        .bind(position)
        .bind(genre)
        .bind(texte)
        .bind(chiffre.then_some(250_000_i64))
        .bind(if chiffre { "pce" } else { "" })
        .bind(chiffre.then_some(1_999_000_i64))
        .bind(chiffre.then_some(1_200_000_i64))
        .bind(chiffre.then_some(tva))
        .bind("Une remarque")
        .execute(&pool)
        .await
        .unwrap();
    }

    let (statut, lieu) = poster(&site(&pool), &format!("/devis/{source}/dupliquer"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let copie: i64 = lieu.unwrap().trim_start_matches("/devis/").parse().unwrap();
    assert_ne!(copie, source);

    let (numero, st, reference, resp, client_copie, remarque): (i64, String, String, String, Option<i64>, String) = sqlx::query_as(
        "SELECT numero, statut::text, reference, responsable, client_entreprise_id, remarque_1 FROM devis WHERE id = $1",
    )
    .bind(copie)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((numero, st.as_str(), reference.as_str(), resp.as_str()), (6, "brouillon", "C44 Volet", "Marc"));
    assert_eq!((client_copie, remarque.as_str()), (Some(client), "Acompte 30 %"));
    let (ajust, marge, frais, ht, ttc, acceptation): (String, bool, i32, i64, i64, Option<String>) = sqlx::query_as(
        "SELECT ajustement_mode, marge_activee, frais_generaux_centiemes, montant_ht_cents, montant_ttc_cents, date_acceptation::text FROM devis WHERE id = $1",
    )
    .bind(copie)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((ajust.as_str(), marge, frais, ht, ttc), ("montant", true, 750, 12_345, 13_086));
    assert_eq!(acceptation, None, "la copie ne garde pas la décision du client");
    let date: String = sqlx::query_scalar("SELECT date_devis::text FROM devis WHERE id = $1").bind(copie).fetch_one(&pool).await.unwrap();
    assert_eq!(date, aujourdhui(&pool).await, "la copie est datée d'aujourd'hui");

    // Les lignes sont copiées à l'identique, dans le même ordre.
    let lignes: Vec<(String, String, Option<i64>, Option<i64>, Option<i64>, String)> = sqlx::query_as(
        "SELECT genre::text, description, quantite_e5, prix_unitaire_e5, prix_achat_unitaire_e5, remarque FROM devis_lignes WHERE devis_id = $1 ORDER BY position",
    )
    .bind(copie)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(lignes.len(), 2);
    assert_eq!((lignes[0].0.as_str(), lignes[0].1.as_str()), ("titre", "Lot A"));
    assert_eq!(
        (lignes[1].0.as_str(), lignes[1].2, lignes[1].3, lignes[1].4, lignes[1].5.as_str()),
        ("poste_libre", Some(250_000), Some(1_999_000), Some(1_200_000), "Une remarque")
    );

    // L'original n'a pas changé ; les deux journaux se répondent.
    assert_eq!(statut_de(&pool, source).await, "accepte");
    let annee: i32 = sqlx::query_scalar("SELECT extract(year FROM date_devis)::int FROM devis WHERE id = $1").bind(copie).fetch_one(&pool).await.unwrap();
    assert_eq!(evenements(&pool, source).await, vec![("duplication".to_string(), format!("Dupliqué vers le devis {annee}-6"))]);
    assert_eq!(evenements(&pool, copie).await, vec![("creation".to_string(), "Devis créé par duplication du devis 2026-5".to_string())]);
}

#[sqlx::test]
async fn dupliquer_deux_fois_donne_deux_numeros_et_aucun_trou(pool: PgPool) {
    let d = devis(&pool, 1, "refuse").await;
    sqlx::query("UPDATE compteurs SET valeur = 1 WHERE nom = 'devis'").execute(&pool).await.unwrap();
    let s = site(&pool);
    for _ in 0..2 {
        assert_eq!(poster(&s, &format!("/devis/{d}/dupliquer"), "").await.0, StatusCode::SEE_OTHER);
    }
    let numeros: Vec<i64> = sqlx::query_scalar("SELECT numero FROM devis ORDER BY numero").fetch_all(&pool).await.unwrap();
    assert_eq!(numeros, vec![1, 2, 3]);
}

#[sqlx::test]
async fn douze_duplications_en_meme_temps_donnent_douze_numeros_differents(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    sqlx::query("UPDATE compteurs SET valeur = 1 WHERE nom = 'devis'").execute(&pool).await.unwrap();
    let s = site(&pool);
    let mut taches = Vec::new();
    for _ in 0..12 {
        let s = s.clone();
        taches.push(tokio::spawn(async move { poster(&s, &format!("/devis/{d}/dupliquer"), "").await.0 }));
    }
    for t in taches {
        assert_eq!(t.await.unwrap(), StatusCode::SEE_OTHER);
    }
    let numeros: Vec<i64> = sqlx::query_scalar("SELECT numero FROM devis ORDER BY numero").fetch_all(&pool).await.unwrap();
    assert_eq!(numeros, (1..=13).collect::<Vec<i64>>());
}

#[sqlx::test]
async fn la_liste_propose_dupliquer(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let (_, liste) = lire(&site(&pool), "/devis").await;
    assert!(liste.contains(&format!("action=\"/devis/{d}/dupliquer\"")));
}

// ─── Historique ─────────────────────────────────────────────────────────────

#[sqlx::test]
async fn nouveau_ecrit_la_creation_dans_le_journal(pool: PgPool) {
    let s = site(&pool);
    let (statut, lieu) = poster(&s, "/devis", "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let id: i64 = lieu.unwrap().trim_start_matches("/devis/").parse().unwrap();
    assert_eq!(evenements(&pool, id).await, vec![("creation".to_string(), "Devis créé".to_string())]);
}

#[sqlx::test]
async fn la_page_historique_montre_les_evenements_du_plus_recent_au_plus_ancien(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    sqlx::query("INSERT INTO devis_evenements (devis_id, genre, detail, created_at) VALUES ($1, 'creation', 'Devis créé', '2026-10-01 09:00+02'), ($1, 'statut', 'Brouillon → Envoyé', '2026-10-02 14:30+02')")
        .bind(d)
        .execute(&pool)
        .await
        .unwrap();
    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/historique")).await;
    assert_eq!(statut, StatusCode::OK);
    // Les dates sont celles de Belgique, au format belge.
    assert!(page.contains("02/10/2026 · 14:30") && page.contains("01/10/2026 · 09:00"));
    assert!(page.find("Brouillon → Envoyé").unwrap() < page.find("Devis créé").unwrap());
    assert!(page.contains(&format!("tab tab-active\" href=\"/devis/{d}/historique\"")));
}

#[sqlx::test]
async fn l_historique_d_un_devis_vide_ne_plante_pas(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let (statut, page) = lire(&site(&pool), &format!("/devis/{d}/historique")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Rien à signaler."));
}

#[sqlx::test]
async fn les_textes_de_l_historique_sont_neutralises(pool: PgPool) {
    let d = devis(&pool, 1, "envoye").await;
    poster(&site(&pool), &format!("/devis/{d}/statut/refuser"), "date=2026-10-07&motif=%3Cscript%3Ealert(1)%3C%2Fscript%3E").await;
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}/historique")).await;
    assert!(!page.contains("<script>alert(1)"));
    let (_, entete) = lire(&site(&pool), &format!("/devis/{d}")).await;
    assert!(!entete.contains("<script>alert(1)"));
}

#[sqlx::test]
async fn seul_le_telechargement_du_pdf_est_note(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let s = site(&pool);
    assert_eq!(lire(&s, &format!("/devis/{d}/pdf")).await.0, StatusCode::OK);
    assert!(evenements(&pool, d).await.is_empty(), "afficher le PDF dans l'onglet n'est pas noté");
    assert_eq!(lire(&s, &format!("/devis/{d}/pdf?telecharger=1")).await.0, StatusCode::OK);
    assert_eq!(evenements(&pool, d).await, vec![("pdf".to_string(), "PDF téléchargé".to_string())]);
}

#[sqlx::test]
async fn la_base_refuse_un_genre_d_evenement_inconnu_et_supprime_le_journal_avec_le_devis(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let res = sqlx::query("INSERT INTO devis_evenements (devis_id, genre) VALUES ($1, 'bidon')").bind(d).execute(&pool).await;
    assert!(res.is_err());
    sqlx::query("INSERT INTO devis_evenements (devis_id, genre, detail) VALUES ($1, 'pdf', 'x')").bind(d).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM devis WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM devis_evenements").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test]
async fn les_cinq_onglets_sont_relies(pool: PgPool) {
    let d = devis(&pool, 1, "brouillon").await;
    let (_, page) = lire(&site(&pool), &format!("/devis/{d}")).await;
    for lien in ["", "/encodage", "/totaux", "/apercu", "/historique"] {
        assert!(page.contains(&format!("href=\"/devis/{d}{lien}\"")), "{lien}");
    }
}
