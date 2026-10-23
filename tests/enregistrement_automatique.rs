// Enregistrement automatique des derniers formulaires qui avaient un bouton « Enregistrer » (23/10/2026, même système
// que les informations du devis et de la facture) : ajustement de l'onglet Totaux, fiche chantier, en-tête d'un
// avancement, Réglages › Société et Réglages › E-mails (modèles, délais des rappels, mentions).
// Chaque valeur valable est gardée tout de suite ; une valeur refusée garde la précédente, avec un message.

mod commun;
#[path = "commun/donnees.rs"]
mod donnees;

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

async fn lire(site: &Router, uri: &str) -> String {
    let r = site.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{uri}");
    String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

/// Ce que htmx envoie quand le formulaire part tout seul.
async fn poster_htmx(site: &Router, uri: &str, corps: &str) -> (StatusCode, String) {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    let statut = r.status();
    (statut, String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap())
}

/// Le morceau de page entre `debut` et le `fin` qui suit.
fn morceau<'a>(page: &'a str, debut: &str, fin: &str) -> &'a str {
    let i = page.find(debut).unwrap_or_else(|| panic!("{debut} absent"));
    let reste = &page[i..];
    &reste[..reste.find(fin).map(|j| j + fin.len()).unwrap_or(reste.len())]
}

async fn aujourdhui(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn ajustement_fiche_chantier_et_avancement(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let d = donnees::remplir(&pool, &s).await;
    let devis = d.devis[6];

    // L'ajustement du devis brouillon : plus de bouton « Enregistrer l'ajustement », la note dans la barre du bas.
    let page = lire(&s, &format!("/devis/{devis}/totaux")).await;
    assert!(page.contains(&format!("hx-post=\"/devis/{devis}/ajustement\"")) && !page.contains("Enregistrer l'ajustement"), "{page}");
    assert!(page.contains("Les modifications s'enregistrent automatiquement."));
    let ajustement = |pool: PgPool| async move {
        sqlx::query_as::<_, (String, String, i64)>("SELECT ajustement_libelle, ajustement_mode, ajustement_valeur FROM devis WHERE id = $1")
            .bind(devis)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let (statut, page) = poster_htmx(&s, &format!("/devis/{devis}/ajustement"), "libelle=Remise+fid%C3%A9lit%C3%A9&sens=remise&mode=pourcentage&valeur=5").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(morceau(&page, "id=\"etat-enregistrement\"", "</span>").contains("Enregistré à "), "{page}");
    assert!(morceau(&page, "id=\"totaux-devis\"", "Total TTC à payer").contains("Remise fidélité"), "les totaux sont recalculés");
    assert!(morceau(&page, "id=\"ajustement-actions\"", "</div>").contains("Retirer l'ajustement"));
    assert_eq!(ajustement(pool.clone()).await, ("Remise fidélité".to_string(), "pourcentage".to_string(), -500));
    // Une valeur refusée garde l'ajustement d'avant ; le libellé, lui, est gardé.
    let (statut, page) = poster_htmx(&s, &format!("/devis/{devis}/ajustement"), "libelle=Geste&sens=remise&mode=pourcentage&valeur=150").await;
    assert_eq!(statut, StatusCode::OK);
    let note = morceau(&page, "id=\"etat-enregistrement\"", "</span></span>");
    assert!(note.contains("Pas enregistré") && note.contains("100 %"), "{note}");
    assert_eq!(ajustement(pool.clone()).await, ("Geste".to_string(), "pourcentage".to_string(), -500));
    // Une valeur vide retire l'ajustement.
    poster_htmx(&s, &format!("/devis/{devis}/ajustement"), "libelle=Geste&sens=remise&mode=pourcentage&valeur=").await;
    assert_eq!(ajustement(pool.clone()).await.1, "aucun");

    // La fiche chantier : le statut et la date de début partent tout seuls ; la pastille de l'en-tête suit.
    let chantier = d.chantier;
    let page = lire(&s, &format!("/chantiers/{chantier}")).await;
    assert!(page.contains(&format!("hx-post=\"/chantiers/{chantier}\"")) && page.contains("id=\"etat-chantier\""));
    let (statut, page) = poster_htmx(&s, &format!("/chantiers/{chantier}"), "statut=termine&date_debut=2026-09-01").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(morceau(&page, "id=\"etat-chantier\"", "</span>").contains("Enregistré à "));
    assert!(morceau(&page, "id=\"chantier-statut\"", "</div>").contains("Terminé"));
    // Une date effacée est refusée : la date d'avant reste, le statut choisi est gardé.
    let (_, page) = poster_htmx(&s, &format!("/chantiers/{chantier}"), "statut=en_cours&date_debut=").await;
    assert!(morceau(&page, "id=\"etat-chantier\"", "</span></span>").contains("La date de début n&#39;est pas valide."), "{page}");
    let (statut_chantier, debut): (String, String) =
        sqlx::query_as("SELECT statut::text, date_debut::text FROM chantiers WHERE id = $1").bind(chantier).fetch_one(&pool).await.unwrap();
    assert_eq!((statut_chantier.as_str(), debut.as_str()), ("en_cours", "2026-09-01"));

    // L'en-tête d'un avancement en brouillon : référence et date, le titre suit.
    let av = d.avancement_brouillon;
    let url = format!("/chantiers/{chantier}/avancements/{av}/infos");
    let page = lire(&s, &format!("/chantiers/{chantier}/avancements/{av}")).await;
    assert!(page.contains(&format!("hx-post=\"{url}\"")) && page.contains("id=\"etat-enregistrement\""));
    let jour = aujourdhui(&pool).await;
    let (statut, page) = poster_htmx(&s, &url, &format!("reference=Novembre&date={jour}")).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(morceau(&page, "id=\"avancement-titre\"", "</h1>").contains("Novembre"));
    assert!(morceau(&page, "id=\"etat-enregistrement\"", "</span>").contains("Enregistré à "));
    let (_, page) = poster_htmx(&s, &url, &format!("reference=&date={jour}")).await;
    assert!(morceau(&page, "id=\"etat-enregistrement\"", "</span></span>").contains("La référence est obligatoire"));
    let reference: String = sqlx::query_scalar("SELECT reference FROM avancements WHERE id = $1").bind(av).fetch_one(&pool).await.unwrap();
    assert_eq!(reference, "Novembre", "la référence vidée garde la précédente");
}

#[sqlx::test]
async fn la_fiche_d_un_client_existant_s_enregistre_toute_seule(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    donnees::remplir(&pool, &s).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM entreprises WHERE nom = 'Hôtel du Parc SA'").fetch_one(&pool).await.unwrap();

    // Plus de bouton « Enregistrer » sur une fiche existante : la note dans la barre du bas, le nom en titre.
    let page = lire(&s, &format!("/entreprises/{id}/modifier")).await;
    assert!(page.contains(&format!("hx-post=\"/entreprises/{id}\"")) && page.contains("Les modifications s'enregistrent automatiquement."), "{page}");
    assert!(page.contains("<h1 id=\"fiche-titre\">Hôtel du Parc SA</h1>"));
    // Une nouvelle fiche garde son bouton de création.
    assert!(lire(&s, "/entreprises/nouveau").await.contains("Créer l&#39;entreprise") || lire(&s, "/entreprises/nouveau").await.contains("Créer l'entreprise"));

    // Un e-mail valable est gardé ; un e-mail refusé garde le précédent, le reste est enregistré.
    let (statut, page) = poster_htmx(&s, &format!("/entreprises/{id}"), "nom=H%C3%B4tel+du+Parc+SA&email=accueil%40hotel.example&langue=fr&pays=BE&ville=Bruxelles").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(morceau(&page, "id=\"etat-enregistrement\"", "</span>").contains("Enregistré à "), "{page}");
    let (statut, page) = poster_htmx(&s, &format!("/entreprises/{id}"), "nom=&email=pas-un-mail&langue=fr&pays=BE&ville=Ixelles").await;
    assert_eq!(statut, StatusCode::OK);
    let note = morceau(&page, "id=\"etat-enregistrement\"", "</span></span>");
    assert!(note.contains("Le nom est obligatoire.") && note.contains("n&#39;est pas valide") || note.contains("n'est pas valide"), "{note}");
    let (nom, email, ville): (String, Option<String>, String) =
        sqlx::query_as("SELECT nom, email, ville FROM entreprises WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((nom.as_str(), email.as_deref(), ville.as_str()), ("Hôtel du Parc SA", Some("accueil@hotel.example"), "Ixelles"));
}

#[sqlx::test]
async fn reglages_societe_et_e_mails(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    donnees::remplir(&pool, &s).await;

    // Société : plus de bouton « Enregistrer » ; une valeur refusée garde la précédente, le reste est gardé.
    let page = lire(&s, "/parametres/societe").await;
    let barre = morceau(&page, "class=\"u-barre\"", "</div>");
    assert!(barre.contains("id=\"etat-enregistrement\"") && !barre.contains("Enregistrer"), "{barre}");
    assert!(page.contains("hx-post=\"/parametres/societe\"") && page.contains("hx-disinherit=\"*\""));
    let email: String = sqlx::query_scalar("SELECT email FROM societe WHERE organisation_id = 1").fetch_one(&pool).await.unwrap();
    let corps = "nom=Toitures+Exemple&nom_abrege=TE&rue=Rue+Haute&numero=3&code_postal=1300&ville=Wavre&pays=BE&email=pas-un-email\
                 &telephone=010+11+22+33&site_web=&numero_tva=&regime_tva=assujetti&iban=BE68539007547034&bic=";
    let (statut, page) = poster_htmx(&s, "/parametres/societe", corps).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(morceau(&page, "id=\"etat-enregistrement\"", "</span></span>").contains("pas-un-email"), "{page}");
    let (nom, ville, email_garde): (String, String, String) =
        sqlx::query_as("SELECT nom, ville, email FROM societe WHERE organisation_id = 1").fetch_one(&pool).await.unwrap();
    assert_eq!((nom.as_str(), ville.as_str(), email_garde), ("Toitures Exemple", "Wavre", email));
    let (_, page) = poster_htmx(&s, "/parametres/societe", &corps.replace("pas-un-email", "info%40toitures.example")).await;
    assert!(morceau(&page, "id=\"etat-enregistrement\"", "</span>").contains("Enregistré à "));
    // Retirer une ligne (✕) demande au formulaire de s'enregistrer une fois la ligne partie.
    let r = s.clone().oneshot(Request::builder().uri("/parametres/societe/ligne/vide").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.headers().get("hx-trigger-after-swap").unwrap(), r##"{"enregistrer":{"target":"#societe"}}"##);

    // E-mails : chaque formulaire a sa note ; plus de bouton « Enregistrer ».
    let page = lire(&s, "/parametres/emails").await;
    for id in ["etat-modele-facture", "etat-rappels", "etat-mentions"] {
        assert!(page.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert!(!page.contains("Enregistrer le modèle") && !page.contains("Enregistrer les délais"));
    // Un objet vidé est refusé (il garde le précédent), le message est gardé ; la réponse est la note seule.
    let objet: String = sqlx::query_scalar("SELECT objet FROM modeles_email WHERE organisation_id = 1 AND code = 'facture'").fetch_one(&pool).await.unwrap();
    let (statut, note) = poster_htmx(&s, "/parametres/emails/facture", "objet=&corps=Bonjour+%5BNOM_CLIENT%5D").await;
    assert_eq!(statut, StatusCode::OK);
    assert!(note.trim_start().starts_with("<span id=\"etat-modele-facture\"") && note.contains("L&#39;objet est obligatoire"), "{note}");
    let (objet_garde, message): (String, String) =
        sqlx::query_as("SELECT objet, corps FROM modeles_email WHERE organisation_id = 1 AND code = 'facture'").fetch_one(&pool).await.unwrap();
    assert_eq!((objet_garde, message.as_str()), (objet, "Bonjour [NOM_CLIENT]"));
    // Délais : le bon est gardé, le mauvais garde le précédent.
    let suivant: i32 = sqlx::query_scalar("SELECT rappel_suivant_jours FROM reglages WHERE organisation_id = 1").fetch_one(&pool).await.unwrap();
    let (_, note) = poster_htmx(&s, "/parametres/rappels", "premier=10&suivant=0").await;
    assert!(note.contains("id=\"etat-rappels\"") && note.contains("Rappel suivant"), "{note}");
    let delais: (i32, i32) = sqlx::query_as("SELECT rappel_premier_jours, rappel_suivant_jours FROM reglages WHERE organisation_id = 1").fetch_one(&pool).await.unwrap();
    assert_eq!(delais, (10, suivant));
    // Mentions : la case est gardée, un taux hors bornes est refusé.
    let (_, note) = poster_htmx(&s, "/parametres/rappels/mentions", "pme=on&interets=99").await;
    assert!(note.contains("id=\"etat-mentions\"") && note.contains("Pas enregistré"), "{note}");
    let (pme, interets): (bool, Option<i32>) =
        sqlx::query_as("SELECT rappel_pme, rappel_interets_particuliers FROM reglages WHERE organisation_id = 1").fetch_one(&pool).await.unwrap();
    assert!(pme && interets.is_none_or(|i| i <= 3000));
    let (_, note) = poster_htmx(&s, "/parametres/rappels/mentions", "interets=8").await;
    assert!(note.contains("Enregistré à "), "{note}");
    let interets: Option<i32> = sqlx::query_scalar("SELECT rappel_interets_particuliers FROM reglages WHERE organisation_id = 1").fetch_one(&pool).await.unwrap();
    assert_eq!(interets, Some(800));
}
