// Tests de l'étape 7-3 : notes de crédit (créditer une facture validée, crédit partiel, numérotation propre) et PDF.

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

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>, Option<String>) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let disposition = r.headers().get(header::CONTENT_DISPOSITION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    (statut, lieu, octets, disposition)
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps, _) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, String::from_utf8_lossy(&corps).to_string())
}

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let (s, lieu, _, _) = envoyer(site, requete).await;
    (s, lieu)
}

fn id_depuis(lieu: Option<String>) -> i64 {
    lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap()
}

/// Une facture simple validée : 16 h à 50 € (21 %) + 2,5 × 19,99 € (6 %) = 1 020,98 € TTC. Renvoie son id.
async fn facture_validee(pool: &PgPool, s: &Router) -> i64 {
    sqlx::query(
        "INSERT INTO societe (nom, nom_abrege, rue, numero, code_postal, ville, email, telephone, numero_tva)
         VALUES ('Ubique SRL', 'Ubique', 'Rue du Test', '1', '1000', 'Bruxelles', 'info@exemple.be', '02 000 00 00', 'BE0123456789')",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ('BE68 5390 0754 7034', true)").execute(pool).await.unwrap();
    let client: i64 = sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, rue, numero, code_postal, ville, numero_tva)
         VALUES ('Client SA', '{client}', 'Avenue Louise', '10', '1050', 'Ixelles', 'BE0987654321') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let conditions: i64 =
        sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30").fetch_one(pool).await.unwrap();
    let (_, lieu) = poster(s, "/factures", "").await;
    let id = id_depuis(lieu);
    let aujourdhui: String =
        sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference=Petits+travaux&responsable=Max&client=e{client}&conditions_paiement_id={conditions}")).await;
    for (description, quantite, prix, taux) in [("Main+d%27oeuvre", "16", "50", 2100), ("Fournitures", "2,5", "19,99", 600)] {
        poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
        let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
        let t: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(taux).fetch_one(pool).await.unwrap();
        poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description={description}&quantite={quantite}&unite=h&prix_unitaire={prix}&taux_tva_id={t}")).await;
    }
    let (statut, _) = poster(s, &format!("/factures/{id}/valider"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    id
}

async fn ttc(pool: &PgPool, id: i64) -> i64 {
    sqlx::query_scalar("SELECT montant_ttc_cents FROM factures WHERE id = $1").bind(id).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn crediter_entierement_une_facture(pool: PgPool) {
    let s = site(&pool);
    let facture = facture_validee(&pool, &s).await;

    // Un brouillon ne se crédite pas.
    let (_, lieu) = poster(&s, "/factures", "").await;
    assert_eq!(poster(&s, &format!("/factures/{}/crediter", id_depuis(lieu)), "").await.0, StatusCode::CONFLICT);

    let (statut, lieu) = poster(&s, &format!("/factures/{facture}/crediter"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert!(lieu.as_deref().unwrap().ends_with("?ok=credit"));
    let note = id_depuis(lieu);
    let (genre, origine, reference, numero): (String, Option<i64>, String, Option<i64>) =
        sqlx::query_as("SELECT genre::text, facture_origine_id, reference, numero FROM factures WHERE id = $1")
            .bind(note)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((genre.as_str(), origine, reference.as_str(), numero), ("note_credit", Some(facture), "Note de crédit pour la facture n° 125", None));
    assert_eq!(ttc(&pool, note).await, 102_098, "toutes les lignes sont reprises");

    // Cliquer deux fois rouvre le même brouillon.
    let (_, lieu) = poster(&s, &format!("/factures/{facture}/crediter"), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{note}").as_str()));

    // Validée : numéro propre aux notes de crédit (après la 5e de Vertuoza), sans échéance ni communication structurée.
    let (statut, _) = poster(&s, &format!("/factures/{note}/valider"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (numero, vcs, echeance): (Option<i64>, Option<String>, Option<chrono::NaiveDate>) =
        sqlx::query_as("SELECT numero, vcs, date_echeance FROM factures WHERE id = $1").bind(note).fetch_one(&pool).await.unwrap();
    assert_eq!((numero, vcs, echeance), (Some(6), None, None));
    let compteur: i64 = sqlx::query_scalar("SELECT valeur FROM compteurs WHERE nom = 'facture'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, 125, "une note de crédit ne consomme pas de numéro de facture");

    let (_, page) = lire(&s, &format!("/factures/{note}")).await;
    for attendu in ["Note de crédit N° 6", "Note de crédit pour la facture n° 125"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("Créditer (note de crédit)"), "une note de crédit ne se crédite pas");
    let (_, page) = lire(&s, &format!("/factures/{facture}")).await;
    assert!(page.contains("Créditée par la note de crédit n° 6"));

    // Plus rien à créditer.
    assert_eq!(poster(&s, &format!("/factures/{facture}/crediter"), "").await.0, StatusCode::CONFLICT);

    // Dans la liste, la note de crédit compte en négatif : total TTC = 0.
    let (_, liste) = lire(&s, "/factures").await;
    assert!(liste.contains("NC 6") && liste.contains("-1\u{a0}020,98\u{a0}€"));
    assert!(liste.contains("Total TTC <strong>0,00\u{a0}€</strong>"));
}

#[sqlx::test]
async fn un_credit_partiel_ne_depasse_jamais_le_reste(pool: PgPool) {
    let s = site(&pool);
    let facture = facture_validee(&pool, &s).await;

    // Première note : on ne garde que les fournitures (2,5 × 19,99 € à 6 % = 52,98 € TTC).
    let (_, lieu) = poster(&s, &format!("/factures/{facture}/crediter"), "").await;
    let note = id_depuis(lieu);
    let main_oeuvre: i64 = sqlx::query_scalar("SELECT id FROM facture_lignes WHERE facture_id = $1 AND taux_centiemes = 2100")
        .bind(note)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (statut, _) = poster(&s, &format!("/factures/{note}/lignes/{main_oeuvre}/supprimer"), "").await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(ttc(&pool, note).await, 5_298);
    poster(&s, &format!("/factures/{note}/valider"), "").await;

    // Seconde note : reprend tout (1 020,98 €) mais il ne reste que 968,00 € à créditer.
    let (_, lieu) = poster(&s, &format!("/factures/{facture}/crediter"), "").await;
    let seconde = id_depuis(lieu);
    let (_, page) = lire(&s, &format!("/factures/{seconde}")).await;
    assert!(page.contains("Un total TTC d&#39;au plus 968,00") || page.contains("Un total TTC d'au plus 968,00"), "{page}");
    assert_eq!(poster(&s, &format!("/factures/{seconde}/valider"), "").await.0, StatusCode::CONFLICT);

    // En retirant les fournitures, il reste 968,00 € : la note se valide (n° 7).
    let fournitures: i64 = sqlx::query_scalar("SELECT id FROM facture_lignes WHERE facture_id = $1 AND taux_centiemes = 600")
        .bind(seconde)
        .fetch_one(&pool)
        .await
        .unwrap();
    poster(&s, &format!("/factures/{seconde}/lignes/{fournitures}/supprimer"), "").await;
    assert_eq!(ttc(&pool, seconde).await, 96_800);
    assert_eq!(poster(&s, &format!("/factures/{seconde}/valider"), "").await.0, StatusCode::SEE_OTHER);
    let numero: Option<i64> = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(seconde).fetch_one(&pool).await.unwrap();
    assert_eq!(numero, Some(7));
}

#[sqlx::test]
async fn le_pdf_d_une_facture_et_d_une_note_de_credit(pool: PgPool) {
    let s = site(&pool);
    let facture = facture_validee(&pool, &s).await;
    for (uri, nom) in [(format!("/factures/{facture}/pdf"), "inline; filename=\"Facture-125.pdf\""), (format!("/factures/{facture}/pdf?telecharger=1"), "attachment; filename=\"Facture-125.pdf\"")] {
        let (statut, _, octets, disposition) = envoyer(&s, Request::builder().uri(&uri).body(Body::empty()).unwrap()).await;
        assert_eq!(statut, StatusCode::OK);
        assert!(octets.starts_with(b"%PDF-") && octets.len() > 5_000);
        assert_eq!(disposition.as_deref(), Some(nom));
    }

    let (_, lieu) = poster(&s, &format!("/factures/{facture}/crediter"), "").await;
    let note = id_depuis(lieu);
    // Le brouillon a aussi son PDF (marqué « BROUILLON »).
    let (statut, _, octets, disposition) = envoyer(&s, Request::builder().uri(&format!("/factures/{note}/pdf")).body(Body::empty()).unwrap()).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
    assert_eq!(disposition, Some(format!("inline; filename=\"Facture-brouillon-{note}.pdf\"")));
    poster(&s, &format!("/factures/{note}/valider"), "").await;
    let (_, _, _, disposition) = envoyer(&s, Request::builder().uri(&format!("/factures/{note}/pdf")).body(Body::empty()).unwrap()).await;
    assert_eq!(disposition.as_deref(), Some("inline; filename=\"Note-de-credit-6.pdf\""));

    let (statut, _) = lire(&s, "/factures/999999/pdf").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

// Outil de vérification à l'œil, ignoré par `cargo test` (base temporaire, jamais la vraie base) :
//   $env:RENDU_DOSSIER = "C:\chemin\dossier"; cargo test --test notes_credit -- --ignored
#[sqlx::test]
#[ignore]
async fn ecrire_les_pages_d_une_facture_en_png(pool: PgPool) {
    let dossier = std::env::var("RENDU_DOSSIER").expect("Il faut RENDU_DOSSIER (dossier de sortie).");
    let s = site(&pool);
    let facture = facture_validee(&pool, &s).await;
    let (_, lieu) = poster(&s, &format!("/factures/{facture}/crediter"), "").await;
    let note = id_depuis(lieu);
    for (nom, id) in [("facture", facture), ("note", note)] {
        let pages = ubifacture::facture_pdf::pages_en_png(&pool, id).await.unwrap();
        for (i, png) in pages.iter().enumerate() {
            std::fs::write(format!("{dossier}/{nom}-{}.png", i + 1), png).unwrap();
        }
    }
}
