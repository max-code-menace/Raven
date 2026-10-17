// Tests de l'étape 7-1 : factures simples, lignes, validation, numérotation continue et communication structurée.

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

async fn poster(site: &Router, uri: &str, corps: &str) -> (StatusCode, Option<String>, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    envoyer(site, requete).await
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

/// Crée une facture simple et renvoie son id.
async fn nouvelle(s: &Router) -> i64 {
    let (statut, lieu, _) = poster(s, "/factures", "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap()
}

async fn client(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO entreprises (nom, profils) VALUES ('Client SA', '{client}') RETURNING id").fetch_one(pool).await.unwrap()
}

async fn condition_30_jours(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30").fetch_one(pool).await.unwrap()
}

async fn taux(pool: &PgPool, centiemes: i32) -> i64 {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(centiemes).fetch_one(pool).await.unwrap()
}

/// Remplit l'en-tête : client, conditions de paiement, date du jour.
async fn completer(pool: &PgPool, s: &Router, id: i64) {
    let corps = format!(
        "date={}&reference=Petits+travaux&responsable=Max&client=e{}&conditions_paiement_id={}&taux_tva_id=&remarque=",
        aujourdhui(pool).await,
        client(pool).await,
        condition_30_jours(pool).await
    );
    let (statut, lieu, _) = poster(s, &format!("/factures/{id}"), &corps).await;
    assert_eq!((statut, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/factures/{id}?ok=infos").as_str())));
}

/// Ajoute un poste et le remplit ; renvoie l'id de la ligne.
async fn poste(pool: &PgPool, s: &Router, id: i64, description: &str, quantite: &str, prix: &str, taux_centiemes: i32) -> i64 {
    let (statut, _, _) = poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    assert_eq!(statut, StatusCode::OK);
    let ligne: i64 =
        sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
    let corps = format!(
        "description={}&quantite={quantite}&unite=h&prix_unitaire={prix}&taux_tva_id={}",
        description.replace(' ', "+"),
        taux(pool, taux_centiemes).await
    );
    let (statut, _, reponse) = poster(s, &format!("/factures/{id}/lignes/{ligne}"), &corps).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(!reponse.contains("alert-warning"), "{reponse}");
    ligne
}

async fn montants(pool: &PgPool, id: i64) -> (i64, i64, i64) {
    sqlx::query_as("SELECT montant_ht_cents, montant_tva_cents, montant_ttc_cents FROM factures WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn une_facture_simple_de_bout_en_bout(pool: PgPool) {
    let s = site(&pool);
    let id = nouvelle(&s).await;
    let (numero, statut, genre): (Option<i64>, String, String) =
        sqlx::query_as("SELECT numero, statut::text, genre::text FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((numero, statut.as_str(), genre.as_str()), (None, "brouillon", "simple"), "un brouillon n'a pas de numéro");

    // Tant que rien n'est rempli, la fiche dit ce qui manque et ne propose pas de valider.
    let (code, page) = lire(&s, &format!("/factures/{id}?ok=creee")).await;
    assert_eq!(code, StatusCode::OK);
    for attendu in ["Facture simple (brouillon)", "À compléter avant de valider", "Le client", "Au moins une ligne chiffrée"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("Valider et numéroter"));

    completer(&pool, &s, id).await;
    // 16 h à 50 € (21 %) + 2,5 × 19,99 € (6 %) = 800,00 + 49,98 = 849,98 € HT ; TVA 168,00 + 3,00 = 171,00 €.
    poste(&pool, &s, id, "Main d'oeuvre", "16", "50", 2100).await;
    poste(&pool, &s, id, "Fournitures", "2,5", "19,99", 600).await;
    assert_eq!(montants(&pool, id).await, (84_998, 17_100, 102_098));

    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Valider et numéroter"));
    assert!(page.contains("1\u{a0}020,98\u{a0}€"));

    // Valider : premier numéro après la dernière facture de Vertuoza (124), échéance à 30 jours, VCS.
    let (statut, lieu, _) = poster(&s, &format!("/factures/{id}/valider"), "").await;
    assert_eq!((statut, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/factures/{id}?ok=validee").as_str())));
    let (numero, statut, vcs, echeance_ok): (Option<i64>, String, Option<String>, bool) = sqlx::query_as(
        "SELECT numero, statut::text, vcs, date_echeance = date_facture + 30 FROM factures WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((numero, statut.as_str(), vcs.as_deref(), echeance_ok), (Some(125), "a_envoyer", Some(vcs_125(&pool).await.as_str()), true));

    let (_, page) = lire(&s, &format!("/factures/{id}?ok=validee")).await;
    let vcs = vcs_125(&pool).await;
    for attendu in ["Facture N° 125", vcs.as_str(), "Marquer comme envoyée", "Client SA"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("name=\"quantite\""), "plus de champ de saisie");

    // Plus rien ne se modifie : ni l'en-tête, ni les lignes, ni la suppression.
    let ligne: i64 = sqlx::query_scalar("SELECT min(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/factures/{id}"), "date=2026-01-01").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/factures/{id}/lignes"), "genre=poste").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/factures/{id}/lignes/{ligne}"), "description=x").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/factures/{id}/supprimer"), "").await.0, StatusCode::CONFLICT);
    assert_eq!(poster(&s, &format!("/factures/{id}/valider"), "").await.0, StatusCode::CONFLICT);

    // Envoyée (à la main en attendant les e-mails).
    let (statut, _, _) = poster(&s, &format!("/factures/{id}/envoyee"), "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(statut, "envoyee");
    assert_eq!(poster(&s, &format!("/factures/{id}/envoyee"), "").await.0, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn la_base_refuse_de_supprimer_ou_de_modifier_une_facture_numerotee(pool: PgPool) {
    let s = site(&pool);
    let id = nouvelle(&s).await;
    completer(&pool, &s, id).await;
    poste(&pool, &s, id, "Travaux", "1", "100", 2100).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;

    assert!(sqlx::query("DELETE FROM factures WHERE id = $1").bind(id).execute(&pool).await.is_err());
    assert!(sqlx::query("UPDATE facture_lignes SET total_cents = 1 WHERE facture_id = $1").bind(id).execute(&pool).await.is_err());
    assert!(sqlx::query("DELETE FROM facture_lignes WHERE facture_id = $1").bind(id).execute(&pool).await.is_err());
    assert!(
        sqlx::query("INSERT INTO facture_lignes (facture_id, position, genre) VALUES ($1, 99, 'texte')").bind(id).execute(&pool).await.is_err()
    );
}

#[sqlx::test]
async fn la_numerotation_est_continue_et_un_brouillon_supprime_ne_laisse_pas_de_trou(pool: PgPool) {
    let s = site(&pool);
    let mut numeros = Vec::new();
    for i in 0..3 {
        let id = nouvelle(&s).await;
        completer(&pool, &s, id).await;
        poste(&pool, &s, id, "Travaux", "1", "100", 2100).await;
        if i == 1 {
            // Ce brouillon est supprimé : il n'avait pas de numéro.
            let (statut, lieu, _) = poster(&s, &format!("/factures/{id}/supprimer"), "").await;
            assert_eq!((statut, lieu.as_deref()), (StatusCode::SEE_OTHER, Some("/factures")));
            continue;
        }
        poster(&s, &format!("/factures/{id}/valider"), "").await;
        numeros.push(sqlx::query_scalar::<_, Option<i64>>("SELECT numero FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap());
    }
    assert_eq!(numeros, vec![Some(125), Some(126)]);
    let compteur: i64 = sqlx::query_scalar("SELECT valeur FROM compteurs WHERE nom = 'facture'").fetch_one(&pool).await.unwrap();
    assert_eq!(compteur, 126);
}

#[sqlx::test]
async fn une_facture_ne_peut_pas_etre_datee_avant_la_precedente(pool: PgPool) {
    let s = site(&pool);
    let premiere = nouvelle(&s).await;
    completer(&pool, &s, premiere).await;
    poste(&pool, &s, premiere, "Travaux", "1", "100", 2100).await;
    poster(&s, &format!("/factures/{premiere}/valider"), "").await;

    let seconde = nouvelle(&s).await;
    completer(&pool, &s, seconde).await;
    poste(&pool, &s, seconde, "Travaux", "1", "100", 2100).await;
    sqlx::query("UPDATE factures SET date_facture = date_facture - 1 WHERE id = $1").bind(seconde).execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/factures/{seconde}")).await;
    assert!(page.contains("Une date au plus tôt le"), "la fiche explique pourquoi");
    assert_eq!(poster(&s, &format!("/factures/{seconde}/valider"), "").await.0, StatusCode::CONFLICT);
    let numero: Option<i64> = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(seconde).fetch_one(&pool).await.unwrap();
    assert_eq!(numero, None, "aucun numéro consommé");

    // Une date dans le futur est refusée aussi.
    sqlx::query("UPDATE factures SET date_facture = current_date + 10 WHERE id = $1").bind(seconde).execute(&pool).await.unwrap();
    assert_eq!(poster(&s, &format!("/factures/{seconde}/valider"), "").await.0, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn une_facture_a_zero_ne_se_valide_pas(pool: PgPool) {
    let s = site(&pool);
    let id = nouvelle(&s).await;
    completer(&pool, &s, id).await;
    poste(&pool, &s, id, "Rien", "1", "0", 2100).await;
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains("Un total TTC positif"));
    assert_eq!(poster(&s, &format!("/factures/{id}/valider"), "").await.0, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn saisies_refusees_et_lignes_supprimees(pool: PgPool) {
    let s = site(&pool);
    let id = nouvelle(&s).await;
    let ligne = poste(&pool, &s, id, "Travaux", "2", "10", 2100).await;
    assert_eq!(montants(&pool, id).await.0, 2_000);

    // Un nombre illisible : message, rien n'est changé.
    let corps = format!("description=Travaux&quantite=deux&unite=h&prix_unitaire=10&taux_tva_id={}", taux(&pool, 2100).await);
    let (statut, _, reponse) = poster(&s, &format!("/factures/{id}/lignes/{ligne}"), &corps).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(reponse.contains("n&#39;est pas un nombre") || reponse.contains("n'est pas un nombre"), "{reponse}");
    assert_eq!(montants(&pool, id).await.0, 2_000);

    // Un taux inconnu est refusé.
    let (_, _, reponse) = poster(&s, &format!("/factures/{id}/lignes/{ligne}"), "description=x&quantite=1&prix_unitaire=1&taux_tva_id=999999").await;
    assert!(reponse.contains("Choisis un taux de TVA"));

    // Un titre et un texte ne comptent pas dans les totaux ; supprimer une ligne recalcule.
    poster(&s, &format!("/factures/{id}/lignes"), "genre=titre").await;
    poster(&s, &format!("/factures/{id}/lignes"), "genre=texte").await;
    assert_eq!(montants(&pool, id).await.0, 2_000);
    let (statut, _, page) = poster(&s, &format!("/factures/{id}/lignes/{ligne}/supprimer"), "").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("id=\"lignes\"") && !page.contains("<html"));
    assert_eq!(montants(&pool, id).await, (0, 0, 0));

    // Un client inconnu dans l'en-tête est refusé.
    let (_, lieu, _) = poster(&s, &format!("/factures/{id}"), "date=2026-10-16&client=e999999").await;
    assert_eq!(lieu.as_deref(), Some(format!("/factures/{id}?erreur=infos").as_str()));
}

#[sqlx::test]
async fn la_liste_filtre_et_totalise(pool: PgPool) {
    let s = site(&pool);
    let id = nouvelle(&s).await;
    completer(&pool, &s, id).await;
    poste(&pool, &s, id, "Travaux", "1", "100", 2100).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;
    nouvelle(&s).await;

    let (code, page) = lire(&s, "/factures").await;
    assert_eq!(code, StatusCode::OK);
    for attendu in ["Factures et notes de crédit", "125", "Client SA", "Petits travaux", "121,00\u{a0}€", "2 pièce(s)", "Brouillon"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    let (_, _, tableau) =
        envoyer(&s, Request::builder().uri("/factures?statut=brouillon").header("hx-request", "true").body(Body::empty()).unwrap()).await;
    assert!(!tableau.contains("<html") && tableau.contains("1 pièce(s)") && !tableau.contains("Client SA"));
    let (_, page) = lire(&s, "/factures?q=Client").await;
    assert!(page.contains("1 pièce(s)"));
}

/// La communication structurée de la facture n° 125 de l'année en cours (numérotation annuelle).
async fn vcs_125(pool: &PgPool) -> String {
    let annee: i32 = sqlx::query_scalar("SELECT extract(year FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(pool).await.unwrap();
    ubifacture::facture_calcul::vcs_annuelle(annee, 125)
}
