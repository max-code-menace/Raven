// Tests de l'éditeur de devis, onglet Informations.

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
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    let lieu = r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
    let octets = r.into_body().collect().await.unwrap().to_bytes();
    (statut, lieu, String::from_utf8(octets.to_vec()).unwrap())
}

async fn lire(site: &Router, uri: &str) -> (StatusCode, String) {
    let (statut, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    (statut, corps)
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

async fn nouveau_brouillon(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO devis (numero, date_devis) VALUES (1, '2026-10-01') RETURNING id").fetch_one(pool).await.unwrap()
}

async fn entreprise(pool: &PgPool, nom: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ($1) RETURNING id").bind(nom).fetch_one(pool).await.unwrap()
}

async fn contact(pool: &PgPool, prenom: &str, nom: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO contacts (prenom, nom) VALUES ($1, $2) RETURNING id")
        .bind(prenom)
        .bind(nom)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn taux_reduit(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 600").fetch_one(pool).await.unwrap()
}

async fn sept_jours(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Un formulaire complet et valide ; `supplements` s'ajoute à la fin (le dernier champ répété gagne
/// pour la plupart des cas, donc les tests passent plutôt par `sans` pour retirer un champ).
fn formulaire(client: &str, tva: i64, conditions: i64) -> String {
    format!(
        "reference=C44+Volet&responsable=Marc&date_devis=2026-10-15&client={client}&taux_tva_id={tva}\
         &conditions_paiement_id={conditions}&facturation_identique=on&chantier_identique=on\
         &chantier_pays=BE&remarque_1=Acompte+de+30+%25&remarque_2="
    )
}

#[sqlx::test]
async fn un_devis_inconnu_est_introuvable(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    assert_eq!(lire(&s, "/devis/999").await.0, StatusCode::NOT_FOUND);
    assert_eq!(poster(&s, "/devis/999", "reference=x").await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn la_page_montre_le_numero_et_le_statut(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    entreprise(&pool, "Dupont SRL").await;
    let (statut, page) = lire(&site(&pool), &format!("/devis/{id}")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Devis n° 20") && page.contains("-1</h1>"));
    assert!(page.contains("Brouillon"));
    assert!(page.contains("Dupont SRL (entreprise)"));
    // Plus de bouton « Enregistrer » : le formulaire s'envoie tout seul et la barre du bas le dit.
    assert!(page.contains(&format!("hx-post=\"/devis/{id}\"")) && page.contains("Les modifications s'enregistrent automatiquement."));
    assert!(!page.contains(">Enregistrer<"));
    assert!(!page.contains("<fieldset class=\"flex flex-col gap-6\" disabled"));
    // Les trois taux et les dix conditions de départ sont proposés.
    assert!(page.contains("Taux réduit (6,00 %)") && page.contains("Paiement comptant"));
}

#[sqlx::test]
async fn enregistrer_met_a_jour_le_brouillon(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "Dupont SRL").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    let s = site(&pool);

    let (statut, lieu, _) = poster(&s, &format!("/devis/{id}"), &formulaire(&format!("e{e}"), tva, cond)).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    assert_eq!(lieu, Some(format!("/devis/{id}?ok=1")));

    let (reference, responsable, date, client, t, c, remarque): (String, String, String, Option<i64>, Option<i64>, Option<i64>, String) =
        sqlx::query_as(
            "SELECT reference, responsable, date_devis::text, client_entreprise_id, taux_tva_id,
                    conditions_paiement_id, remarque_1 FROM devis WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((reference.as_str(), responsable.as_str(), date.as_str()), ("C44 Volet", "Marc", "2026-10-15"));
    assert_eq!((client, t, c), (Some(e), Some(tva), Some(cond)));
    assert_eq!(remarque, "Acompte de 30 %");

    // La page rechargée montre ce qui a été enregistré, avec la confirmation.
    let (_, page) = lire(&s, &format!("/devis/{id}?ok=1")).await;
    assert!(page.contains("Les informations sont enregistrées."));
    assert!(page.contains("value=\"C44 Volet\"") && page.contains("value=\"Marc\""));
    assert!(page.contains(&format!("value=\"e{e}\" selected")));
    assert!(page.contains("value=\"2026-10-15\""));
}

#[sqlx::test]
async fn un_contact_peut_etre_le_client(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let c = contact(&pool, "Anne", "Lambert").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    let (statut, _, _) = poster(&site(&pool), &format!("/devis/{id}"), &formulaire(&format!("c{c}"), tva, cond)).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (e, k): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT client_entreprise_id, client_contact_id FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((e, k), (None, Some(c)));
}

#[sqlx::test]
async fn un_brouillon_se_remplit_petit_a_petit_et_une_valeur_refusee_garde_l_ancienne(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let s = site(&pool);
    let lire_devis = || async {
        sqlx::query_as::<_, (String, String, Option<i64>, Option<i64>)>(
            "SELECT reference, date_devis::text, client_entreprise_id, taux_tva_id FROM devis WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // Enregistrement automatique : la référence est gardée alors que le client, le taux et les conditions manquent encore
    // (ils sont rappelés avant l'envoi, onglet Aperçu).
    let (statut, lieu, _) =
        poster(&s, &format!("/devis/{id}"), "reference=C44+Volet&responsable=&date_devis=2026-10-15&facturation_identique=on&chantier_identique=on").await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/devis/{id}?ok=1"))));
    assert_eq!(lire_devis().await, ("C44 Volet".to_string(), "2026-10-15".to_string(), None, None));

    // Une date illisible n'est pas enregistrée : l'ancienne reste, avec un message ; le reste est enregistré.
    let (statut, _, page) =
        poster(&s, &format!("/devis/{id}"), "reference=C45&date_devis=pas-une-date&facturation_identique=on&chantier_identique=on").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("La date du devis est obligatoire."));
    assert!(!page.contains("Le responsable est obligatoire.") && !page.contains("La référence est obligatoire."));
    assert_eq!(lire_devis().await.0, "C45");
    assert_eq!(lire_devis().await.1, "2026-10-15");

    // Une date d'une autre année non plus : le numéro « 2026-1 » appartient à 2026.
    let (statut, _, page) = poster(&s, &format!("/devis/{id}"), "reference=C45&date_devis=2027-01-04&facturation_identique=on&chantier_identique=on").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("La date doit rester en 2026"));
    assert_eq!(lire_devis().await.1, "2026-10-15");

    // Un client choisi puis un client inconnu : le premier reste.
    let e = entreprise(&pool, "Dupont SRL").await;
    poster(&s, &format!("/devis/{id}"), &format!("reference=C45&date_devis=2026-10-15&client=e{e}&facturation_identique=on&chantier_identique=on")).await;
    let (statut, _, page) =
        poster(&s, &format!("/devis/{id}"), "reference=C45&date_devis=2026-10-15&client=e999999&facturation_identique=on&chantier_identique=on").await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("Choisissez le client du devis."));
    assert_eq!(lire_devis().await.2, Some(e));
}

/// Comme le navigateur avec htmx : l'enregistrement automatique.
async fn poster_htmx(site: &Router, uri: &str, corps: &str) -> (StatusCode, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("HX-Request", "true")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let (statut, _, page) = envoyer(site, requete).await;
    (statut, page)
}

/// Le morceau de page qui commence à `debut` et finit à la première `fin` qui suit.
fn morceau<'a>(page: &'a str, debut: &str, fin: &str) -> &'a str {
    let i = page.find(debut).unwrap_or_else(|| panic!("{debut} absent"));
    let reste = &page[i..];
    &reste[..reste.find(fin).map(|j| j + fin.len()).unwrap_or(reste.len())]
}

#[sqlx::test]
async fn l_enregistrement_automatique_repond_par_la_note_et_l_en_tete(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "Dupont SRL").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    let s = site(&pool);

    // Un gestionnaire inconnu : pas enregistré, le reste l'est. Réponse 200 (htmx ne remplace rien sur une erreur).
    let (statut, page) = poster_htmx(&s, &format!("/devis/{id}"), &format!("{}&gestionnaire_id=9999", formulaire(&format!("e{e}"), tva, cond))).await;
    assert_eq!(statut, StatusCode::OK);
    let note = morceau(&page, "id=\"etat-enregistrement\"", "</span>\n");
    assert!(note.contains("Pas enregistré : Ce gestionnaire n&#39;existe pas.") && note.contains("Le reste est enregistré."), "{note}");
    // L'en-tête renvoyé (que htmx remplace aussi) porte déjà le client et la référence enregistrés.
    let tete = morceau(&page, "id=\"devis-tete\"", "</header>");
    assert!(tete.contains("Dupont SRL") && tete.contains("C44 Volet"), "{tete}");
    // Pas de bandeau d'erreur en haut de page pendant qu'on tape (il décalerait le formulaire).
    assert!(!page.contains("role=\"alert\""));
    let (reference, client, gestionnaire): (String, Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT reference, client_entreprise_id, gestionnaire_contact_id FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((reference.as_str(), client, gestionnaire), ("C44 Volet", Some(e), None));

    // Corrigé : la note donne l'heure de l'enregistrement.
    let (statut, page) = poster_htmx(&s, &format!("/devis/{id}"), &formulaire(&format!("e{e}"), tva, cond)).await;
    assert_eq!(statut, StatusCode::OK);
    let note = morceau(&page, "id=\"etat-enregistrement\"", "</span>\n");
    assert!(note.contains("Enregistré à ") && !note.contains("Pas enregistré"), "{note}");

    // Un devis envoyé ne s'enregistre plus, même tout seul.
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(id).execute(&pool).await.unwrap();
    assert_eq!(poster_htmx(&s, &format!("/devis/{id}"), &formulaire(&format!("e{e}"), tva, cond)).await.0, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn les_valeurs_tapees_reviennent_apres_une_erreur(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let (_, _, page) = poster(&site(&pool), &format!("/devis/{id}"), "reference=Garde+moi&responsable=Marc").await;
    assert!(page.contains("value=\"Garde moi\"") && page.contains("value=\"Marc\""));
}

#[sqlx::test]
async fn un_client_ou_un_taux_inexistant_est_refuse(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let s = site(&pool);
    let (statut, _, page) = poster(&s, &format!("/devis/{id}"), &formulaire("e9999", 9999, 9999)).await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("Choisissez le client du devis.") && page.contains("Choisissez le taux de TVA par défaut dans la liste."));
    // Un code mal écrit ne provoque pas d'erreur serveur.
    let (statut, _, _) = poster(&s, &format!("/devis/{id}"), &formulaire("n'importe quoi", 1, 1)).await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test]
async fn un_autre_client_de_facturation_est_exige_si_la_case_est_decochee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "Dupont SRL").await;
    let assurance = entreprise(&pool, "Assurance Belfius").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    let s = site(&pool);
    let sans_case = formulaire(&format!("e{e}"), tva, cond).replace("&facturation_identique=on", "");

    let (statut, _, page) = poster(&s, &format!("/devis/{id}"), &sans_case).await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.contains("Choisissez le client de facturation"));

    let (statut, _, _) = poster(&s, &format!("/devis/{id}"), &format!("{sans_case}&facturation_client=e{assurance}")).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (identique, fe): (bool, Option<i64>) =
        sqlx::query_as("SELECT facturation_identique, facturation_entreprise_id FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((identique, fe), (false, Some(assurance)));

    // En recochant « identique », l'autre client est oublié même s'il est encore envoyé.
    let (statut, _, _) = poster(&s, &format!("/devis/{id}"), &format!("{}&facturation_client=e{assurance}", formulaire(&format!("e{e}"), tva, cond))).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (identique, fe): (bool, Option<i64>) =
        sqlx::query_as("SELECT facturation_identique, facturation_entreprise_id FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((identique, fe), (true, None));
}

#[sqlx::test]
async fn l_adresse_du_chantier_n_est_gardee_que_si_elle_est_propre_au_devis(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "Dupont SRL").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    let s = site(&pool);
    let adresse = "&chantier_rue=Rue+Neuve&chantier_numero=12&chantier_code_postal=1000&chantier_ville=Bruxelles";

    // Case cochée : l'adresse envoyée est ignorée.
    poster(&s, &format!("/devis/{id}"), &format!("{}{adresse}", formulaire(&format!("e{e}"), tva, cond))).await;
    let (rue,): (String,) = sqlx::query_as("SELECT chantier_rue FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(rue, "");

    // Case décochée : l'adresse est enregistrée et réaffichée.
    let sans_case = formulaire(&format!("e{e}"), tva, cond).replace("&chantier_identique=on", "");
    let (statut, _, _) = poster(&s, &format!("/devis/{id}"), &format!("{sans_case}{adresse}")).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (_, page) = lire(&s, &format!("/devis/{id}")).await;
    assert!(page.contains("value=\"Rue Neuve\"") && page.contains("value=\"Bruxelles\""));
}

#[sqlx::test]
async fn architecte_et_gestionnaire_sont_facultatifs_mais_doivent_exister(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "Dupont SRL").await;
    let archi = contact(&pool, "Paul", "Architecte").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    let base = formulaire(&format!("e{e}"), tva, cond);
    let s = site(&pool);

    let (statut, _, _) = poster(&s, &format!("/devis/{id}"), &format!("{base}&architecte_id={archi}")).await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let (a, g): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT architecte_contact_id, gestionnaire_contact_id FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((a, g), (Some(archi), None));

    let (statut, _, page) = poster(&s, &format!("/devis/{id}"), &format!("{base}&gestionnaire_id=9999")).await;
    assert_eq!(statut, StatusCode::UNPROCESSABLE_ENTITY);
    // (Askama écrit l'apostrophe d'un message venu du serveur sous la forme « &#39; ».)
    assert!(page.contains("Ce gestionnaire n&#39;existe pas."));
}

#[sqlx::test]
async fn un_devis_envoye_n_est_plus_modifiable(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "Dupont SRL").await;
    let (tva, cond) = (taux_reduit(&pool).await, sept_jours(&pool).await);
    sqlx::query("UPDATE devis SET statut = 'envoye', reference = 'ORIGINAL' WHERE id = $1").bind(id).execute(&pool).await.unwrap();
    let s = site(&pool);

    let (statut, _, _) = poster(&s, &format!("/devis/{id}"), &formulaire(&format!("e{e}"), tva, cond)).await;
    assert_eq!(statut, StatusCode::CONFLICT);
    let (reference,): (String,) = sqlx::query_as("SELECT reference FROM devis WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(reference, "ORIGINAL");

    let (statut, page) = lire(&s, &format!("/devis/{id}")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(page.contains("Ce devis n'est plus un brouillon") && page.contains(" disabled>"));
    assert!(!page.contains("Enregistrer") && page.contains("Envoyé"));
}

#[sqlx::test]
async fn les_textes_tapes_sont_neutralises(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let (_, _, page) = poster(&site(&pool), &format!("/devis/{id}"), "reference=%3Cscript%3Ealert(1)%3C%2Fscript%3E&remarque_1=%3Cb%3Egras%3C%2Fb%3E").await;
    assert!(!page.contains("<script>alert(1)") && !page.contains("<b>gras"));
}

#[sqlx::test]
async fn deux_clients_de_facturation_sont_refuses_par_la_base(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let id = nouveau_brouillon(&pool).await;
    let e = entreprise(&pool, "E").await;
    let c = contact(&pool, "C", "D").await;
    let res = sqlx::query(
        "UPDATE devis SET facturation_identique = false, facturation_entreprise_id = $1, facturation_contact_id = $2 WHERE id = $3",
    )
    .bind(e)
    .bind(c)
    .bind(id)
    .execute(&pool)
    .await;
    assert!(res.is_err());
    let res = sqlx::query("UPDATE devis SET facturation_entreprise_id = $1 WHERE id = $2").bind(e).bind(id).execute(&pool).await;
    assert!(res.is_err(), "« identique » interdit un autre client de facturation");
}

#[sqlx::test]
async fn la_liste_mene_a_l_editeur_et_nouveau_y_arrive(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (statut, lieu, _) = poster(&s, "/devis", "").await;
    assert_eq!(statut, StatusCode::SEE_OTHER);
    let id: i64 = sqlx::query_scalar("SELECT id FROM devis").fetch_one(&pool).await.unwrap();
    assert_eq!(lieu, Some(format!("/devis/{id}")));
    let (_, liste) = lire(&s, "/devis").await;
    assert!(liste.contains(&format!("href=\"/devis/{id}\"")));
}
