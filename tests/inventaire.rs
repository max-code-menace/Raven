// Tests de ce qui a été ajouté après l'inventaire de Vertuoza (17/10/2026) : numérotation annuelle des factures,
// factures récurrentes, ouvertures par les clients sur le tableau de bord, import de la bibliothèque, colonne
// Chantier de la liste des factures.

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

async fn televerser(site: &Router, uri: &str, nom: &str, octets: &[u8]) -> (StatusCode, Option<String>, String) {
    let limite = "----limite-ubifacture";
    let mut corps =
        format!("--{limite}\r\nContent-Disposition: form-data; name=\"fichier\"; filename=\"{nom}\"\r\nContent-Type: text/csv\r\n\r\n").into_bytes();
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

async fn aujourdhui(pool: &PgPool) -> chrono::NaiveDate {
    sqlx::query_scalar("SELECT (now() AT TIME ZONE 'Europe/Brussels')::date").fetch_one(pool).await.unwrap()
}

/// Une facture simple complète (client, conditions, une ligne de 100 € HT à 21 %), datée `date`, en brouillon.
async fn facture(pool: &PgPool, s: &Router, date: chrono::NaiveDate, reference: &str) -> i64 {
    let (_, lieu, _) = poster(s, "/factures", "").await;
    let id: i64 = lieu.unwrap().strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom, profils, email) VALUES ('Locataire SA', '{client}', 'loc@exemple.be') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap();
    let condition: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 30").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}"), &format!("date={date}&reference={reference}&client=e{client}&conditions_paiement_id={condition}")).await;
    poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
    let ligne: i64 = sqlx::query_scalar("SELECT max(id) FROM facture_lignes WHERE facture_id = $1").bind(id).fetch_one(pool).await.unwrap();
    let taux: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(pool).await.unwrap();
    poster(s, &format!("/factures/{id}/lignes/{ligne}"), &format!("description=Location+du+lot&quantite=1&prix_unitaire=100&taux_tva_id={taux}")).await;
    id
}

#[sqlx::test]
async fn la_numerotation_repart_a_1_chaque_annee(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let jour = aujourdhui(&pool).await;
    let annee = chrono::Datelike::year(&jour);
    // Comme dans Vertuoza fin 2025 : le compteur est à 106 pour l'année passée.
    sqlx::query("UPDATE compteurs SET valeur = 106, annee = $1 WHERE nom = 'facture'").bind(annee - 1).execute(&pool).await.unwrap();

    let id = facture(&pool, &s, jour, "Lot1").await;
    let (_, lieu, _) = poster(&s, &format!("/factures/{id}/valider"), "").await;
    assert_eq!(lieu, Some(format!("/factures/{id}?ok=validee")));
    let (numero, vcs): (i64, String) = sqlx::query_as("SELECT numero, vcs FROM factures WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((numero, vcs), (1, ubifacture::facture_calcul::vcs_annuelle(annee, 1)), "première facture de l'année : n° 1");
    let second = facture(&pool, &s, jour, "Lot2").await;
    poster(&s, &format!("/factures/{second}/valider"), "").await;
    let numero: i64 = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(second).fetch_one(&pool).await.unwrap();
    assert_eq!(numero, 2);

    // L'année passée est close : une facture datée de l'an dernier ne se valide plus.
    let ancienne = facture(&pool, &s, jour - chrono::Days::new(400), "Vieux").await;
    let (_, page) = lire(&s, &format!("/factures/{ancienne}")).await;
    assert!(page.contains(&format!("Une date en {annee} au plus tôt")), "le motif est affiché");
    assert_eq!(poster(&s, &format!("/factures/{ancienne}/valider"), "").await.0, StatusCode::CONFLICT);
    let (valeur, an): (i64, i32) = sqlx::query_as("SELECT valeur, annee FROM compteurs WHERE nom = 'facture'").fetch_one(&pool).await.unwrap();
    assert_eq!((valeur, an), (2, annee), "le compteur n'a pas bougé");

    // Deux factures n° 1 de deux années différentes peuvent coexister (unique par année).
    sqlx::query("INSERT INTO factures (genre, numero, statut, date_facture) VALUES ('simple', 1, 'a_envoyer', $1)")
        .bind(jour - chrono::Days::new(400))
        .execute(&pool)
        .await
        .unwrap();
    let doublon = sqlx::query("INSERT INTO factures (genre, numero, statut, date_facture) VALUES ('simple', 1, 'a_envoyer', $1)").bind(jour).execute(&pool).await;
    assert!(doublon.is_err(), "jamais deux n° 1 la même année");
}

#[sqlx::test]
async fn une_location_mensuelle(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let jour = aujourdhui(&pool).await;
    let modele = facture(&pool, &s, jour, "Location+-+Lot+1").await;

    // La fiche propose « Créer/voir la récurrence » ; le formulaire montre le nombre d'occurrences.
    let (_, page) = lire(&s, &format!("/factures/{modele}")).await;
    assert!(page.contains(&format!("/factures/{modele}/recurrence")));
    let (code, page) = lire(&s, &format!("/factures/{modele}/recurrence")).await;
    assert_eq!(code, StatusCode::OK);
    assert!(page.contains("Création d&#39;une récurrence") || page.contains("Création d'une récurrence"));
    assert!(page.contains("12 occurrence(s) trouvée(s)"), "proposition : chaque mois pendant un an");

    // Chaque mois, depuis deux mois avant aujourd'hui jusqu'à dans deux mois.
    let debut = jour.checked_sub_months(chrono::Months::new(2)).unwrap();
    let fin = jour.checked_add_months(chrono::Months::new(2)).unwrap();
    let (_, page) = lire(&s, &format!("/factures/recurrences/apercu?date_debut={debut}&date_fin={fin}&frequence=mois&intervalle=1&jours_avance=0")).await;
    assert!(page.contains("5 occurrence(s) trouvée(s)") && page.contains("Tous les mois"), "{page}");
    // Une saisie fausse est refusée sans rien enregistrer.
    let (code, _, page) = poster(&s, &format!("/factures/{modele}/recurrence"), &format!("date_debut={fin}&date_fin={debut}&frequence=mois&intervalle=1")).await;
    assert!(code == StatusCode::OK && page.contains("La date de fin doit suivre la date de début."));
    let (_, lieu, _) =
        poster(&s, &format!("/factures/{modele}/recurrence"), &format!("date_debut={debut}&date_fin={fin}&frequence=mois&intervalle=1&jours_avance=0")).await;
    assert_eq!(lieu, Some(format!("/factures/{modele}/recurrence?ok=enregistree")));

    // Trois échéances sont arrivées (il y a deux mois, il y a un mois, aujourd'hui) : le tableau de bord les signale.
    let (_, accueil) = lire(&s, "/").await;
    assert!(accueil.contains("3 facture(s) récurrente(s) à préparer"));
    let (_, liste) = lire(&s, "/factures/recurrences").await;
    assert!(liste.contains("Locataire SA") && liste.contains("Tous les mois") && liste.contains("Préparer les factures (3)"));

    let (_, lieu, _) = poster(&s, "/factures/recurrences/preparer", "").await;
    assert_eq!(lieu.as_deref(), Some("/factures/recurrences?preparees=3"));
    let creees: Vec<(i64, String, Option<i64>, chrono::NaiveDate, i64)> = sqlx::query_as(
        "SELECT id, genre::text, numero, date_facture, montant_ttc_cents FROM factures WHERE recurrence_id IS NOT NULL ORDER BY date_facture",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(creees.len(), 3);
    assert!(creees.iter().all(|(_, genre, numero, _, ttc)| genre == "recurrente" && numero.is_none() && *ttc == 12_100), "brouillons copiés du modèle");
    assert_eq!((creees[0].3, creees[2].3), (debut, jour));
    let lignes: i64 = sqlx::query_scalar("SELECT count(*) FROM facture_lignes WHERE facture_id = $1").bind(creees[0].0).fetch_one(&pool).await.unwrap();
    assert_eq!(lignes, 1);
    let (_, page) = lire(&s, &format!("/factures/{}", creees[0].0)).await;
    assert!(page.contains("Facture récurrente") && page.contains("Facture récurrente prévue le"));

    // Rien de plus à préparer ; un brouillon supprimé n'est pas reproposé.
    assert_eq!(poster(&s, "/factures/recurrences/preparer", "").await.1.as_deref(), Some("/factures/recurrences?preparees=0"));
    poster(&s, &format!("/factures/{}/supprimer", creees[1].0), "").await;
    assert_eq!(poster(&s, "/factures/recurrences/preparer", "").await.1.as_deref(), Some("/factures/recurrences?preparees=0"));
    let (_, accueil) = lire(&s, "/").await;
    assert!(!accueil.contains("récurrente(s) à préparer"));

    // Un brouillon préparé se valide comme une autre facture.
    poster(&s, &format!("/factures/{}/valider", creees[2].0), "").await;
    let numero: Option<i64> = sqlx::query_scalar("SELECT numero FROM factures WHERE id = $1").bind(creees[2].0).fetch_one(&pool).await.unwrap();
    assert!(numero.is_some());
    let (_, page) = lire(&s, "/factures?genre=recurrente").await;
    assert!(page.contains("Facture récurrente"));

    // Arrêter la récurrence : les factures créées restent.
    poster(&s, &format!("/factures/{modele}/recurrence/supprimer"), "").await;
    let reste: i64 = sqlx::query_scalar("SELECT count(*) FROM factures WHERE genre = 'recurrente'").fetch_one(&pool).await.unwrap();
    assert_eq!(reste, 2);

    // Une note de crédit ne sert pas de modèle.
    poster(&s, &format!("/factures/{}/crediter", creees[2].0), "").await;
    let note: i64 = sqlx::query_scalar("SELECT id FROM factures WHERE facture_origine_id = $1").bind(creees[2].0).fetch_one(&pool).await.unwrap();
    assert_eq!(lire(&s, &format!("/factures/{note}/recurrence")).await.0, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn les_ouvertures_par_les_clients(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    sqlx::query("INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('Ubique SRL', 'Ubique', 'info@ubique.be', '010')").execute(&pool).await.unwrap();
    let s = site(&pool);
    let jour = aujourdhui(&pool).await;
    let id = facture(&pool, &s, jour, "HP1").await;
    poster(&s, &format!("/factures/{id}/valider"), "").await;
    let (_, accueil) = lire(&s, "/").await;
    assert!(accueil.contains("Ouvertures par vos clients") && accueil.contains("Aucun devis ni aucune facture ouverts en ligne"));

    let (_, page) = lire(&s, &format!("/factures/{id}")).await;
    let debut = page.find("/client/facture/").expect("lien de la page client");
    let chemin = &page[debut..debut + page[debut..].find('"').unwrap()];
    lire(&s, chemin).await;
    lire(&s, chemin).await;
    let ouvertures: i64 = sqlx::query_scalar("SELECT count(*) FROM consultations_client WHERE facture_id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(ouvertures, 2, "chaque ouverture est notée");
    let (_, accueil) = lire(&s, "/").await;
    assert!(accueil.contains("La facture n° 125 (HP1) a été ouverte par Locataire SA."));
}

#[sqlx::test]
async fn importer_la_bibliotheque(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let (code, page) = lire(&s, "/bibliotheque/import").await;
    assert!(code == StatusCode::OK && page.contains("enctype=\"multipart/form-data\""));
    let csv = "Description;Type;Catégorie;Fournisseur;Unité;Prix d’achat;Marge\n\
               Modification alimentation boiler;Ouvrage;;;fft;197,50000 €;0 %\n\
               Tableau divisionnaire complet;Ouvrage;;;pce;825,00000 €;0 %\n\
               Prix illisible;;;;pce;abc;\n";
    let (code, _, page) = televerser(&s, "/bibliotheque/import", "ouvrages.csv", csv.as_bytes()).await;
    assert_eq!(code, StatusCode::OK, "une ligne écartée : le résultat et le détail restent affichés");
    assert!(page.contains("2 ouvrage(s) ajouté(s)") && page.contains("Ligne 4 :"));
    let (achat, vente): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT prix_achat_e5, prix_vente_e5 FROM ouvrages WHERE description = 'Modification alimentation boiler'").fetch_one(&pool).await.unwrap();
    assert_eq!((achat, vente), (Some(19_750_000), Some(19_750_000)), "marge 0 % : vente = achat, comme chez Ubique");

    // Relancer l'import n'ajoute pas de doublons.
    let propre = "Description;Unité;Prix d’achat;Marge\nModification alimentation boiler;fft;197,5;0\nPlinthes MDF à peindre;ml;12;0\n";
    let (_, lieu, _) = televerser(&s, "/bibliotheque/import", "ouvrages.csv", propre.as_bytes()).await;
    assert_eq!(lieu.as_deref(), Some("/bibliotheque/import?ajoutes=1&deja=1"));
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ouvrages").fetch_one(&pool).await.unwrap();
    assert_eq!(total, 3);
    let (_, page) = lire(&s, "/bibliotheque/import?ajoutes=1&deja=1").await;
    assert!(page.contains("1 ouvrage(s) ajouté(s)") && page.contains("1 déjà présent(s)"));

    // Un fichier sans en-tête reconnu est refusé.
    let (code, _, page) = televerser(&s, "/bibliotheque/import", "x.csv", b"a;b\n1;2\n").await;
    assert!(code == StatusCode::UNPROCESSABLE_ENTITY && page.contains("La ligne d&#39;en-tête") || page.contains("La ligne d'en-tête"));
}
