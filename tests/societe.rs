// Tests de l'écran « paramètres de la société ».

mod commun;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use ubifacture::{AppState, app};

const TVA: &str = "BE0726407561";
const IBAN: &str = "BE68539007547034";
const IBAN2: &str = "FR1420041010050500013M02606";
const PNG: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
const BASE: &str = "nom=Ubique+Building+Technics&nom_abrege=Ubique&pays=BE&email=info%40ubique.be&telephone=0477123456";

fn site(pool: &PgPool) -> Router {
    app(AppState { db: pool.clone() })
}

struct Reponse {
    statut: StatusCode,
    entetes: HeaderMap,
    octets: Vec<u8>,
}

impl Reponse {
    fn texte(&self) -> String {
        String::from_utf8_lossy(&self.octets).into_owned()
    }
    fn lieu(&self) -> Option<&str> {
        self.entetes.get(header::LOCATION).map(|v| v.to_str().unwrap())
    }
}

async fn envoyer(site: &Router, requete: Request<Body>) -> Reponse {
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let entetes = r.headers().clone();
    let octets = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    Reponse { statut, entetes, octets }
}

async fn lire(site: &Router, uri: &str) -> Reponse {
    envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await
}

async fn poster(site: &Router, uri: &str, corps: &str) -> Reponse {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    envoyer(site, requete).await
}

async fn televerser(site: &Router, uri: &str, nom_fichier: &str, contenu: &[u8]) -> Reponse {
    let limite = "----limite-de-test";
    let mut corps = Vec::new();
    corps.extend(
        format!(
            "--{limite}\r\nContent-Disposition: form-data; name=\"fichier\"; filename=\"{nom_fichier}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    corps.extend(contenu);
    corps.extend(format!("\r\n--{limite}--\r\n").as_bytes());
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={limite}"))
        .body(Body::from(corps))
        .unwrap();
    envoyer(site, requete).await
}

async fn compter(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

fn pdf(taille: usize) -> Vec<u8> {
    let mut c = b"%PDF-1.4\n".to_vec();
    c.resize(taille, b' ');
    c
}

// ─── Formulaire ─────────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_page_vide_propose_un_compte_vide_et_les_boutons_d_ajout(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let r = lire(&site(&pool), "/parametres/societe").await;
    assert_eq!(r.statut, StatusCode::OK);
    let page = r.texte();
    assert!(page.contains("Société *") && page.contains("Nom abrégé *"));
    assert!(page.contains("Ajouter un e-mail") && page.contains("Ajouter un téléphone"));
    assert!(page.contains("Ajouter un compte bancaire"));
    assert_eq!(page.matches("name=\"iban\"").count(), 1);
    assert!(page.contains("Aucun logo.") && page.contains("Aucun fichier."));
    assert!(page.contains("<option value=\"BE\" selected>Belgique</option>"));
    assert!(!page.contains("Paramètres enregistrés."));
}

#[sqlx::test]
async fn enregistrer_puis_retrouver_les_valeurs(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let corps = format!(
        "{BASE}&rue=Rue+du+Beau+Site&numero=12&code_postal=1390&ville=Grez-Doiceau&site_web=https%3A%2F%2Fubique.be\
         &emails_supplementaires=compta%40ubique.be&emails_supplementaires=&emails_supplementaires=devis%40ubique.be\
         &telephones_supplementaires=02+123+45+67&telephones_supplementaires=\
         &numero_tva={TVA}&iban={IBAN}&bic=gebabebb&iban=&bic="
    );
    let r = poster(&s, "/parametres/societe", &corps).await;
    assert_eq!(r.statut, StatusCode::SEE_OTHER);
    assert_eq!(r.lieu(), Some("/parametres/societe?ok=1"));

    let (nom, tva, emails, telephones): (String, Option<String>, Vec<String>, Vec<String>) = sqlx::query_as(
        "SELECT nom, numero_tva, emails_supplementaires, telephones_supplementaires FROM societe",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nom, "Ubique Building Technics");
    assert_eq!(tva.as_deref(), Some("BE 0726 407 561"));
    assert_eq!(emails, vec!["compta@ubique.be", "devis@ubique.be"]);
    assert_eq!(telephones, vec!["02 123 45 67"]);

    let (iban, bic, par_defaut): (String, Option<String>, bool) =
        sqlx::query_as("SELECT iban, bic, par_defaut FROM societe_comptes_bancaires").fetch_one(&pool).await.unwrap();
    assert_eq!((iban.as_str(), bic.as_deref(), par_defaut), ("BE68 5390 0754 7034", Some("GEBABEBB"), true));

    let page = lire(&s, "/parametres/societe?ok=1").await.texte();
    assert!(page.contains("Paramètres enregistrés."));
    assert!(page.contains("value=\"Ubique Building Technics\"") && page.contains("value=\"Grez-Doiceau\""));
    assert!(page.contains("value=\"compta@ubique.be\"") && page.contains("value=\"devis@ubique.be\""));
    assert!(page.contains("value=\"BE 0726 407 561\"") && page.contains("value=\"BE68 5390 0754 7034\""));
    assert!(page.contains("value=\"GEBABEBB\""));
    assert!(!lire(&s, "/parametres/societe").await.texte().contains("Paramètres enregistrés."));
}

#[sqlx::test]
async fn enregistrer_deux_fois_met_a_jour_sans_dupliquer(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/parametres/societe", &format!("{BASE}&iban={IBAN}&bic=")).await;
    let r = poster(
        &s,
        "/parametres/societe",
        "nom=Nouveau+Nom&nom_abrege=NN&pays=FR&email=a%40b.fr&telephone=1",
    )
    .await;
    assert_eq!(r.statut, StatusCode::SEE_OTHER);
    assert_eq!(compter(&pool, "societe").await, 1);
    let (nom, pays): (String, String) =
        sqlx::query_as("SELECT nom, pays FROM societe").fetch_one(&pool).await.unwrap();
    assert_eq!((nom.as_str(), pays.as_str()), ("Nouveau Nom", "FR"));
    // Plus aucun compte envoyé : les anciens sont retirés.
    assert_eq!(compter(&pool, "societe_comptes_bancaires").await, 0);
}

#[sqlx::test]
async fn les_erreurs_sont_toutes_signalees_et_rien_n_est_enregistre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let r = poster(
        &site(&pool),
        "/parametres/societe",
        "nom=&nom_abrege=&pays=BE&email=pas-un-email&telephone=&numero_tva=BE0726407562\
         &iban=BE68539007547035&bic=xx&iban=&bic=ABCDEFGH",
    )
    .await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    let page = r.texte();
    assert!(page.contains("Le nom de la société est obligatoire."));
    assert!(page.contains("Le nom abrégé est obligatoire."));
    assert!(page.contains("pas-un-email"));
    assert!(page.contains("Le numéro de téléphone est obligatoire."));
    assert!(page.contains("Ce numéro de TVA belge n&#39;est pas valide"));
    assert!(page.contains("Cet IBAN n&#39;est pas valide (la clé de contrôle ne correspond pas)."));
    assert!(page.contains("Un BIC est renseigné sans IBAN."));
    assert_eq!(compter(&pool, "societe").await, 0);
    assert_eq!(compter(&pool, "societe_comptes_bancaires").await, 0);
}

#[sqlx::test]
async fn un_bic_invalide_et_un_iban_en_double_sont_refuses(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let r = poster(&s, "/parametres/societe", &format!("{BASE}&iban={IBAN}&bic=xx")).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("Le BIC « XX » n&#39;est pas valide."));

    let r = poster(&s, "/parametres/societe", &format!("{BASE}&iban={IBAN}&bic=&iban=BE68+5390+0754+7034&bic=")).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("Le même IBAN est saisi deux fois."));
    assert_eq!(compter(&pool, "societe_comptes_bancaires").await, 0);
}

#[sqlx::test]
async fn le_formulaire_garde_ce_qui_a_ete_tape_en_cas_d_erreur(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let r = poster(
        &site(&pool),
        "/parametres/societe",
        &format!("nom=Ma+Societe&nom_abrege=&pays=BE&email=a%40b.be&telephone=1&ville=Namur&emails_supplementaires=x%40y.be&iban={IBAN2}&bic="),
    )
    .await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    let page = r.texte();
    assert!(page.contains("value=\"Ma Societe\"") && page.contains("value=\"Namur\""));
    assert!(page.contains("value=\"x@y.be\""));
    assert!(page.contains(&format!("value=\"{IBAN2}\"")));
}

#[sqlx::test]
async fn le_premier_compte_est_celui_par_defaut(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/parametres/societe", &format!("{BASE}&iban={IBAN}&bic=&iban={IBAN2}&bic=")).await;
    let comptes: Vec<(String, bool)> =
        sqlx::query_as("SELECT iban, par_defaut FROM societe_comptes_bancaires ORDER BY id").fetch_all(&pool).await.unwrap();
    assert_eq!(comptes, vec![("BE68 5390 0754 7034".to_string(), true), ("FR14 2004 1010 0505 0001 3M02 606".to_string(), false)]);

    // On supprime le premier : le second devient le compte par défaut.
    poster(&s, "/parametres/societe", &format!("{BASE}&iban={IBAN2}&bic=")).await;
    let comptes: Vec<(String, bool)> =
        sqlx::query_as("SELECT iban, par_defaut FROM societe_comptes_bancaires").fetch_all(&pool).await.unwrap();
    assert_eq!(comptes, vec![("FR14 2004 1010 0505 0001 3M02 606".to_string(), true)]);
}

#[sqlx::test]
async fn la_tva_belge_sans_prefixe_est_completee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    poster(&site(&pool), "/parametres/societe", &format!("{BASE}&numero_tva=726.407.561")).await;
    let tva: Option<String> = sqlx::query_scalar("SELECT numero_tva FROM societe").fetch_one(&pool).await.unwrap();
    assert_eq!(tva.as_deref(), Some("BE 0726 407 561"));
}

#[sqlx::test]
async fn le_html_tape_dans_le_nom_est_neutralise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/parametres/societe", "nom=%3Cscript%3Ealert(1)%3C%2Fscript%3E&nom_abrege=X&email=a%40b.be&telephone=1").await;
    let page = lire(&s, "/parametres/societe").await.texte();
    assert!(!page.contains("<script>alert(1)"));
    assert!(page.contains("&#60;script&#62;alert(1)"));
}

// ─── Lignes ajoutées par htmx ───────────────────────────────────────────────

#[sqlx::test]
async fn les_boutons_d_ajout_renvoient_une_ligne(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let email = lire(&s, "/parametres/societe/ligne/email").await.texte();
    assert!(email.contains("name=\"emails_supplementaires\"") && email.contains("type=\"email\""));
    assert!(!email.contains("<html"));
    let tel = lire(&s, "/parametres/societe/ligne/telephone").await.texte();
    assert!(tel.contains("name=\"telephones_supplementaires\"") && tel.contains("type=\"tel\""));
    let compte = lire(&s, "/parametres/societe/ligne/compte").await.texte();
    assert!(compte.contains("name=\"iban\"") && compte.contains("name=\"bic\""));
    let vide = lire(&s, "/parametres/societe/ligne/vide").await;
    assert_eq!(vide.statut, StatusCode::OK);
    assert!(vide.octets.is_empty());
}

// ─── Logo ───────────────────────────────────────────────────────────────────

#[sqlx::test]
async fn envoyer_afficher_remplacer_et_retirer_le_logo(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    assert_eq!(lire(&s, "/parametres/societe/logo").await.statut, StatusCode::NOT_FOUND);

    let r = televerser(&s, "/parametres/societe/logo/envoyer", "mon logo.png", &PNG).await;
    assert_eq!(r.statut, StatusCode::SEE_OTHER);
    assert_eq!(r.lieu(), Some("/parametres/societe?ok=1"));

    let logo = lire(&s, "/parametres/societe/logo").await;
    assert_eq!(logo.statut, StatusCode::OK);
    assert_eq!(logo.entetes.get(header::CONTENT_TYPE).unwrap(), "image/png");
    assert_eq!(logo.entetes.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
    assert_eq!(logo.octets, PNG);
    assert!(lire(&s, "/parametres/societe").await.texte().contains("src=\"/parametres/societe/logo?v="));

    // Le remplacer par un JPEG ne crée pas de seconde ligne.
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    televerser(&s, "/parametres/societe/logo/envoyer", "autre.jpg", &jpeg).await;
    assert_eq!(compter(&pool, "societe_fichiers").await, 1);
    assert_eq!(lire(&s, "/parametres/societe/logo").await.entetes.get(header::CONTENT_TYPE).unwrap(), "image/jpeg");

    let r = poster(&s, "/parametres/societe/logo/retirer", "").await;
    assert_eq!(r.statut, StatusCode::SEE_OTHER);
    assert_eq!(lire(&s, "/parametres/societe/logo").await.statut, StatusCode::NOT_FOUND);
    assert!(lire(&s, "/parametres/societe").await.texte().contains("Aucun logo."));
}

#[sqlx::test]
async fn le_logo_refuse_les_svg_les_pdf_et_les_fichiers_trop_gros(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let uri = "/parametres/societe/logo/envoyer";

    let r = televerser(&s, uri, "logo.png", b"<svg xmlns='http://www.w3.org/2000/svg' onload='alert(1)'/>").await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("Le logo doit être une image PNG ou JPEG."));

    let r = televerser(&s, uri, "logo.png", &pdf(100)).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);

    let mut gros = PNG.to_vec();
    gros.resize(1024 * 1024 + 1, 0);
    let r = televerser(&s, uri, "logo.png", &gros).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("Le logo dépasse 1 Mo."));

    let r = televerser(&s, uri, "logo.png", b"").await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("Choisis un fichier"));

    assert_eq!(compter(&pool, "societe_fichiers").await, 0);
}

// ─── Conditions générales ───────────────────────────────────────────────────

#[sqlx::test]
async fn envoyer_et_lire_les_conditions_generales(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let r = televerser(&s, "/parametres/societe/conditions-generales/envoyer", "C:\\docs\\Mes CG;2026.pdf", &pdf(500)).await;
    assert_eq!(r.statut, StatusCode::SEE_OTHER);

    let (nom, type_mime): (String, String) =
        sqlx::query_as("SELECT nom, type_mime FROM societe_fichiers WHERE usage = 'conditions_generales'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(nom, "Mes CG_2026.pdf");
    assert_eq!(type_mime, "application/pdf");

    let fichier = lire(&s, "/parametres/societe/conditions-generales").await;
    assert_eq!(fichier.statut, StatusCode::OK);
    assert_eq!(fichier.entetes.get(header::CONTENT_TYPE).unwrap(), "application/pdf");
    assert_eq!(fichier.entetes.get(header::CONTENT_DISPOSITION).unwrap(), "inline; filename=\"Mes CG_2026.pdf\"");
    assert_eq!(fichier.octets.len(), 500);

    let page = lire(&s, "/parametres/societe").await.texte();
    assert!(page.contains("Mes CG_2026.pdf") && page.contains("href=\"/parametres/societe/conditions-generales\""));

    poster(&s, "/parametres/societe/conditions-generales/retirer", "").await;
    assert_eq!(lire(&s, "/parametres/societe/conditions-generales").await.statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn les_conditions_generales_refusent_ce_qui_n_est_pas_un_pdf_ou_est_trop_gros(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let uri = "/parametres/societe/conditions-generales/envoyer";

    // Un nom en .pdf ne suffit pas : c'est le contenu qui compte.
    let r = televerser(&s, uri, "cg.pdf", &PNG).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("doit être un fichier PDF."));

    let r = televerser(&s, uri, "cg.pdf", &pdf(2 * 1024 * 1024 + 1)).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("dépasse 2 Mo."));

    // Au-delà de la limite de la requête : message clair plutôt qu'une erreur brute.
    let r = televerser(&s, uri, "cg.pdf", &pdf(4 * 1024 * 1024)).await;
    assert_eq!(r.statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.texte().contains("trop volumineux"));

    assert_eq!(compter(&pool, "societe_fichiers").await, 0);
}

#[sqlx::test]
async fn un_pdf_de_deux_mo_pile_est_accepte(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let r = televerser(&site(&pool), "/parametres/societe/conditions-generales/envoyer", "cg.pdf", &pdf(2 * 1024 * 1024)).await;
    assert_eq!(r.statut, StatusCode::SEE_OTHER);
}
