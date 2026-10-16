// Tests de l'étape 8-1 : paiements partiels et complets, méthodes, statuts « Partiellement payée », « Payée »,
// « En retard », « Créditée », reste à recevoir et historique de la facture.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
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

async fn virement(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").fetch_one(pool).await.unwrap()
}

/// Une facture simple validée de 1 020,98 € TTC (849,98 € HT), renvoie son id.
async fn facture_validee(pool: &PgPool, s: &Router) -> i64 {
    let (_, lieu) = poster(s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let client: i64 =
        sqlx::query_scalar("INSERT INTO entreprises (nom, profils) VALUES ('Client SA', '{client}') RETURNING id").fetch_one(pool).await.unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30")
        .fetch_one(pool)
        .await
        .unwrap();
    let corps = format!("date={}&reference=Travaux&responsable=Max&client=e{client}&conditions_paiement_id={condition}", aujourdhui(pool).await);
    poster(s, &format!("/factures/{id}"), &corps).await;
    for (description, quantite, prix, taux) in [("Main+d%27oeuvre", "16", "50", 2100), ("Fournitures", "2,5", "19,99", 600)] {
        poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
        let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
        let taux_id: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(taux).fetch_one(pool).await.unwrap();
        poster(
            s,
            &format!("/factures/{id}/lignes/{ligne}"),
            &format!("description={description}&quantite={quantite}&unite=h&prix_unitaire={prix}&taux_tva_id={taux_id}"),
        )
        .await;
    }
    let (statut, lieu) = poster(s, &format!("/factures/{id}/valider"), "").await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/factures/{id}?ok=validee"))));
    id
}

async fn etat(pool: &PgPool, id: i64) -> (String, i64, i64) {
    sqlx::query_as("SELECT etat, paye_cents, reste_cents FROM factures_etats WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn paiement_partiel_puis_complet(pool: PgPool) {
    let s = site(&pool);
    let id = facture_validee(&pool, &s).await;
    let m = virement(&pool).await;
    let jour = aujourdhui(&pool).await;
    assert_eq!(etat(&pool, id).await, ("a_envoyer".to_string(), 0, 102_098));

    let page = lire(&s, &format!("/factures/{id}")).await;
    for attendu in ["Ajouter le paiement", "Reste à recevoir", &format!("1\u{a0}020,98{E}"), "Aucun paiement enregistré.", "Virement", "Bancontact"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("Ajouter le paiement <small>"), "plus de mention « bientôt » pour le paiement");

    // Paiement partiel de 500 € par virement.
    let (statut, lieu) =
        poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=partiel&date={jour}&montant=500&methode_id={m}&remarque=Premier+versement")).await;
    assert_eq!((statut, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/factures/{id}?ok=paiement").as_str())));
    assert_eq!(etat(&pool, id).await, ("partielle".to_string(), 50_000, 52_098));
    let page = lire(&s, &format!("/factures/{id}?ok=paiement")).await;
    for attendu in ["Partiellement payée", &format!("reste 520,98{E}"), &format!("500,00{E}"), "Premier versement", "Paiement partiel de 500,00"] {
        assert!(page.contains(attendu), "{attendu}");
    }

    // Plus que le reste : refusé, rien n'est enregistré.
    let (_, lieu) = poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=partiel&date={jour}&montant=520,99&methode_id={m}")).await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{id}?erreur=paiement_montant&payer=1#paiement").as_str()));
    assert_eq!(etat(&pool, id).await.1, 50_000);
    let page = lire(&s, &format!("/factures/{id}?erreur=paiement_montant&payer=1")).await;
    assert!(page.contains("ne pas dépasser le reste à recevoir"));

    // Paiement complet : le reste, quel que soit le montant tapé.
    let (_, lieu) = poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=complet&date={jour}&montant=1&methode_id={m}")).await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{id}?ok=payee").as_str()));
    assert_eq!(etat(&pool, id).await, ("payee".to_string(), 102_098, 0));
    let page = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Payée") && !page.contains("Enregistrer le paiement"));

    // Rien de plus à encaisser.
    let (statut, _) = poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=complet&date={jour}&methode_id={m}")).await;
    assert_eq!(statut, StatusCode::CONFLICT);

    // Retirer le second paiement : la facture redevient partiellement payée, c'est noté dans l'historique.
    let dernier: i64 = sqlx::query_scalar("SELECT max(id) FROM paiements WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    let (_, lieu) = poster(&s, &format!("/factures/{id}/paiements/{dernier}/supprimer"), "").await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{id}?ok=paiement_retire").as_str()));
    assert_eq!(etat(&pool, id).await.0, "partielle");
    let genres: Vec<String> =
        sqlx::query_scalar("SELECT genre FROM facture_evenements WHERE facture_id = $1 ORDER BY id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(genres, vec!["validation", "paiement", "paiement", "paiement_supprime"]);
    let page = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Historique") && page.contains("Paiement retiré"));
}

#[sqlx::test]
async fn paiements_refuses(pool: PgPool) {
    let s = site(&pool);
    let id = facture_validee(&pool, &s).await;
    let m = virement(&pool).await;
    let jour = aujourdhui(&pool).await;
    let demain: String = sqlx::query_scalar("SELECT (((now() AT TIME ZONE 'Europe/Brussels')::date) + 1)::text").fetch_one(&pool).await.unwrap();

    for (corps, code) in [
        (format!("genre=complet&date={demain}&methode_id={m}"), "paiement_date"),
        (format!("genre=partiel&date={jour}&montant=0&methode_id={m}"), "paiement_montant"),
        (format!("genre=partiel&date={jour}&montant=abc&methode_id={m}"), "paiement_montant"),
        (format!("genre=complet&date={jour}&methode_id=999"), "paiement_methode"),
    ] {
        let (_, lieu) = poster(&s, &format!("/factures/{id}/paiements"), &corps).await;
        assert_eq!(lieu, Some(format!("/factures/{id}?erreur={code}&payer=1#paiement")), "{corps}");
    }
    assert_eq!(etat(&pool, id).await.1, 0);

    // Un brouillon ne se paie pas.
    let (_, lieu) = poster(&s, "/factures", "").await;
    let brouillon: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let (statut, _) = poster(&s, &format!("/factures/{brouillon}/paiements"), &format!("genre=complet&date={jour}&methode_id={m}")).await;
    assert_eq!(statut, StatusCode::CONFLICT);
    // La base refuse aussi un paiement sur un brouillon, même sans passer par l'écran.
    let direct = sqlx::query("INSERT INTO paiements (facture_id, date_paiement, montant_cents, methode_id) VALUES ($1, current_date, 100, $2)")
        .bind(brouillon)
        .bind(m)
        .execute(&pool)
        .await;
    assert!(direct.is_err());

    // Une facture inconnue : 404.
    let (statut, _) = poster(&s, "/factures/999999/paiements", &format!("genre=complet&date={jour}&methode_id={m}")).await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn une_facture_echue_est_en_retard(pool: PgPool) {
    let s = site(&pool);
    let id = facture_validee(&pool, &s).await;
    sqlx::query("UPDATE factures SET date_echeance = (now() AT TIME ZONE 'Europe/Brussels')::date - 24 WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(etat(&pool, id).await.0, "en_retard");
    let page = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("En retard de 24 jours.") && page.contains("· 24 jours"));

    // Un paiement partiel ne la sort pas du retard ; le paiement complet, oui.
    let m = virement(&pool).await;
    let jour = aujourdhui(&pool).await;
    poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=partiel&date={jour}&montant=20&methode_id={m}")).await;
    assert_eq!(etat(&pool, id).await.0, "en_retard");
    poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=complet&date={jour}&methode_id={m}")).await;
    assert_eq!(etat(&pool, id).await.0, "payee");
}

#[sqlx::test]
async fn une_facture_entierement_creditee_ne_doit_plus_rien(pool: PgPool) {
    let s = site(&pool);
    let id = facture_validee(&pool, &s).await;
    let (_, lieu) = poster(&s, &format!("/factures/{id}/crediter"), "").await;
    let note: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/factures/{note}/valider"), "").await;
    assert_eq!(etat(&pool, id).await, ("creditee".to_string(), 0, 0));
    let page = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Créditée") && page.contains("Crédité (notes de crédit)") && !page.contains("Enregistrer le paiement"));
    // Pas de paiement sur une note de crédit.
    let m = virement(&pool).await;
    let jour = aujourdhui(&pool).await;
    let (statut, _) = poster(&s, &format!("/factures/{note}/paiements"), &format!("genre=complet&date={jour}&methode_id={m}")).await;
    assert_eq!(statut, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn la_liste_montre_l_etat_et_le_montant_a_recevoir(pool: PgPool) {
    let s = site(&pool);
    let id = facture_validee(&pool, &s).await;
    let m = virement(&pool).await;
    let jour = aujourdhui(&pool).await;
    poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=partiel&date={jour}&montant=1000&methode_id={m}")).await;

    let page = lire(&s, "/factures").await;
    for attendu in ["Partiellement payée", &format!("reste 20,98{E}"), &format!("Montant à recevoir TTC <strong style=\"color:#0B6371\">20,98{E}</strong>"), "?payer=1#paiement"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(lire(&s, "/factures?statut=partielle").await.contains("1 pièce(s)"));
    assert!(lire(&s, "/factures?statut=payee").await.contains("0 pièce(s)"));
    assert!(lire(&s, "/factures?statut=envoyee").await.contains("0 pièce(s)"));
}
