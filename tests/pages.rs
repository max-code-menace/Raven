// Tests des écrans : chaque test reçoit une base temporaire et appelle le site comme un navigateur.

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
async fn la_page_d_accueil_s_affiche(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, corps) = lire(&site(&pool), "/").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("RAVEN") && !corps.contains("Ubifacture"));
    assert!(corps.contains("href=\"/entreprises\""));
    assert!(lire(&site(&pool), "/entreprises").await.1.contains("href=\"/contacts\""), "Contacts › Contacts");
}

#[sqlx::test]
async fn la_route_sante_interroge_la_base(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, corps) = lire(&site(&pool), "/sante").await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(corps, "Le serveur et la base répondent.");
}

#[sqlx::test]
async fn une_page_inconnue_donne_404(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, _) = lire(&site(&pool), "/nimporte-quoi").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

// ─── Entreprises ────────────────────────────────────────────────────────────

#[sqlx::test]
async fn liste_vide_au_depart(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, corps) = lire(&site(&pool), "/entreprises").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("Aucune entreprise trouvée."));
    assert!(corps.contains("0\u{a0}résultat"));
}

#[sqlx::test]
async fn le_formulaire_de_creation_s_affiche(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, corps) = lire(&site(&pool), "/entreprises/nouveau").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(corps.contains("Nouvelle entreprise"));
    assert!(corps.contains("name=\"profils\""));
    assert!(corps.contains("Taux réduit (6,00 %)"));
    assert!(corps.contains("10\u{a0}jours fin du mois"));
    assert!(corps.contains("Paiement comptant"));
    // Une entreprise n'a ni prénom ni civilité.
    assert!(!corps.contains("name=\"prenom\""));
}

#[sqlx::test]
async fn creer_une_entreprise_avec_plusieurs_profils(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
    assert!(corps.contains("1\u{a0}résultat"));
}

#[sqlx::test]
async fn un_nom_vide_est_refuse_et_rien_n_est_enregistre(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, _, corps) = poster(&site(&pool), "/entreprises", "nom=+&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Le nom est obligatoire."));
    assert_eq!(compter(&pool, "entreprises").await, 0);
}

#[sqlx::test]
async fn les_erreurs_gardent_ce_qui_a_ete_tape(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn un_profil_ou_une_condition_inconnus_sont_refuses(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (statut, _, corps) = poster(&s, "/entreprises", "nom=X&profils=roi&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(corps.contains("Un des profils choisis n&#39;existe pas."));

    let (statut, _, _) = poster(&s, "/entreprises", "nom=X&conditions_paiement_id=9999&langue=fr").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(compter(&pool, "entreprises").await, 0);
}

#[sqlx::test]
async fn adresse_de_facturation_identique_efface_la_seconde_adresse(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn adresse_de_facturation_differente_est_gardee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn la_recherche_filtre_par_nom_email_et_tva(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn les_jokers_de_la_recherche_sont_neutralises(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&langue=fr").await;
    let (_, corps) = lire(&s, "/entreprises?q=%25").await; // « % » tapé par l'utilisateur
    assert!(corps.contains("Aucune entreprise trouvée."));
    let (_, corps) = lire(&s, "/entreprises?q=D_pont").await;
    assert!(corps.contains("Aucune entreprise trouvée."));
}

#[sqlx::test]
async fn le_filtre_par_profil_fonctionne(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn htmx_recoit_seulement_le_tableau(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&langue=fr").await;
    let (_, complet) = lire(&s, "/entreprises").await;
    let (_, partiel) = lire_htmx(&s, "/entreprises").await;
    assert!(complet.contains("<html") && complet.contains("name=\"q\""));
    assert!(!partiel.contains("<html") && !partiel.contains("name=\"q\""));
    assert!(partiel.contains("Dupont"));
}

#[sqlx::test]
async fn pagination_vingt_par_page(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO entreprises (nom) SELECT 'Entreprise ' || lpad(n::text, 2, '0') FROM generate_series(1, 25) n")
        .execute(&pool)
        .await
        .unwrap();
    let s = site(&pool);
    let (_, p1) = lire(&s, "/entreprises").await;
    assert!(p1.contains("Entreprise 01") && p1.contains("Entreprise 20") && !p1.contains("Entreprise 21"));
    assert!(p1.contains("25\u{a0}résultats") && p1.contains("Page 1 / 2"));
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
async fn modifier_une_entreprise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Dupont&profils=client&langue=fr").await;
    let (id,): (i64,) = sqlx::query_as("SELECT id FROM entreprises").fetch_one(&pool).await.unwrap();

    let (statut, page) = lire(&s, &format!("/entreprises/{id}/modifier")).await;
    assert_eq!(statut, StatusCode::OK);
    // Le titre d'une fiche existante est son nom ; elle s'enregistre toute seule (enregistrement automatique).
    assert!(page.contains("<h1 id=\"fiche-titre\">Dupont</h1>") && page.contains("value=\"Dupont\""), "{page}");
    assert!(page.contains(&format!("hx-post=\"/entreprises/{id}\"")) && !page.contains(">Enregistrer</button>"));
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
async fn modifier_avec_une_erreur_ne_change_rien(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn une_entreprise_inexistante_donne_404(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (statut, _) = lire(&s, "/entreprises/999/modifier").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
    let (statut, _, _) = poster(&s, "/entreprises/999", "nom=X&langue=fr").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn le_html_tape_par_l_utilisateur_est_neutralise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=%3Cscript%3Ealert(1)%3C%2Fscript%3E&langue=fr").await;
    let (_, corps) = lire(&s, "/entreprises").await;
    assert!(!corps.contains("<script>alert(1)"));
    assert!(corps.contains("&#60;script&#62;alert(1)"));
}

// ─── Contacts ───────────────────────────────────────────────────────────────

#[sqlx::test]
async fn creer_un_contact_lie_a_une_entreprise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn la_recherche_de_contacts_trouve_aussi_par_entreprise_et_nom_complet(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn un_contact_sans_nom_ou_avec_une_entreprise_inconnue_est_refuse(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
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
async fn modifier_un_contact_et_changer_d_entreprise(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    poster(&s, "/entreprises", "nom=Premiere&langue=fr").await;
    poster(&s, "/entreprises", "nom=Seconde&langue=fr").await;
    let (a,): (i64,) = sqlx::query_as("SELECT id FROM entreprises WHERE nom = 'Premiere'").fetch_one(&pool).await.unwrap();
    let (b,): (i64,) = sqlx::query_as("SELECT id FROM entreprises WHERE nom = 'Seconde'").fetch_one(&pool).await.unwrap();
    poster(&s, "/contacts", &format!("nom=Dupont&prenom=Jean&entreprise_id={a}&langue=fr")).await;
    let (id,): (i64,) = sqlx::query_as("SELECT id FROM contacts").fetch_one(&pool).await.unwrap();

    let (_, page) = lire(&s, &format!("/contacts/{id}/modifier")).await;
    assert!(page.contains("<h1 id=\"fiche-titre\">Jean Dupont</h1>"), "{page}");
    assert!(page.contains(&format!("<option value=\"{a}\" selected>Premiere</option>")));

    let (statut, _, _) =
        poster(&s, &format!("/contacts/{id}"), &format!("nom=Dupont&prenom=Jean-Paul&entreprise_id={b}&langue=fr")).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (prenom, entreprise): (String, i64) =
        sqlx::query_as("SELECT prenom, entreprise_id FROM contacts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((prenom.as_str(), entreprise), ("Jean-Paul", b));
}

#[sqlx::test]
async fn un_contact_inexistant_donne_404(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, _) = lire(&site(&pool), "/contacts/999/modifier").await;
    assert_eq!(statut, StatusCode::NOT_FOUND);
}

// Refonte du design, étape 1 : le menu à huit espaces (Tableau de bord en premier), Données et le compte ; chaque entrée mène à un écran qui répond.
#[sqlx::test]
async fn le_menu_a_huit_espaces_qui_menent_tous_a_un_ecran(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (statut, page) = lire(&s, "/").await;
    assert_eq!(statut, StatusCode::OK);
    let mut avant = 0;
    for libelle in ["Tableau de bord", "Devis", "Factures", "Paiements", "Chantiers", "Contacts", "Bibliothèque", "Statistiques", "Importer", "Exporter", "Réglages", "Se déconnecter"] {
        let position = page.find(&format!("<span class=\"rv-nav-txt\">{libelle}</span>")).unwrap_or_else(|| panic!("{libelle} absent du menu"));
        assert!(position > avant, "{libelle} n'est pas à sa place");
        avant = position;
    }
    assert!(page.contains("rv-eyebrow rv-side-groupe\">Données") && !page.contains("rv-side rv-side--rail"));
    // L'espace de l'écran est annoncé aux lecteurs d'écran, et lui seul.
    assert!(page.contains("<a class=\"rv-nav\" aria-current=\"page\" href=\"/\"") && page.matches("aria-current=\"page\" href").count() == 1);
    let (_, devis) = lire(&s, "/devis").await;
    assert!(devis.contains("<a class=\"rv-nav\" aria-current=\"page\" href=\"/devis\""));
    for adresse in ["/", "/devis", "/factures", "/factures/rappels", "/chantiers", "/entreprises", "/bibliotheque", "/statistiques", "/donnees/importer", "/exports", "/parametres/societe"] {
        assert!(page.contains(&format!("href=\"{adresse}\"")), "{adresse} absent du menu");
        assert_eq!(lire(&s, adresse).await.0, StatusCode::OK, "{adresse}");
    }

    // Données › Importer rassemble les imports qui existent.
    let (_, page) = lire(&s, "/donnees/importer").await;
    for adresse in ["/entreprises/import", "/contacts/import", "/bibliotheque/import"] {
        assert!(page.contains(&format!("href=\"{adresse}\"")), "{adresse}");
    }
    assert!(page.contains("action=\"/devis/import\""), "l'import de métrés est allumé pour l'organisation n° 1");

    // Les espaces regroupent les anciens écrans par des onglets.
    let (_, page) = lire(&s, "/factures/recurrences").await;
    assert!(page.contains("href=\"/factures?genre=note_credit\">Notes de crédit") && page.contains("is-on\" href=\"/factures/recurrences\""));
    let (_, page) = lire(&s, "/factures/rappels").await;
    assert!(page.contains("is-on\" href=\"/factures/rappels\" aria-current=\"page\">À recevoir") && page.contains("href=\"/achats/recues?vue=a_payer\">À payer"));
    let (_, page) = lire(&s, "/parametres/emails").await;
    assert!(page.contains("href=\"/parametres/peppol\">Peppol") && page.contains("is-on\" href=\"/parametres/emails\""));
    let (_, page) = lire(&s, "/contacts").await;
    assert!(page.contains("<h1>Contacts</h1>") && page.contains("href=\"/entreprises\">Entreprises"));

    // L'éditeur de devis replie le menu en rail.
    let (_, lieu, _) = poster(&s, "/devis", "").await;
    let (statut, page) = lire(&s, &lieu.unwrap()).await;
    assert!(statut == StatusCode::OK && page.contains("rv-side rv-side--rail"));
}

// Refonte du design, étape 2 : chaque composant partagé (_ui.html) s'affiche sur la page de démonstration.
#[sqlx::test]
async fn la_page_des_composants_montre_chaque_composant(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (statut, page) = lire(&site(&pool), "/composants").await;
    assert_eq!(statut, StatusCode::OK);
    // Boutons : les quatre genres, un lien, un bouton carré, un bouton inactif avec sa raison.
    for attendu in [
        "class=\"rv-btn rv-btn--acc\"",
        "class=\"rv-btn\" type=\"submit\"",
        "class=\"rv-btn rv-btn--ink\"",
        "class=\"rv-btn rv-btn--ghost rv-btn--sm\"",
        "<a class=\"rv-btn\" href=\"/composants\">",
        "rv-btn--icon\" type=\"submit\" aria-label=\"Supprimer le brouillon\"",
        "disabled>Valider la facture</button><span class=\"rv-raison\">Choisissez d&#39;abord un client.</span>",
    ] {
        assert!(page.contains(attendu), "bouton : {attendu}");
    }
    // Pastilles : les statuts du code, avec leur mot.
    for attendu in [
        "<span class=\"rv-pill rv-pill--draft\">Brouillon</span>",
        "<span class=\"rv-pill\">Envoyé</span>",
        "<span class=\"rv-pill rv-pill--done\">Accepté</span>",
        "<span class=\"rv-pill rv-pill--refused\">Refusé</span>",
        "<span class=\"rv-pill rv-pill--late\">! Échec envoi</span>",
        "<span class=\"rv-pill rv-pill--late\">! En retard</span>",
        "<span class=\"rv-pill rv-pill--done\">Payée</span>",
        "<span class=\"rv-pill\">Partiellement payée</span>",
        "<span class=\"rv-pill rv-pill--refused\">Créditée</span>",
        "<span class=\"rv-pill\">En cours</span>",
        "<span class=\"rv-pill rv-pill--done\">Terminé</span>",
    ] {
        assert!(page.contains(attendu), "pastille : {attendu}");
    }
    // Champs : intitulé relié, aide, montant à droite, erreur annoncée, liste, densité éditeur.
    assert!(page.contains("<label class=\"rv-label\" for=\"demo-titre\">") && page.contains("<span class=\"rv-hint\" id=\"demo-titre-aide\">"));
    assert!(page.contains("class=\"rv-input rv-input--num\" id=\"demo-acompte\" type=\"text\" inputmode=\"decimal\""));
    assert!(page.contains("class=\"rv-input is-error\" id=\"demo-tva\"") && page.contains("aria-invalid=\"true\"") && page.contains("! Le numéro doit commencer par BE0 ou BE1."));
    assert!(page.contains("<option value=\"2100\" selected>21 %</option>"));
    assert!(page.contains("rv-input rv-input--num rv-input--sm\" id=\"demo-qte\""));
    // Tableau : colonne de nombres à droite, montant écrit à la belge, ligne vide qui dit quoi faire.
    assert!(page.contains("<th scope=\"col\" class=\"rv-r\">Montant TTC</th>"));
    assert!(page.contains("<td class=\"rv-r rv-num\">7\u{a0}364,75\u{a0}€</td>"));
    assert!(page.contains("<tr class=\"rv-vide\"><td colspan=\"4\">Aucun devis envoyé."));
    // Bandeaux : trois tons, avec leur action.
    assert!(page.contains("<div class=\"rv-banner\" role=\"status\">") && page.contains("rv-banner rv-banner--bad\" role=\"alert\"") && page.contains("rv-banner rv-banner--ok\""));
    assert!(page.contains("<a class=\"rv-btn rv-btn--sm\" href=\"/composants\">Voir les rappels</a>"));
    // Onglets : l'actif est annoncé, le compteur en petit.
    assert!(page.contains("<a class=\"rv-tab is-on\" href=\"/composants\" aria-current=\"page\">Tous<small>8</small></a>"));
    // Tuiles : une seule mise en avant, cliquable ou non.
    assert_eq!(page.matches("rv-kpi rv-kpi--plate").count(), 1);
    assert!(page.contains("<div class=\"rv-kpi\"><small>Contrat HT</small><b>13\u{a0}246,43\u{a0}€</b>"));
    assert!(page.contains("<a class=\"rv-kpi\" href=\"/composants\"><small>Facturé HT</small>"));
    // Totaux : Total HT, base et TVA par taux, Total TTC ; la remise avant le Total HT.
    let ordre = ["Total HT", "Base 6 %", "TVA 6 %", "Base 21 %", "TVA 21 %", "Total TTC"];
    let mut avant = 0;
    for intitule in ordre {
        let position = page[avant..].find(&format!("<span>{intitule}</span>")).unwrap_or_else(|| panic!("{intitule}")) + avant;
        avant = position;
    }
    assert!(page.contains("<span>Remise fidélité</span><span>-500,00\u{a0}€</span>"));
    assert!(page.contains("<div class=\"rv-totals-hors\">Taux de 6 % : le client fait sa déclaration sur la facture.</div>"));
}
