// Tests de l'étape 8-2 : envoi des devis et des factures par e-mail, modèles modifiables.
// Aucune clé Brevo n'est configurée pendant les tests : chaque e-mail est seulement enregistré, rien ne part.

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

fn id_depuis(lieu: Option<String>, prefixe: &str) -> i64 {
    lieu.unwrap().strip_prefix(prefixe).unwrap().split(['?', '/']).next().unwrap().parse().unwrap()
}

async fn societe(pool: &PgPool) {
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('Ubique SRL', 'Ubique', 'info@ubique.be', '010')")
        .execute(pool)
        .await
        .unwrap();
}

/// Un devis en brouillon pour Mme Claes (anne@exemple.be).
async fn devis(pool: &PgPool, s: &Router) -> i64 {
    let (_, lieu, _) = poster(s, "/devis", "").await;
    let id = id_depuis(lieu, "/devis/");
    let contact: i64 = sqlx::query_scalar(
        "INSERT INTO contacts (civilite, nom, prenom, profils, email) VALUES ('Mme.', 'Claes', 'Anne', '{client}', 'anne@exemple.be') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE devis SET client_contact_id = $1, reference = 'PL46 Salle de bain', responsable = 'Max' WHERE id = $2")
        .bind(contact)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn nombre_emails(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM emails").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn envoyer_un_devis_sans_cle_l_enregistre_seulement(pool: PgPool) {
    societe(&pool).await;
    let s = site(&pool);
    let id = devis(&pool, &s).await;
    let numero: String = sqlx::query_scalar("SELECT extract(year FROM date_devis)::int || '-' || numero FROM devis WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let (code, page) = lire(&s, &format!("/devis/{id}/email")).await;
    assert_eq!(code, StatusCode::OK);
    for attendu in [
        "anne@exemple.be",
        &format!("Votre devis {numero} – PL46 Salle de bain"),
        "nous avons le plaisir de vous transmettre le devis n°",
        "Bonjour Madame Anne Claes,",
        "/client/devis/",
        "Mode test",
        "Max\ninfo@ubique.be",
    ] {
        assert!(page.contains(attendu), "{attendu}");
    }
    let jeton: Option<String> = sqlx::query_scalar("SELECT jeton FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(jeton.as_ref().map(|j| j.len()), Some(24), "le lien secret est créé");

    // Une adresse fausse : rien n'est enregistré, ce qui a été tapé reste.
    let (code, _, page) = poster(&s, &format!("/devis/{id}/email"), "destinataires=pas-une-adresse&objet=Mon+devis&corps=Texte+gard%C3%A9").await;
    assert_eq!(code, StatusCode::OK);
    assert!(page.contains("adresse du destinataire") && page.contains("Texte gardé"));
    assert_eq!(nombre_emails(&pool).await, 0);

    let (code, lieu, _) =
        poster(&s, &format!("/devis/{id}/email"), "destinataires=anne%40exemple.be&copie=max%40ubique.be&objet=Votre+devis&corps=Bonjour").await;
    assert_eq!((code, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/devis/{id}?ok=email_enregistre").as_str())));
    let (statut, destinataires, copie, pieces): (String, String, String, Vec<String>) =
        sqlx::query_as("SELECT statut, destinataires, copie, pieces FROM emails WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((statut.as_str(), destinataires.as_str(), copie.as_str()), ("enregistre", "anne@exemple.be", "max@ubique.be"));
    assert_eq!(pieces, vec![format!("Devis-{numero}.pdf")]);
    // Rien n'est parti : le devis reste en brouillon ; l'historique le dit.
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(statut, "brouillon");
    let detail: String = sqlx::query_scalar("SELECT detail FROM devis_evenements WHERE devis_id = $1 AND genre = 'email'").bind(id).fetch_one(&pool).await.unwrap();
    assert!(detail.contains("mode test"));
    let (_, page) = lire(&s, &format!("/devis/{id}?ok=email_enregistre")).await;
    assert!(page.contains("Mode test : l&#39;e-mail est enregistré") || page.contains("Mode test : l'e-mail est enregistré"));
    // Le jeton ne change pas au second passage.
    lire(&s, &format!("/devis/{id}/email")).await;
    let encore: Option<String> = sqlx::query_scalar("SELECT jeton FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(encore, jeton);
}

#[sqlx::test]
async fn envoyer_une_facture(pool: PgPool) {
    societe(&pool).await;
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ('BE68 5390 0754 7034', true)").execute(&pool).await.unwrap();
    let s = site(&pool);
    let (_, lieu, _) = poster(&s, "/factures", "").await;
    let id = id_depuis(lieu, "/factures/");

    // Un brouillon ne s'envoie pas.
    assert_eq!(lire(&s, &format!("/factures/{id}/email")).await.0, StatusCode::CONFLICT);

    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils, email) VALUES ('Hôtel du Parc SA', '{client}', 'compta@hotel.be') RETURNING id")
        .fetch_one(&pool)
        .await
        .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30").fetch_one(&pool).await.unwrap();
    let jour: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{id}"), &format!("date={jour}&reference=HP1&client=e{client}&conditions_paiement_id={condition}")).await;
    poster(&s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;

    let (code, _) = lire(&s, &format!("/factures/{id}/email")).await;
    assert_eq!(code, StatusCode::OK);
    // Le modèle « Facture » d'Ubique (repris de Vertuoza) ; la communication structurée suit la numérotation annuelle.
    let vcs = vcs_125(&pool).await;
    sqlx::query("UPDATE modeles_email SET corps = corps || E'\\n[COMMUNICATION] [IBAN]' WHERE code = 'facture'").execute(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/factures/{id}/email")).await;
    let annee: i32 = sqlx::query_scalar("SELECT extract(year FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(&pool).await.unwrap();
    let objet = format!("Facture {annee}-125 – chantier");
    let phrase = format!("la facture n° {annee}-125 du");
    for attendu in ["compta@hotel.be", objet.as_str(), phrase.as_str(), "(« HP1 »)", vcs.as_str(), "BE68 5390 0754 7034", "/client/facture/", "Bonjour Hôtel du Parc SA,"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    let (code, lieu, _) = poster(&s, &format!("/factures/{id}/email"), "destinataires=compta%40hotel.be&objet=Facture+125&corps=Bonjour").await;
    assert_eq!((code, lieu.as_deref()), (StatusCode::SEE_OTHER, Some(format!("/factures/{id}?ok=email_enregistre").as_str())));
    let (statut, pieces): (String, Vec<String>) =
        sqlx::query_as("SELECT statut, pieces FROM emails WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((statut.as_str(), pieces), ("enregistre", vec!["Facture-125.pdf".to_string()]));
    // Rien n'est parti : la facture reste « À envoyer » ; l'historique de la facture le note.
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(statut, "a_envoyer");
    let genres: Vec<String> =
        sqlx::query_scalar("SELECT genre FROM facture_evenements WHERE facture_id = $1 ORDER BY id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(genres, vec!["validation", "email"]);
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(page.contains(&format!("href=\"/factures/{id}/email\"")) && !page.contains("Envoyer par e-mail <small>"));
}

#[sqlx::test]
async fn les_modeles_se_modifient(pool: PgPool) {
    societe(&pool).await;
    let s = site(&pool);
    let (code, page) = lire(&s, "/parametres/emails").await;
    assert_eq!(code, StatusCode::OK);
    for attendu in ["Mode test", "Oubli de paiement ?", "Rappel de paiement (version ferme)", "[NUMBER_QUOTE]", "Aucun e-mail pour l&#39;instant.", "href=\"/parametres/emails\""] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("&#39;", "'")), "{attendu}");
    }
    let (_, lieu, _) = poster(&s, "/parametres/emails/devis", "objet=Notre+offre+[NUMBER_QUOTE]&corps=Cher+[NOM_CLIENT]").await;
    assert_eq!(lieu.as_deref(), Some("/parametres/emails?ok=modele&modele=devis#modele-devis"));
    let id = devis(&pool, &s).await;
    let (_, page) = lire(&s, &format!("/devis/{id}/email")).await;
    assert!(page.contains("Cher Anne Claes") && page.contains("Notre offre "));

    // Objet vide : refusé, le modèle ne change pas.
    let (_, lieu, _) = poster(&s, "/parametres/emails/devis", "objet=&corps=x").await;
    assert_eq!(lieu.as_deref(), Some("/parametres/emails?erreur=modele&modele=devis#modele-devis"));
    let objet: String = sqlx::query_scalar("SELECT objet FROM modeles_email WHERE code = 'devis'").fetch_one(&pool).await.unwrap();
    assert_eq!(objet, "Notre offre [NUMBER_QUOTE]");
    assert_eq!(poster(&s, "/parametres/emails/inconnu", "objet=x").await.0, StatusCode::NOT_FOUND);

    // Le journal montre l'e-mail enregistré.
    poster(&s, &format!("/devis/{id}/email"), "destinataires=anne%40exemple.be&objet=Votre+devis&corps=Bonjour").await;
    let (_, page) = lire(&s, "/parametres/emails").await;
    assert!(page.contains("Votre devis") && page.contains("Enregistré (non envoyé)"));
}

/// La communication structurée de la facture n° 125 de l'année en cours (numérotation annuelle).
async fn vcs_125(pool: &PgPool) -> String {
    let annee: i32 = sqlx::query_scalar("SELECT extract(year FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(pool).await.unwrap();
    ubifacture::facture_calcul::vcs_annuelle(annee, 125)
}
