// Tests de l'étape 9-1 : page client en ligne (lien secret) : consulter, signer un devis, payer une facture par QR code.

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

async fn envoyer(site: &Router, requete: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    (statut, lieu, octets)
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, String::from_utf8_lossy(&corps).to_string())
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

async fn societe(pool: &PgPool) {
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('Ubique SRL', 'Ubique', 'info@ubique.be', '010 00 00 00')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, bic, par_defaut) VALUES ('BE68 5390 0754 7034', 'GKCCBEBB', true)")
        .execute(pool)
        .await
        .unwrap();
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

/// Le chemin de la page client affiché dans l'application (« /client/devis/… »).
fn chemin_client(page: &str, genre: &str) -> String {
    let debut = page.find(&format!("http://127.0.0.1:3001/client/{genre}/")).expect("lien de la page client");
    let reste = &page[debut + "http://127.0.0.1:3001".len()..];
    reste[..reste.find('"').unwrap()].to_string()
}

#[sqlx::test]
async fn le_client_consulte_et_signe_le_devis(pool: PgPool) {
    societe(&pool).await;
    let s = site(&pool);
    let (_, lieu) = poster(&s, "/devis", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    sqlx::query("UPDATE devis SET reference = 'PL46 Salle de bain', montant_ht_cents = 100000, montant_ttc_cents = 106000 WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let (_, apercu) = lire(&s, &format!("/devis/{id}/apercu")).await;
    assert!(apercu.contains("Page client en ligne") && apercu.contains("Visible dès que le devis est envoyé"));
    let chemin = chemin_client(&apercu, "devis");
    // Un brouillon n'est pas visible par le client.
    assert_eq!(lire(&s, &chemin).await.0, StatusCode::NOT_FOUND);
    assert_eq!(lire(&s, "/client/devis/inconnu").await.0, StatusCode::NOT_FOUND);

    poster(&s, &format!("/devis/{id}/statut/envoye"), "").await;
    let (code, page) = lire(&s, &chemin).await;
    assert_eq!(code, StatusCode::OK);
    for attendu in ["Ubique SRL", "PL46 Salle de bain", "1\u{a0}060,00\u{a0}€", "À signer", "Signer le devis", "Votre nom complet"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("aria-label=\"Menu principal\""), "pas de menu de l'application sur la page client");
    let (code, _, pdf) = envoyer(&s, Request::builder().uri(format!("{chemin}/pdf")).body(Body::empty()).unwrap()).await;
    assert!(code == StatusCode::OK && pdf.starts_with(b"%PDF-"));
    // La première consultation est notée, une seule fois.
    lire(&s, &chemin).await;
    let consultations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM devis_evenements WHERE devis_id = $1 AND detail = 'Consulté en ligne par le client'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(consultations, 1);

    // Signature : il faut un nom et l'accord.
    assert_eq!(poster(&s, &chemin, "nom=&accord=oui").await.1, Some(format!("{chemin}?erreur=nom#signer")));
    assert_eq!(poster(&s, &chemin, "nom=Anne+Claes").await.1, Some(format!("{chemin}?erreur=accord#signer")));
    assert_eq!(poster(&s, &chemin, "nom=Anne+Claes&accord=oui").await.1, Some(format!("{chemin}?ok=signe")));
    let (statut, par): (String, String) = sqlx::query_as("SELECT statut::text, signe_par FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((statut.as_str(), par.as_str()), ("signe", "Anne Claes"));
    let (_, page) = lire(&s, &format!("{chemin}?ok=signe")).await;
    assert!(page.contains("Merci, le devis est signé.") && page.contains("par Anne Claes") && !page.contains("name=\"accord\""));
    // Une seconde signature ne change rien.
    poster(&s, &chemin, "nom=Quelqu%27un&accord=oui").await;
    let par: String = sqlx::query_scalar("SELECT signe_par FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(par, "Anne Claes");

    // Dans l'application : « Signé en ligne … : à accepter », puis l'acceptation crée le chantier.
    let (_, page) = lire(&s, &format!("/devis/{id}")).await;
    assert!(page.contains("Signé en ligne le") && page.contains("par Anne Claes : à accepter"));
    poster(&s, &format!("/devis/{id}/statut/accepter"), &format!("date={}", aujourdhui(&pool).await)).await;
    let statut: String = sqlx::query_scalar("SELECT statut::text FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(statut, "accepte");
    let (_, page) = lire(&s, &chemin).await;
    assert!(page.contains("Ce devis est accepté."));
}

#[sqlx::test]
async fn le_client_paie_la_facture_par_qr_code(pool: PgPool) {
    societe(&pool).await;
    let s = site(&pool);
    let (_, lieu) = poster(&s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    assert!(!page.contains("/client/facture/"), "pas de page client pour un brouillon");

    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils) VALUES ('Client SA', '{client}') RETURNING id").fetch_one(&pool).await.unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30").fetch_one(&pool).await.unwrap();
    let jour = aujourdhui(&pool).await;
    poster(&s, &format!("/factures/{id}"), &format!("date={jour}&reference=HP1&client=e{client}&conditions_paiement_id={condition}")).await;
    poster(&s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Travaux&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;

    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    let chemin = chemin_client(&page, "facture");
    let (code, page) = lire(&s, &chemin).await;
    assert_eq!(code, StatusCode::OK);
    let vcs = vcs_125(&pool).await;
    for attendu in ["Facture N° 125", "À payer", "Reste à payer", "121,00\u{a0}€", "<div class=\"c-qr\"><svg", "BE68 5390 0754 7034", vcs.as_str(), "Scannez ce QR code"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    let (code, _, pdf) = envoyer(&s, Request::builder().uri(format!("{chemin}/pdf")).body(Body::empty()).unwrap()).await;
    assert!(code == StatusCode::OK && pdf.starts_with(b"%PDF-"));
    let genre: String = sqlx::query_scalar("SELECT genre FROM facture_evenements WHERE facture_id = $1 AND genre = 'client'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(genre, "client");

    // Payée en partie : le QR code demande le reste ; payée : plus de QR code.
    let methode: i64 = sqlx::query_scalar("SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").fetch_one(&pool).await.unwrap();
    poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=partiel&date={jour}&montant=21&methode_id={methode}")).await;
    let (_, page) = lire(&s, &chemin).await;
    assert!(page.contains("Déjà payé") && page.contains("21,00\u{a0}€") && page.contains("100,00\u{a0}€") && page.contains("<div class=\"c-qr\">"));
    poster(&s, &format!("/factures/{id}/paiements"), &format!("genre=complet&date={jour}&methode_id={methode}")).await;
    let (_, page) = lire(&s, &chemin).await;
    assert!(page.contains("Cette facture est payée.") && !page.contains("<div class=\"c-qr\">"));

    // Un jeton inconnu : 404.
    assert_eq!(lire(&s, "/client/facture/abcdefabcdefabcdefabcdef").await.0, StatusCode::NOT_FOUND);
}

/// La communication structurée de la facture n° 125 de l'année en cours (numérotation annuelle).
async fn vcs_125(pool: &PgPool) -> String {
    let annee: i32 = sqlx::query_scalar("SELECT extract(year FROM now() AT TIME ZONE 'Europe/Brussels')::int").fetch_one(pool).await.unwrap();
    ubifacture::facture_calcul::vcs_annuelle(annee, 125)
}
