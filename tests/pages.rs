// Tests des écrans : chaque test reçoit une base temporaire et appelle le site comme un navigateur.

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
    let reponse = site.clone().oneshot(requete).await.unwrap();
    let statut = reponse.status();
    let lieu = reponse
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    let octets = reponse.into_body().collect().await.unwrap().to_bytes();
    (statut, lieu, String::from_utf8(octets.to_vec()).unwrap())
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (s, corps)
}

async fn lire_htmx(site: &Router, uri: &str) -> (StatusCode, String) {
    let requete = Request::builder().uri(uri).header("hx-request", "true").body(Body::empty()).unwrap();
    let (s, _, corps) = envoyer(site, requete).await;
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

async fn compter(pool: &PgPool, table: &str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap();
    n
}

// ─── Pages de base ──────────────────────────────────────────────────────────

#[sqlx::test]
async fn la_page_d_accueil_s_affiche(pool: PgPool) {
    let (statut, corps) = lire(&site(&pool), "/").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("Ubifacture"));
    assert!(corps.contains("href=\"/entreprises\""));
    assert!(corps.contains("href=\"/contacts\""));
}

#[sqlx::test]
async fn la_route_sante_interroge_la_base(pool: PgPool) {
    let (statut, corps) = lire(&site(&pool), "/sante").await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(corps, "Le serveur et la base répondent.");
}

#[sqlx::test]
async fn une_page_inconnue_donne_404(pool: PgPool) {
    let (statut, _) = lire(&site(&pool), "/nimporte-quoi").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

// ─── Entreprises ────────────────────────────────────────────────────────────

#[sqlx::test]
async fn liste_vide_au_depart(pool: PgPool) {
    let (statut, corps) = lire(&site(&pool), "/entreprises").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("Aucune entreprise trouvée."));
    assert!(corps.contains("0 résultat(s)"));
}

#[sqlx::test]
async fn le_formulaire_de_creation_s_affiche(pool: PgPool) {
    let (statut, corps) = lire(&site(&pool), "/entreprises/nouveau").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("Nouvelle entreprise"));
    assert!(corps.contains("name=\"profils\""));
    assert!(corps.contains("Taux réduit (6,00 %)"));
    assert!(corps.contains("10 jour(s) fin du mois"));
    assert!(corps.contains("Paiement comptant"));
    // Une entreprise n'a ni prénom ni civilité.
    assert!(!corps.contains("name=\"prenom\""));
}

#[sqlx::test]
async fn creer_une_entreprise_avec_plusieurs_profils(pool: PgPool) {
    let s = site(&pool);
    let (statut, lieu, _) = poster(
        &s,
        "/entreprises",
        "nom=Dupont+SRL&profils=client&profils=fournisseur_materiaux&numero_tva=BE0123456789\
         &rue=Rue+du+Pont&numero=12&code_postal=1390&ville=Grez-Doiceau&pays=BE\
         &franco=1+150%2C50&email=info%40dupont.be&facturation_identique=on&langue=fr",
    )
    .await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu.as_deref(), Some("/entreprises"));

    let (profils, franco, identique): (Vec<String>, Option<i64>, bool) =
        sqlx::query_as("SELECT profils, franco_cents, facturation_identique FROM entreprises WHERE nom = 'Dupont SRL'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(profils, vec!["client", "fournisseur_materiaux"]);
    assert_eq!(franco, Some(115_050));
    assert!(identique);

    let (_, corps) = lire(&s, "/entreprises").await;
    assert!(corps.contains("Dupont SRL"));
    assert!(corps.contains("Client"));
    assert!(corps.contains("Fournisseur - Matériaux"));
    assert!(corps.contains("Rue du Pont 12, 1390 Grez-Doiceau, Belgique"));
    assert!(corps.contains("1 résultat(s)"));
}

#[sqlx::test]
async fn un_nom_vide_est_refuse_et_rien_n_est_enregistre(pool: PgPool) {
    let (statut, _, corps) = poster(&site(&pool), "/entreprises", "nom=+&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Le nom est obligatoire."));
    assert_eq!(compter(&pool, "entreprises").await, 0);
}

#[sqlx::test]
async fn les_erreurs_gardent_ce_qui_a_ete_tape(pool: PgPool) {
    let (statut, _, corps) = poster(
        &site(&pool),
        "/entreprises",
        "nom=Dupont&email=pas-un-email&franco=beaucoup&ville=Namur&profils=client&langue=fr",
    )
    .await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("L&#39;e-mail n&#39;est pas valide."));
    assert!(corps.contains("Le franco doit être un montant en euros"));
    assert!(corps.contains("value=\"Dupont\""));
    assert!(corps.contains("value=\"Namur\""));
    assert!(corps.contains("value=\"client\" checked"));
    assert_eq!(compter(&pool, "entreprises").await, 0);
}

#[sqlx::test]
async fn un_profil_ou_une_condition_inconnus_sont_refuses(pool: PgPool) {
    let s = site(&pool);
    let (statut, _, corps) = poster(&s, "/entreprises", "nom=X&profils=roi&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Un des profils choisis n&#39;existe pas."));

    let (statut, _, _) = poster(&s, "/entreprises", "nom=X&conditions_paiement_id=9999&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(compter(&pool, "entreprises").await, 0);
}

#[sqlx::test]
async fn adresse_de_facturation_identique_efface_la_seconde_adresse(pool: PgPool) {
    poster(
        &site(&pool),
        "/entreprises",
        "nom=A&langue=fr&facturation_identique=on&facturation_rue=Ancienne+rue&facturation_ville=Liege",
    )
    .await;
    let (rue, ville): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT facturation_rue, facturation_ville FROM entreprises WHERE nom = 'A'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((rue, ville), (None, None));
}

#[sqlx::test]
async fn adresse_de_facturation_differente_est_gardee(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=B&langue=fr&facturation_rue=Rue+de+Liege&facturation_ville=Liege").await;
    let (identique, rue): (bool, Option<String>) =
        sqlx::query_as("SELECT facturation_identique, facturation_rue FROM entreprises WHERE nom = 'B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!identique);
    assert_eq!(rue.as_deref(), Some("Rue de Liege"));
}

#[sqlx::test]
async fn la_recherche_filtre_par_nom_email_et_tva(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont+SRL&email=info%40dupont.be&langue=fr").await;
    poster(&s, "/entreprises", "nom=Martin+SA&numero_tva=BE0999888777&langue=fr").await;

    let (_, corps) = lire(&s, "/entreprises?q=dup").await;
    assert!(corps.contains("Dupont SRL") && !corps.contains("Martin SA"));
    let (_, corps) = lire(&s, "/entreprises?q=INFO@").await;
    assert!(corps.contains("Dupont SRL") && !corps.contains("Martin SA"));
    let (_, corps) = lire(&s, "/entreprises?q=0999").await;
    assert!(corps.contains("Martin SA") && !corps.contains("Dupont SRL"));
    let (_, corps) = lire(&s, "/entreprises?q=zzz").await;
    assert!(corps.contains("Aucune entreprise trouvée."));
}

#[sqlx::test]
async fn les_jokers_de_la_recherche_sont_neutralises(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&langue=fr").await;
    let (_, corps) = lire(&s, "/entreprises?q=%25").await; // « % » tapé par l'utilisateur
    assert!(corps.contains("Aucune entreprise trouvée."));
    let (_, corps) = lire(&s, "/entreprises?q=D_pont").await;
    assert!(corps.contains("Aucune entreprise trouvée."));
}

#[sqlx::test]
async fn le_filtre_par_profil_fonctionne(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Client+Un&profils=client&langue=fr").await;
    poster(&s, "/entreprises", "nom=Fournisseur+Un&profils=fournisseur_materiaux&langue=fr").await;
    let (_, corps) = lire(&s, "/entreprises?profil=client").await;
    assert!(corps.contains("Client Un") && !corps.contains("Fournisseur Un"));
    // Un profil inconnu est ignoré au lieu de provoquer une erreur.
    let (statut, corps) = lire(&s, "/entreprises?profil=nimporte").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("Client Un") && corps.contains("Fournisseur Un"));
}

#[sqlx::test]
async fn htmx_recoit_seulement_le_tableau(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&langue=fr").await;
    let (_, complet) = lire(&s, "/entreprises").await;
    let (_, partiel) = lire_htmx(&s, "/entreprises").await;
    assert!(complet.contains("<html") && complet.contains("name=\"q\""));
    assert!(!partiel.contains("<html") && !partiel.contains("name=\"q\""));
    assert!(partiel.contains("Dupont"));
}

#[sqlx::test]
async fn pagination_vingt_par_page(pool: PgPool) {
    sqlx::query("INSERT INTO entreprises (nom) SELECT 'Entreprise ' || lpad(n::text, 2, '0') FROM generate_series(1, 25) n")
        .execute(&pool)
        .await
        .unwrap();
    let s = site(&pool);
    let (_, p1) = lire(&s, "/entreprises").await;
    assert!(p1.contains("Entreprise 01") && p1.contains("Entreprise 20") && !p1.contains("Entreprise 21"));
    assert!(p1.contains("25 résultat(s)") && p1.contains("Page 1 / 2"));
    let (_, p2) = lire(&s, "/entreprises?page=2").await;
    assert!(p2.contains("Entreprise 21") && p2.contains("Entreprise 25") && !p2.contains("Entreprise 20"));
    // Une page trop grande ou absurde ramène à une page valide.
    let (statut, p9) = lire(&s, "/entreprises?page=99").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(p9.contains("Page 2 / 2"));
    let (statut, _) = lire(&s, "/entreprises?page=abc").await;
    assert_eq!(statut, StatusCode::OK);
}

#[sqlx::test]
async fn modifier_une_entreprise(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&profils=client&langue=fr").await;
    let (id,): (i64,) = sqlx::query_as("SELECT id FROM entreprises").fetch_one(&pool).await.unwrap();

    let (statut, page) = lire(&s, &format!("/entreprises/{id}/modifier")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Modifier l&#39;entreprise") && page.contains("value=\"Dupont\""));
    assert!(page.contains("value=\"client\" checked"));

    let (statut, lieu, _) =
        poster(&s, &format!("/entreprises/{id}"), "nom=Dupont+et+Fils&profils=architecte&langue=nl").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu.as_deref(), Some("/entreprises"));
    let (nom, profils, langue): (String, Vec<String>, String) =
        sqlx::query_as("SELECT nom, profils, langue FROM entreprises WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((nom.as_str(), profils, langue.as_str()), ("Dupont et Fils", vec!["architecte".to_string()], "nl"));
    assert_eq!(compter(&pool, "entreprises").await, 1);
}

#[sqlx::test]
async fn modifier_avec_une_erreur_ne_change_rien(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&langue=fr").await;
    let (id,): (i64,) = sqlx::query_as("SELECT id FROM entreprises").fetch_one(&pool).await.unwrap();
    let (statut, _, corps) = poster(&s, &format!("/entreprises/{id}"), "nom=&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Le nom est obligatoire."));
    let (nom,): (String,) = sqlx::query_as("SELECT nom FROM entreprises").fetch_one(&pool).await.unwrap();
    assert_eq!(nom, "Dupont");
}

#[sqlx::test]
async fn une_entreprise_inexistante_donne_404(pool: PgPool) {
    let s = site(&pool);
    let (statut, _) = lire(&s, "/entreprises/999/modifier").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
    let (statut, _, _) = poster(&s, "/entreprises/999", "nom=X&langue=fr").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn le_html_tape_par_l_utilisateur_est_neutralise(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=%3Cscript%3Ealert(1)%3C%2Fscript%3E&langue=fr").await;
    let (_, corps) = lire(&s, "/entreprises").await;
    assert!(!corps.contains("<script>alert(1)"));
    assert!(corps.contains("&#60;script&#62;alert(1)"));
}

// ─── Contacts ───────────────────────────────────────────────────────────────

#[sqlx::test]
async fn creer_un_contact_lie_a_une_entreprise(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont+SRL&langue=fr").await;
    let (entreprise,): (i64,) = sqlx::query_as("SELECT id FROM entreprises").fetch_one(&pool).await.unwrap();

    let (_, formulaire) = lire(&s, "/contacts/nouveau").await;
    assert!(formulaire.contains("name=\"prenom\"") && formulaire.contains("name=\"civilite\""));
    assert!(formulaire.contains(&format!("<option value=\"{entreprise}\">Dupont SRL</option>")));

    let (statut, lieu, _) = poster(
        &s,
        "/contacts",
        &format!(
            "civilite=M.&nom=Dupont&prenom=Jean&entreprise_id={entreprise}&profils=client\
             &telephone=0477123456&telephone_secondaire=02+123+45+67&email=jean%40dupont.be&langue=fr"
        ),
    )
    .await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu.as_deref(), Some("/contacts"));

    let (_, corps) = lire(&s, "/contacts").await;
    assert!(corps.contains("Jean Dupont"));
    assert!(corps.contains("Dupont SRL"));
    assert!(corps.contains("0477123456") && corps.contains("02 123 45 67"));
}

#[sqlx::test]
async fn la_recherche_de_contacts_trouve_aussi_par_entreprise_et_nom_complet(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Batiroc+SA&langue=fr").await;
    let (entreprise,): (i64,) = sqlx::query_as("SELECT id FROM entreprises").fetch_one(&pool).await.unwrap();
    poster(&s, "/contacts", &format!("nom=Dupont&prenom=Jean&entreprise_id={entreprise}&langue=fr")).await;
    poster(&s, "/contacts", "nom=Martin&prenom=Anne&langue=fr").await;

    let (_, corps) = lire(&s, "/contacts?q=batiroc").await;
    assert!(corps.contains("Jean Dupont") && !corps.contains("Anne Martin"));
    let (_, corps) = lire(&s, "/contacts?q=jean+dup").await;
    assert!(corps.contains("Jean Dupont") && !corps.contains("Anne Martin"));
}

#[sqlx::test]
async fn un_contact_sans_nom_ou_avec_une_entreprise_inconnue_est_refuse(pool: PgPool) {
    let s = site(&pool);
    let (statut, _, corps) = poster(&s, "/contacts", "prenom=Jean&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Le nom est obligatoire."));
    let (statut, _, corps) = poster(&s, "/contacts", "nom=X&entreprise_id=999&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Cette entreprise n&#39;existe pas."));
    assert_eq!(compter(&pool, "contacts").await, 0);
}

#[sqlx::test]
async fn modifier_un_contact_et_changer_d_entreprise(pool: PgPool) {
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Premiere&langue=fr").await;
    poster(&s, "/entreprises", "nom=Seconde&langue=fr").await;
    let (a,): (i64,) = sqlx::query_as("SELECT id FROM entreprises WHERE nom = 'Premiere'").fetch_one(&pool).await.unwrap();
    let (b,): (i64,) = sqlx::query_as("SELECT id FROM entreprises WHERE nom = 'Seconde'").fetch_one(&pool).await.unwrap();
    poster(&s, "/contacts", &format!("nom=Dupont&prenom=Jean&entreprise_id={a}&langue=fr")).await;
    let (id,): (i64,) = sqlx::query_as("SELECT id FROM contacts").fetch_one(&pool).await.unwrap();

    let (_, page) = lire(&s, &format!("/contacts/{id}/modifier")).await;
    assert!(page.contains("Modifier le contact"));
    assert!(page.contains(&format!("<option value=\"{a}\" selected>Premiere</option>")));

    let (statut, _, _) =
        poster(&s, &format!("/contacts/{id}"), &format!("nom=Dupont&prenom=Jean-Paul&entreprise_id={b}&langue=fr")).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (prenom, entreprise): (String, i64) =
        sqlx::query_as("SELECT prenom, entreprise_id FROM contacts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((prenom.as_str(), entreprise), ("Jean-Paul", b));
}

#[sqlx::test]
async fn un_contact_inexistant_donne_404(pool: PgPool) {
    let (statut, _) = lire(&site(&pool), "/contacts/999/modifier").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}
