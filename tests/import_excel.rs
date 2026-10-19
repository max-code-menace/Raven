// Tests de l'import d'un devis ou d'un avenant depuis le modèle Excel (après l'étape 9).
// Les classeurs .xlsx sont fabriqués ici, sur le modèle de l'exemple de Mahx (sans aucun nom de client).
// Pour essayer un vrai fichier sans l'enregistrer nulle part :
//   $env:IMPORT_FICHIER = "C:\chemin\Devis.xlsx"; cargo test --test import_excel -- --ignored --nocapture

mod commun;

use std::io::Write;

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

/// Envoie un fichier comme le formulaire (multipart/form-data, champ « fichier »).
async fn televerser(site: &Router, uri: &str, nom: &str, octets: &[u8]) -> (StatusCode, Option<String>, String) {
    let limite = "----limite-ubifacture";
    let mut corps = format!(
        "--{limite}\r\nContent-Disposition: form-data; name=\"fichier\"; filename=\"{nom}\"\r\nContent-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\r\n\r\n"
    )
    .into_bytes();
    corps.extend_from_slice(octets);
    corps.extend_from_slice(format!("\r\n--{limite}--\r\n").as_bytes());
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={limite}"))
        .body(Body::from(corps))
        .unwrap();
    envoyer(site, requete).await
}

/// Une case : texte ou nombre (avec, si besoin, la formule et sa valeur, comme Excel l'enregistre).
enum C {
    T(&'static str),
    N(f64),
    F(&'static str, f64),
}

fn colonne(i: usize) -> char {
    (b'A' + i as u8) as char
}

/// Fabrique un classeur .xlsx minimal avec une feuille « Entr ».
fn classeur(lignes: &[(usize, Vec<(usize, C)>)]) -> Vec<u8> {
    let echapper = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut feuille = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    for (r, cases) in lignes {
        feuille.push_str(&format!("<row r=\"{r}\">"));
        for (c, v) in cases {
            let reference = format!("{}{r}", colonne(*c));
            match v {
                C::T(t) => feuille.push_str(&format!("<c r=\"{reference}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>", echapper(t))),
                C::N(n) => feuille.push_str(&format!("<c r=\"{reference}\"><v>{n}</v></c>")),
                C::F(f, n) => feuille.push_str(&format!("<c r=\"{reference}\"><f>{f}</f><v>{n}</v></c>")),
            }
        }
        feuille.push_str("</row>");
    }
    feuille.push_str("</sheetData></worksheet>");
    let fichiers = [
        (
            "[Content_Types].xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>".to_string(),
        ),
        (
            "_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>".to_string(),
        ),
        (
            "xl/workbook.xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Entr\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>".to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".to_string(),
        ),
        ("xl/worksheets/sheet1.xml", feuille),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (nom, contenu) in fichiers {
        zip.start_file(nom, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(contenu.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Le modèle de l'exemple : en-tête ligne 19-20, lots, sous-titres, textes, postes, option, sous-totaux, récapitulatif.
/// Total : 200 + 90 + 3 570 + 0 + 2 820 = 6 680,00 € HT.
fn modele() -> Vec<u8> {
    use C::*;
    classeur(&[
        (3, vec![(7, T("DEVIS "))]),
        (17, vec![(0, T("PROJET: rénovation générale"))]),
        (19, vec![(0, T("POSTE")), (2, T("DESCRIPTION")), (3, T("HAUT.")), (4, T("LONG.")), (5, T("LARG.")), (7, T("Qté")), (8, T("UNITE")), (9, T("PRIX")), (10, T("PRIX"))]),
        (20, vec![(7, T("PREVUE")), (9, T("UNITAIRE")), (10, T("H.T.V.A."))]),
        (22, vec![(0, T("LOT 0")), (2, T("ENTREPRISE - CHANTIER"))]),
        (24, vec![(2, T("INSTALLATION DE CHANTIER"))]),
        (27, vec![(2, T("Déplacement ")), (7, N(4.0)), (8, T("QP")), (9, N(50.0)), (10, F("H27*J27", 200.0))]),
        (29, vec![(8, T("SOUS-TOTAL")), (10, F("SUM(K25:K28)", 200.0))]),
        (31, vec![(0, T("LOT 1")), (2, T("DEMOLITION ET RECALIBRAGE"))]),
        (33, vec![(2, T("Recalibrage de la baie ( création d'une trémie d'escalier )"))]),
        (34, vec![(2, T("Prise de niveau / mesure ")), (3, N(1.0)), (4, T("fft")), (5, N(100.0)), (6, F("D34*F34", 100.0)), (7, N(1.0)), (8, T("fft")), (9, N(90.0)), (10, F("H34*J34", 90.0))]),
        (42, vec![(8, T("SOUS-TOTAL")), (10, N(90.0))]),
        (44, vec![(0, T("LOT 2")), (2, T("PARACHEVEMENT INTERIEUR"))]),
        (45, vec![(1, T("2.1")), (2, T("ENDUITS INTERIEURS"))]),
        (46, vec![(2, T("Les enduits décrits ci-dessous comprennent la préparation du support."))]),
        (49, vec![(1, T("option")), (2, T("Plafonnage des pignons ")), (7, N(1.0)), (8, T("fft")), (9, N(3570.0)), (10, F("H49*J49", 3570.0))]),
        (61, vec![(2, T("Store extérieur solaire ( en attente )")), (7, N(1.0)), (8, T("fft")), (10, F("H61*J61", 0.0))]),
        (68, vec![(2, T("Chape pour passages techniques"))]),
        (69, vec![(2, T("Fourniture et pose d'une chape adhérente."))]),
        (70, vec![(7, N(60.0)), (8, T("M2")), (9, N(47.0)), (10, F("H70*J70", 2820.0))]),
        (74, vec![(8, T("SOUS-TOTAL")), (10, N(6390.0))]),
        (76, vec![(3, T("RECAPITULATIF"))]),
        (78, vec![(7, T("0. ENTREPRISE - CHANTIER")), (10, N(200.0))]),
        (82, vec![(8, T("TOTAL H.T.V.A.")), (10, F("SUM(K78:K80)", 6680.0))]),
        (83, vec![(8, T("T.V.A. 6%")), (10, N(400.8))]),
    ])
}

async fn nouveau_devis(pool: &PgPool, s: &Router) -> i64 {
    let (_, lieu, _) = poster(s, "/devis/import", "").await;
    let lieu = lieu.unwrap();
    let id: i64 = lieu.strip_prefix("/devis/").unwrap().strip_suffix("/import").unwrap().parse().unwrap();
    sqlx::query("UPDATE devis SET taux_tva_id = NULL WHERE id = $1").bind(id).execute(pool).await.unwrap();
    id
}

fn import_depuis(lieu: &str) -> i64 {
    lieu.rsplit('/').next().unwrap().parse().unwrap()
}

#[sqlx::test]
async fn importer_le_modele_dans_un_nouveau_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let id = nouveau_devis(&pool, &s).await;
    let (code, page) = lire(&s, &format!("/devis/{id}/import")).await;
    assert!(code == StatusCode::OK && page.contains("Importer depuis Excel") && page.contains("enctype=\"multipart/form-data\""));

    // Un fichier qui n'est pas un classeur : refusé, avec un message.
    let (code, _, page) = televerser(&s, &format!("/devis/{id}/import"), "notes.txt", b"bonjour").await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("pas un classeur Excel"));

    let (code, lieu, _) = televerser(&s, &format!("/devis/{id}/import"), "Devis RG.xlsx", &modele()).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    let lieu = lieu.unwrap();
    let import = import_depuis(&lieu);

    // L'aperçu : rien n'est encore ajouté au devis.
    let (_, page) = lire(&s, &lieu).await;
    for attendu in [
        "Devis RG.xlsx",
        "3 lot(s), 5 poste(s)",
        &format!("Même total que le fichier : 6\u{a0}680,00{E} HT."),
        "Taux réduit",
        "Option : Plafonnage des pignons",
        "Chape pour passages techniques\nFourniture et pose d&#39;une chape adhérente.",
        "Ligne 61 : pas de prix unitaire",
        "Importer 12 ligne(s)",
    ] {
        assert!(page.contains(attendu) || page.contains(&attendu.replace("&#39;", "'")), "{attendu}");
    }
    let lignes: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_lignes WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(lignes, 0);

    let (_, lieu, _) = poster(&s, &format!("/devis/{id}/import/{import}"), "mode=ajouter").await;
    assert_eq!(lieu.as_deref(), Some(format!("/devis/{id}/encodage?import=12").as_str()));
    let lignes: Vec<(String, String, Option<i64>, String, Option<i64>, Option<i32>)> = sqlx::query_as(
        "SELECT l.genre::text, l.description, l.quantite_e5, l.unite, l.prix_unitaire_e5, t.taux_centiemes
         FROM devis_lignes l LEFT JOIN taux_tva t ON t.id = l.taux_tva_id WHERE l.devis_id = $1 ORDER BY l.position",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let genres: Vec<&str> = lignes.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(
        genres,
        vec!["titre", "sous_titre", "poste_libre", "titre", "texte", "poste_libre", "titre", "sous_titre", "texte", "poste_libre", "poste_libre", "poste_libre"]
    );
    assert_eq!(lignes[2], ("poste_libre".into(), "Déplacement".into(), Some(400_000), "QP".into(), Some(5_000_000), Some(600)));
    assert_eq!(lignes[10].4, Some(0), "un poste sans prix est importé à 0");
    assert_eq!(lignes[11].3, "m²");
    let (ht, ttc): (i64, i64) = sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((ht, ttc), (668_000, 708_080), "6 680,00 € HT + 6 % = 7 080,80 € TTC");
    let detail: String = sqlx::query_scalar("SELECT detail FROM devis_evenements WHERE devis_id = $1 AND genre = 'import'").bind(id).fetch_one(&pool).await.unwrap();
    assert!(detail.contains("12 ligne(s) importée(s) depuis « Devis RG.xlsx »"));
    let (_, page) = lire(&s, &format!("/devis/{id}/encodage?import=12")).await;
    assert!(page.contains("Import Excel terminé : 12 ligne(s) ajoutée(s).") && page.contains("Importer depuis Excel"));
    // L'aperçu a disparu.
    assert_eq!(lire(&s, &format!("/devis/{id}/import/{import}")).await.0, StatusCode::NOT_FOUND);

    // Un second import : remplacer les lignes existantes.
    let (_, lieu, _) = televerser(&s, &format!("/devis/{id}/import"), "Devis RG.xlsx", &modele()).await;
    let lieu = lieu.unwrap();
    let (_, page) = lire(&s, &lieu).await;
    assert!(page.contains("a déjà 12 ligne(s)") && page.contains("value=\"remplacer\""));
    poster(&s, &format!("/devis/{id}/import/{}", import_depuis(&lieu)), "mode=remplacer").await;
    let nombre: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_lignes WHERE devis_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(nombre, 12);

    // Un devis envoyé ne reçoit plus d'import.
    poster(&s, &format!("/devis/{id}/statut/envoye"), "").await;
    assert_eq!(lire(&s, &format!("/devis/{id}/import")).await.0, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn un_avenant_en_moins_depuis_excel(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    use C::*;
    let s = site(&pool);
    // Un chantier : un devis accepté.
    let (_, lieu, _) = poster(&s, "/devis", "").await;
    let devis: i64 = lieu.unwrap().strip_prefix("/devis/").unwrap().split('?').next().unwrap().parse().unwrap();
    let jour: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE devis SET taux_tva_id = (SELECT id FROM taux_tva WHERE taux_centiemes = 600) WHERE id = $1").bind(devis).execute(&pool).await.unwrap();
    poster(&s, &format!("/devis/{devis}/statut/accepter"), &format!("date={jour}")).await;
    let chantier: i64 = sqlx::query_scalar("SELECT id FROM chantiers WHERE devis_id = $1").bind(devis).fetch_one(&pool).await.unwrap();
    let (_, page) = lire(&s, &format!("/chantiers/{chantier}")).await;
    assert!(page.contains("Avenant depuis Excel"));

    // « Avenant depuis Excel » crée l'avenant et ouvre l'import.
    let (_, lieu, _) = poster(&s, &format!("/chantiers/{chantier}/avenants"), "import=1").await;
    let lieu = lieu.unwrap();
    assert!(lieu.ends_with("/import"));
    let avenant: i64 = lieu.strip_prefix("/devis/").unwrap().strip_suffix("/import").unwrap().parse().unwrap();
    let fichier = classeur(&[
        (1, vec![(0, T("POSTE")), (2, T("DESCRIPTION")), (7, T("Qté")), (8, T("UNITE")), (9, T("PRIX")), (10, T("PRIX"))]),
        (2, vec![(9, T("UNITAIRE")), (10, T("H.T.V.A."))]),
        (3, vec![(0, T("LOT 1")), (2, T("MODIFICATIONS"))]),
        (4, vec![(2, T("Suppression du carrelage prévu")), (7, N(-20.0)), (8, T("M2")), (9, N(65.0)), (10, F("H4*J4", -1300.0))]),
        (5, vec![(2, T("Parquet à la place")), (7, N(20.0)), (8, T("M2")), (9, N(48.5)), (10, F("H5*J5", 970.0))]),
        (7, vec![(8, T("TOTAL H.T.V.A.")), (10, N(-330.0))]),
    ]);
    let (code, lieu, _) = televerser(&s, &format!("/devis/{avenant}/import"), "Avenant.xlsx", &fichier).await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    let lieu = lieu.unwrap();
    let (_, page) = lire(&s, &lieu).await;
    assert!(page.contains("Avenant N° 2") && page.contains(&format!("Même total que le fichier : -330,00{E} HT.")));
    assert_eq!(poster(&s, &format!("/devis/{avenant}/import/{}", import_depuis(&lieu)), "mode=ajouter").await.0, StatusCode::SEE_OTHER);
    let ht: i64 = sqlx::query_scalar("SELECT montant_ht_cents FROM devis WHERE id = $1").bind(avenant).fetch_one(&pool).await.unwrap();
    assert_eq!(ht, -33_000, "un avenant en moins");
}

#[sqlx::test]
async fn sans_taux_de_tva_et_fichier_sans_en_tete(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    use C::*;
    let s = site(&pool);
    let id = nouveau_devis(&pool, &s).await;
    // Pas de « T.V.A. x% » dans le fichier et pas de taux dans le devis : l'aperçu prévient, l'import est refusé.
    let fichier = classeur(&[
        (1, vec![(0, T("POSTE")), (2, T("DESCRIPTION")), (7, T("Qté")), (8, T("UNITE")), (9, T("PRIX"))]),
        (2, vec![(2, T("Travaux")), (7, N(1.0)), (8, T("fft")), (9, N(100.0))]),
    ]);
    let (_, lieu, _) = televerser(&s, &format!("/devis/{id}/import"), "a.xlsx", &fichier).await;
    let lieu = lieu.unwrap();
    let (_, page) = lire(&s, &lieu).await;
    assert!(page.contains("Aucun taux de TVA"));
    assert_eq!(poster(&s, &format!("/devis/{id}/import/{}", import_depuis(&lieu)), "mode=ajouter").await.0, StatusCode::CONFLICT);
    // Avec un taux par défaut, ça passe.
    sqlx::query("UPDATE devis SET taux_tva_id = (SELECT id FROM taux_tva WHERE taux_centiemes = 2100) WHERE id = $1").bind(id).execute(&pool).await.unwrap();
    let (_, lieu2, _) = poster(&s, &format!("/devis/{id}/import/{}", import_depuis(&lieu)), "mode=ajouter").await;
    assert_eq!(lieu2.as_deref(), Some(format!("/devis/{id}/encodage?import=1").as_str()));

    // Un classeur sans l'en-tête du modèle.
    let autre = classeur(&[(1, vec![(0, T("Nom")), (1, T("Prénom"))])]);
    let (code, _, page) = televerser(&s, &format!("/devis/{id}/import"), "b.xlsx", &autre).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("ne ressemble pas au modèle"));
    // Abandonner un aperçu.
    let (_, lieu, _) = televerser(&s, &format!("/devis/{id}/import"), "a.xlsx", &fichier).await;
    let import = import_depuis(&lieu.unwrap());
    assert_eq!(poster(&s, &format!("/devis/{id}/import/{import}/abandonner"), "").await.1, Some(format!("/devis/{id}/encodage")));
    assert_eq!(lire(&s, &format!("/devis/{id}/import/{import}")).await.0, StatusCode::NOT_FOUND);
}

/// Lit un vrai fichier (chemin dans IMPORT_FICHIER) et affiche ce qui serait importé. Rien n'est enregistré.
#[sqlx::test]
#[ignore]
async fn essayer_un_vrai_fichier(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let Ok(chemin) = std::env::var("IMPORT_FICHIER") else {
        println!("Indique le fichier : $env:IMPORT_FICHIER = \"C:\\chemin\\Devis.xlsx\"");
        return;
    };
    let octets = std::fs::read(&chemin).expect("fichier introuvable");
    let s = site(&pool);
    let id = nouveau_devis(&pool, &s).await;
    let (code, lieu, page) = televerser(&s, &format!("/devis/{id}/import"), "fichier.xlsx", &octets).await;
    assert_eq!(code, StatusCode::SEE_OTHER, "{page}");
    let import = import_depuis(&lieu.unwrap());
    let lignes: String = sqlx::query_scalar("SELECT lignes FROM imports_excel WHERE id = $1").bind(import).fetch_one(&pool).await.unwrap();
    let (total_fichier, alertes): (Option<i64>, String) =
        sqlx::query_as("SELECT total_fichier_cents, alertes FROM imports_excel WHERE id = $1").bind(import).fetch_one(&pool).await.unwrap();
    let lignes: Vec<serde_json::Value> = serde_json::from_str(&lignes).unwrap();
    let mut total = 0_i64;
    for l in &lignes {
        let q = l["quantite_e5"].as_i64();
        let p = l["prix_e5"].as_i64();
        if let (Some(q), Some(p)) = (q, p) {
            total += ((q as i128 * p as i128 + 50_000_000) / 100_000_000) as i64;
        }
        println!("{:>4}  {:<12} {:<60} {:>10} {:<6} {:>10}", l["ligne_excel"], l["genre"].as_str().unwrap(), l["description"].as_str().unwrap().replace('\n', " / ").chars().take(60).collect::<String>(), q.map(|q| q as f64 / 1e5).unwrap_or(0.0), l["unite"].as_str().unwrap(), p.map(|p| p as f64 / 1e5).unwrap_or(0.0));
    }
    println!("Total des lignes : {:.2} € HT ; total du fichier : {:?}", total as f64 / 100.0, total_fichier.map(|t| t as f64 / 100.0));
    println!("Alertes :\n{alertes}");
}
