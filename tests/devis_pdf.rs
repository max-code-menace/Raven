// Tests du PDF du devis (Typst) et de l'onglet Visualisation.

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

async fn reponse(site: &Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let r = site.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let (statut, entetes) = (r.status(), r.headers().clone());
    (statut, entetes, r.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn page(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps) = reponse(site, uri).await;
    (s, String::from_utf8(corps).unwrap())
}

async fn typst(pool: &PgPool, devis: i64) -> String {
    match ubifacture::devis_pdf::charger_donnees(&mut *pool.acquire().await.unwrap(), devis).await {
        Ok(donnees) => ubifacture::devis_pdf::source_typst(&donnees).0,
        Err(_) => panic!("devis introuvable"),
    }
}

async fn taux(pool: &PgPool, centiemes: i32) -> i64 {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1").bind(centiemes).fetch_one(pool).await.unwrap()
}

async fn societe(pool: &PgPool) {
    sqlx::query("INSERT INTO societe (nom, nom_abrege, rue, numero, code_postal, ville, email, telephone, numero_tva) VALUES ('Ubique SRL', 'Ubique', 'Rue du Bâtiment', '5', '4000', 'Liège', 'info@ubique.be', '04 123 45 67', 'BE0123456789')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ('BE68 5390 0754 7034', true)").execute(pool).await.unwrap();
}

/// Un devis complet : client, TVA, conditions, un lot et deux lignes.
async fn devis_complet(pool: &PgPool) -> i64 {
    let reduit = taux(pool, 600).await;
    let conditions: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7").fetch_one(pool).await.unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, rue, numero, code_postal, ville, numero_tva) VALUES ('Dupont SRL', 'Rue Neuve', '12', '1000', 'Bruxelles', 'BE0987654321') RETURNING id").fetch_one(pool).await.unwrap();
    let d: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, date_devis, reference, responsable, client_entreprise_id, taux_tva_id, conditions_paiement_id, remarque_1, remarque_2)
         VALUES (57, '2026-10-15', 'C44 Volet', 'Marc', $1, $2, $3, 'Acompte de 30 %', 'Chantier accessible') RETURNING id",
    )
    .bind(client).bind(reduit).bind(conditions)
    .fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description) VALUES ($1, 1, 'titre', 'Fourniture des matériaux')").bind(d).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id) VALUES ($1, 2, 'poste_libre', 'Volet roulant', 250000, 'pce', 1999000, $2)")
        .bind(d).bind(reduit).execute(pool).await.unwrap();
    d
}

// ─── La route du PDF ────────────────────────────────────────────────────────

#[sqlx::test]
async fn le_pdf_se_telecharge_avec_le_bon_nom(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    let s = site(&pool);

    let (statut, entetes, octets) = reponse(&s, &format!("/devis/{d}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(entetes[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(entetes[header::CONTENT_DISPOSITION], "inline; filename=\"Devis-2026-57.pdf\"");
    assert_eq!(entetes[header::CACHE_CONTROL], "no-store");
    assert!(octets.starts_with(b"%PDF-") && octets.len() > 5_000);

    let (_, entetes, _) = reponse(&s, &format!("/devis/{d}/pdf?telecharger=1")).await;
    assert_eq!(entetes[header::CONTENT_DISPOSITION], "attachment; filename=\"Devis-2026-57.pdf\"");
}

#[sqlx::test]
async fn un_devis_inconnu_n_a_pas_de_pdf(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    assert_eq!(reponse(&site(&pool), "/devis/999/pdf").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn un_devis_vide_donne_quand_meme_un_pdf(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let (statut, _, octets) = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
}

#[sqlx::test]
async fn le_pdf_tient_sur_plusieurs_pages_quand_il_y_a_beaucoup_de_lignes(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    let reduit = taux(&pool, 600).await;
    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id)
         SELECT $1, 100 + n, 'poste_libre', 'Ligne numéro ' || n, 100000, 'pce', 1000000, $2 FROM generate_series(1, 120) n",
    )
    .bind(d).bind(reduit)
    .execute(&pool).await.unwrap();
    let (_, _, octets) = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await;
    let doc = lopdf_pages(&octets);
    assert!(doc >= 3, "{doc} pages seulement");
}

/// Le nombre de pages d'un PDF (compté sans autre bibliothèque : chaque page est un objet « /Type /Page »).
fn lopdf_pages(octets: &[u8]) -> usize {
    let texte = String::from_utf8_lossy(octets);
    texte.matches("/Type /Page\n").count() + texte.matches("/Type/Page/").count() + texte.matches("/Type /Page ").count()
}

// ─── Les conditions générales ───────────────────────────────────────────────

#[sqlx::test]
async fn les_conditions_generales_sont_ajoutees_a_la_suite(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    let sans = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await.2;

    // Deux pages de conditions générales, fabriquées avec le même outil.
    let conditions = ubifacture::pdf::generer("Article 1\n#pagebreak()\nArticle 2".to_string(), Default::default()).unwrap();
    sqlx::query("INSERT INTO societe_fichiers (usage, nom, type_mime, contenu) VALUES ('conditions_generales', 'cg.pdf', 'application/pdf', $1)")
        .bind(&conditions)
        .execute(&pool).await.unwrap();
    let avec = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await.2;
    assert!(avec.starts_with(b"%PDF-"));
    assert!(avec.len() > sans.len(), "le PDF avec les conditions générales est plus gros");
}

#[sqlx::test]
async fn des_conditions_generales_illisibles_ne_bloquent_pas_le_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    sqlx::query("INSERT INTO societe_fichiers (usage, nom, type_mime, contenu) VALUES ('conditions_generales', 'cg.pdf', 'application/pdf', $1)")
        .bind(b"%PDF-1.4 abime".to_vec())
        .execute(&pool).await.unwrap();
    let (statut, _, octets) = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
}

#[sqlx::test]
async fn un_logo_abime_ne_bloque_pas_le_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    // Un « PNG » dont seul l'en-tête est bon : Typst ne peut pas le lire.
    let faux_png: Vec<u8> = [&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..], b"pas une image"].concat();
    sqlx::query("INSERT INTO societe_fichiers (usage, nom, type_mime, contenu) VALUES ('logo', 'logo.png', 'image/png', $1)")
        .bind(faux_png)
        .execute(&pool).await.unwrap();
    let (statut, _, octets) = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
}

// ─── Ce que contient le texte envoyé à Typst ────────────────────────────────

#[sqlx::test]
async fn le_pdf_reprend_les_informations_et_les_lignes(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    let source = typst(&pool, d).await;
    for texte in [
        "\"2026-57\"", "\"C44 Volet\"", "\"15/10/2026\"", "\"Marc\"",
        "\"Ubique SRL\"", "\"Rue du Bâtiment 5\"", "\"4000 Liège\"", "\"BE0123456789\"", "\"BE68 5390 0754 7034\"",
        "\"Dupont SRL\"", "\"Rue Neuve 12\"", "\"1000 Bruxelles\"", "\"BE0987654321\"",
        "\"Fourniture des matériaux\"", "\"Volet roulant\"", "\"2,5\"", "\"pce\"", "\"19,99\u{a0}€\"", "\"6 %\"", "\"49,98\u{a0}€\"",
        "\"7 jour(s) après date de facturation\"", "\"Acompte de 30 %\"", "\"Chantier accessible\"",
        "\"52,98\u{a0}€\"",
    ] {
        assert!(source.contains(texte), "{texte}");
    }
    // Le chantier reprend l'adresse du client ; pas de « facturé à » ; pas de logo.
    assert!(source.contains("chantier: \"Rue Neuve 12, 1000 Bruxelles\""));
    assert!(source.contains("facturation: none") && source.contains("logo: none"));
}

#[sqlx::test]
async fn un_autre_client_de_facturation_et_une_adresse_de_chantier_propre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Client SA') RETURNING id").fetch_one(&pool).await.unwrap();
    let assurance: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, rue, numero, code_postal, ville, pays) VALUES ('Assurance Belfius', 'Boulevard Pachéco', '44', '1000', 'Bruxelles', 'BE') RETURNING id").fetch_one(&pool).await.unwrap();
    let d: i64 = sqlx::query_scalar(
        "INSERT INTO devis (numero, client_entreprise_id, facturation_identique, facturation_entreprise_id, chantier_identique,
                            chantier_rue, chantier_numero, chantier_code_postal, chantier_ville, chantier_pays)
         VALUES (1, $1, false, $2, false, 'Rue de Paris', '3', '75001', 'Paris', 'FR') RETURNING id",
    )
    .bind(client).bind(assurance).fetch_one(&pool).await.unwrap();
    let source = typst(&pool, d).await;
    assert!(source.contains("facturation: (nom: \"Assurance Belfius\"") && source.contains("\"Boulevard Pachéco 44\""));
    assert!(source.contains("chantier: \"Rue de Paris 3, 75001 Paris, France\""));
}

#[sqlx::test]
async fn les_textes_tapes_ne_peuvent_pas_s_echapper_du_texte_typst(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let piege = "\") #panic(\"boom\") [*gras*] \\ $x$ #import \"a\"";
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero, reference, remarque_1, remarque_2, responsable) VALUES (1, $1, $1, $1, $1) RETURNING id")
        .bind(piege)
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description, remarque) VALUES ($1, 1, 'texte', $2, $2)").bind(d).bind(piege).execute(&pool).await.unwrap();
    // Le piège reste du texte : le PDF se fabrique sans erreur.
    let (statut, _, octets) = reponse(&site(&pool), &format!("/devis/{d}/pdf")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(octets.starts_with(b"%PDF-"));
    assert!(typst(&pool, d).await.contains("\\\") #panic(\\\"boom\\\")"));
}

#[sqlx::test]
async fn un_saut_de_page_coupe_le_tableau_en_deux(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    for (position, genre, texte) in [(1, "texte", "avant"), (2, "saut_de_page", ""), (3, "texte", "apres")] {
        sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description) VALUES ($1, $2, $3::text::genre_ligne, $4)")
            .bind(d).bind(position).bind(genre).bind(texte).execute(&pool).await.unwrap();
    }
    let source = typst(&pool, d).await;
    let segments = source.split("segments: ").nth(1).unwrap();
    assert!(segments.contains("\"avant\"") && segments.contains("\"apres\""));
    // Deux segments : le saut n'est pas une ligne, il sépare.
    assert_eq!(segments.matches("(type: ").count(), 2);
    assert!(segments.starts_with("(((type: \"texte\"") && segments.contains("),), ((type: \"texte\""));
}

#[sqlx::test]
async fn le_pdf_du_client_ne_contient_aucune_donnee_de_marge(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    sqlx::query("UPDATE devis SET marge_activee = true, frais_generaux_centiemes = 1717 WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    sqlx::query("UPDATE devis_lignes SET prix_achat_unitaire_e5 = 7321000 WHERE genre = 'poste_libre'").execute(&pool).await.unwrap();
    let source = typst(&pool, d).await;
    for interdit in ["73,21", "7321", "17,17", "1717", "Rentabilit", "rentabilit"] {
        assert!(!source.contains(interdit), "{interdit}");
    }
}

#[sqlx::test]
async fn l_ajustement_figure_dans_le_pdf(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d = devis_complet(&pool).await;
    sqlx::query("UPDATE devis SET ajustement_libelle = 'Remise commerciale', ajustement_mode = 'montant', ajustement_valeur = -1000 WHERE id = $1").bind(d).execute(&pool).await.unwrap();
    let source = typst(&pool, d).await;
    assert!(source.contains("a_ajustement: true") && source.contains("\"Remise commerciale\"") && source.contains("\"-10,00\u{a0}€\""));
}

// ─── L'onglet Visualisation ─────────────────────────────────────────────────

#[sqlx::test]
async fn l_onglet_montre_le_pdf_et_les_boutons(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    societe(&pool).await;
    let d = devis_complet(&pool).await;
    let (statut, html) = page(&site(&pool), &format!("/devis/{d}/apercu")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(html.contains(&format!("<iframe class=\"w-full rounded-box bg-white shadow\" style=\"height: 80vh\" src=\"/devis/{d}/pdf\"")));
    assert!(html.contains(&format!("href=\"/devis/{d}/pdf?telecharger=1\"")) && html.contains("Télécharger le PDF"));
    assert!(html.contains(&format!("tab tab-active\" href=\"/devis/{d}/apercu\"")));
    assert_eq!(page(&site(&pool), "/devis/999/apercu").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn la_liste_de_ce_qu_il_reste_a_completer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let d: i64 = sqlx::query_scalar("INSERT INTO devis (numero) VALUES (1) RETURNING id").fetch_one(&pool).await.unwrap();
    let (_, html) = page(&site(&pool), &format!("/devis/{d}/apercu")).await;
    for manque in ["La référence du devis", "Le responsable", "Le client", "Les conditions de paiement", "Au moins une ligne chiffrée", "Les coordonnées de votre société"] {
        assert!(html.contains(manque), "{manque}");
    }

    // Un devis complet, une société complète : plus rien à signaler.
    societe(&pool).await;
    let complet = devis_complet(&pool).await;
    let (_, html) = page(&site(&pool), &format!("/devis/{complet}/apercu")).await;
    assert!(!html.contains("À compléter avant d&#39;envoyer") && !html.contains("À compléter avant d'envoyer"));
}

#[sqlx::test]
async fn la_liste_signale_une_societe_sans_tva_ni_compte(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('Ubique', 'Ubique', 'a@b.be', '0')").execute(&pool).await.unwrap();
    let d = devis_complet(&pool).await;
    let (_, html) = page(&site(&pool), &format!("/devis/{d}/apercu")).await;
    assert!(html.contains("Le numéro de TVA de votre société") && html.contains("Un compte bancaire par défaut"));
    assert!(!html.contains("Les coordonnées de votre société"));
}
