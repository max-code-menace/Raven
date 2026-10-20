// Tests d'isolation entre organisations (étape 1-3).
//
// L'organisation A (n° 1) est remplie avec toutes les données de la vitrine. Un utilisateur de l'organisation B,
// connecté, essaie **chaque adresse du site** avec les numéros de A (devis, chantiers, avancements, lignes,
// factures, paiements, exports, ouvrages, entreprises, contacts, achats, imports) :
// - aucune page ne lui montre quoi que ce soit de A (noms, références, descriptions) ;
// - aucun PDF, export ou fichier de A ne lui est servi ;
// - après toutes ses tentatives d'écriture, les données de A sont identiques, octet par octet.

mod commun;
#[path = "commun/donnees.rs"]
mod donnees;

use std::collections::BTreeMap;

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
use ubifacture::{AppState, app, application, base::Base, connexion, organisations};

/// Une empreinte de chaque table de données de l'organisation A (toutes les colonnes de toutes ses lignes). Les
/// tables sont celles qui portent `organisation_id` : une table ajoutée plus tard est vérifiée d'office.
async fn empreinte_de_a(pool: &PgPool) -> BTreeMap<String, String> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'organisation_id'
         WHERE c.relkind = 'r' AND c.relnamespace = 'public'::regnamespace AND c.relname NOT IN ('membres', 'sessions', 'invitations')
         ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(tables.len() >= 26, "toutes les tables de données");
    let mut empreintes = BTreeMap::new();
    for table in tables {
        let sql = format!("SELECT count(*) || ':' || md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), '')) FROM {table} t");
        let e: String = sqlx::query_scalar(sqlx::AssertSqlSafe(sql)).fetch_one(pool).await.unwrap();
        empreintes.insert(table, e);
    }
    // Les tables de comptes, communes au site : la part de A.
    for (nom, sql) in [
        ("membres de A", "SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), '')) FROM membres t WHERE organisation_id = 1"),
        ("invitations de A", "SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), '')) FROM invitations t WHERE organisation_id = 1"),
        (
            "comptes de A",
            "SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), '')) FROM utilisateurs t
             WHERE id IN (SELECT utilisateur_id FROM membres WHERE organisation_id = 1)",
        ),
        ("organisation A", "SELECT md5(t::text) FROM organisations t WHERE id = 1"),
    ] {
        let e: String = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_string())).fetch_one(pool).await.unwrap();
        empreintes.insert(nom.to_string(), e);
    }
    empreintes
}

/// Des textes qui n'existent que dans les données de A (et leur forme échappée dans le HTML).
async fn textes_de_a(pool: &PgPool) -> Vec<String> {
    let textes: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT x FROM (
            SELECT nom AS x FROM entreprises UNION SELECT nom FROM contacts UNION SELECT reference FROM devis
            UNION SELECT reference FROM factures UNION SELECT description FROM ouvrages UNION SELECT nom FROM societe
            UNION SELECT fournisseur FROM chantier_achats UNION SELECT email_comptable FROM reglages
            UNION SELECT description FROM devis_lignes UNION SELECT description FROM facture_lignes UNION SELECT nom FROM devis_signatures
            UNION SELECT u.email FROM utilisateurs u JOIN membres m ON m.utilisateur_id = u.id AND m.organisation_id = 1
            UNION SELECT email FROM invitations WHERE organisation_id = 1
         ) t WHERE length(x) >= 6
           -- Les textes du catalogue commun de Raven (noms des bibliothèques, ouvrages) sont les mêmes pour tous.
           AND x NOT IN (SELECT nom FROM catalogue_bibliotheques UNION SELECT description FROM catalogue_ouvrages)",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(textes.len() > 30, "A est bien remplie : {textes:?}");
    let echapper = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;").replace('\'', "&#39;");
    textes.iter().flat_map(|t| [t.clone(), echapper(t)]).collect()
}

async fn ids(pool: &PgPool, sql: &str) -> Vec<i64> {
    let v: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_string())).fetch_all(pool).await.unwrap();
    assert!(!v.is_empty(), "A a des lignes pour : {sql}");
    v
}

/// Toutes les adresses du site avec les numéros de A. Chaque gabarit est essayé avec chaque numéro de A de ce genre.
async fn adresses_de_a(pool: &PgPool) -> Vec<(&'static str, String)> {
    let devis = ids(pool, "SELECT id FROM devis").await;
    let lignes_devis = ids(pool, "SELECT id FROM devis_lignes").await;
    let chantiers = ids(pool, "SELECT id FROM chantiers").await;
    let avancements = ids(pool, "SELECT id FROM avancements").await;
    let factures = ids(pool, "SELECT id FROM factures").await;
    let lignes_factures = ids(pool, "SELECT id FROM facture_lignes").await;
    let paiements = ids(pool, "SELECT id FROM paiements").await;
    let exports = ids(pool, "SELECT id FROM exports").await;
    let ouvrages = ids(pool, "SELECT id FROM ouvrages").await;
    let entreprises = ids(pool, "SELECT id FROM entreprises").await;
    let contacts = ids(pool, "SELECT id FROM contacts").await;
    let achats = ids(pool, "SELECT id FROM chantier_achats").await;
    let imports = ids(pool, "SELECT id FROM imports_excel").await;
    ids(pool, "SELECT id FROM recurrences").await;
    let comptes_a = ids(pool, "SELECT utilisateur_id FROM membres WHERE organisation_id = 1").await;
    let invitations_a = ids(pool, "SELECT id FROM invitations WHERE organisation_id = 1").await;

    let mut a: Vec<(&'static str, String)> = Vec::new();
    let mut deux = |m: &'static str, chemin: String| a.push((m, chemin));
    for &d in &devis {
        for suite in ["", "/apercu", "/historique", "/encodage", "/pdf", "/pdf?telecharger=1", "/totaux", "/email", "/import", "/signature/pdf"] {
            deux("GET", format!("/devis/{d}{suite}"));
        }
        for suite in [
            "", "/dupliquer", "/lignes", "/bibliotheque", "/marge", "/frais-generaux", "/statut/envoye", "/statut/accepter",
            "/statut/refuser", "/statut/brouillon", "/ajustement", "/ajustement/retirer", "/email", "/import",
        ] {
            deux("POST", format!("/devis/{d}{suite}"));
        }
        for &l in &lignes_devis {
            for suite in ["", "/supprimer", "/monter", "/descendre"] {
                deux("POST", format!("/devis/{d}/lignes/{l}{suite}"));
            }
        }
        for &i in &imports {
            deux("GET", format!("/devis/{d}/import/{i}"));
            deux("POST", format!("/devis/{d}/import/{i}"));
            deux("POST", format!("/devis/{d}/import/{i}/abandonner"));
        }
    }
    for &c in &chantiers {
        for suite in ["", "/devis", "/finances"] {
            deux("GET", format!("/chantiers/{c}{suite}"));
        }
        for suite in ["", "/avancements", "/avenants", "/factures/acompte", "/factures/finale", "/achats"] {
            deux("POST", format!("/chantiers/{c}{suite}"));
        }
        for &av in &avancements {
            deux("GET", format!("/chantiers/{c}/avancements/{av}"));
            for suite in ["/infos", "/valider", "/brouillon", "/supprimer", "/facturer"] {
                deux("POST", format!("/chantiers/{c}/avancements/{av}{suite}"));
            }
            for &l in &lignes_devis {
                deux("POST", format!("/chantiers/{c}/avancements/{av}/lignes/l{l}"));
            }
        }
        for &achat in &achats {
            deux("POST", format!("/chantiers/{c}/achats/{achat}/supprimer"));
        }
    }
    for &f in &factures {
        for suite in ["", "/pdf", "/pdf?telecharger=1", "/email", "/rappel", "/recurrence", "/archive/pdf", "/archive/ubl"] {
            deux("GET", format!("/factures/{f}{suite}"));
        }
        for suite in [
            "", "/lignes", "/valider", "/envoyee", "/supprimer", "/crediter", "/email", "/rappel", "/paiements", "/recurrence",
            "/recurrence/supprimer",
        ] {
            deux("POST", format!("/factures/{f}{suite}"));
        }
        for &l in &lignes_factures {
            deux("POST", format!("/factures/{f}/lignes/{l}"));
            deux("POST", format!("/factures/{f}/lignes/{l}/supprimer"));
        }
        for &p in &paiements {
            deux("POST", format!("/factures/{f}/paiements/{p}/supprimer"));
        }
    }
    for &e in &exports {
        deux("GET", format!("/exports/{e}/telecharger"));
        deux("POST", format!("/exports/{e}/envoyer"));
        deux("POST", format!("/exports/{e}/supprimer"));
    }
    for &o in &ouvrages {
        deux("GET", format!("/bibliotheque/{o}"));
        deux("POST", format!("/bibliotheque/{o}"));
        deux("POST", format!("/bibliotheque/{o}/supprimer"));
    }
    for (genre, liste) in [("entreprises", &entreprises), ("contacts", &contacts)] {
        for &id in liste {
            deux("GET", format!("/{genre}/{id}/modifier"));
            deux("POST", format!("/{genre}/{id}"));
        }
    }
    for &u in &comptes_a {
        deux("POST", format!("/parametres/equipe/membres/{u}/role"));
        deux("POST", format!("/parametres/equipe/membres/{u}/retirer"));
    }
    for &i in &invitations_a {
        deux("POST", format!("/parametres/equipe/invitations/{i}/annuler"));
    }
    // Les pages sans numéro : B n'y voit que ses propres données.
    for chemin in [
        "/", "/devis", "/factures", "/factures?statut=en_retard", "/chantiers", "/chantiers?statut=tous", "/entreprises", "/contacts",
        "/bibliotheque", "/bibliotheque/choisir?q=a", "/statistiques", "/statistiques?periode=12_mois", "/exports", "/factures/rappels",
        "/factures/recurrences", "/achats", "/parametres/societe", "/parametres/emails", "/parametres/societe/logo",
        "/parametres/societe/conditions-generales", "/parametres/equipe", "/parametres/metier", "/parametres/bibliotheques", "/demarrage", "/devis?q=Toit", "/factures?q=L63", "/entreprises?q=Parc", "/contacts?q=Lamb",
    ] {
        deux("GET", chemin.to_string());
    }
    for chemin in [
        "/factures/rappels", "/factures/recurrences/preparer", "/exports", "/parametres/emails/devis", "/parametres/rappels", "/demarrage",
        "/parametres/equipe/inviter", "/parametres/metier", "/parametres/rappels/mentions", "/parametres/bibliotheques", "/bibliotheque/vider",
    ] {
        deux("POST", chemin.to_string());
    }
    a
}

/// Un corps de formulaire qui remplit la plupart des champs du site (les champs inconnus d'un écran sont ignorés).
const FORMULAIRE: &str = "date=2026-10-01&reference=Intrus&responsable=Intrus&nom=Intrus&prenom=Intrus&description=Intrus&genre=poste\
&quantite=1&unite=h&prix_unitaire=1&pourcentage=50&montant=1&mode=pourcentage&valeur=10&libelle=Intrus&email=intrus%40exemple.be\
&destinataires=intrus%40exemple.be&objet=Intrus&corps=Intrus&debut=2026-01-01&fin=2026-12-31&format=les_deux&contenu=toutes\
&date_debut=2026-10-01&date_fin=2027-10-01&frequence=mois&intervalle=1&jours_avance=0&categorie=fourniture&fournisseur=Intrus\
&prix_vente=1&prix_achat=1&marge=10&remplacer=oui&rappel_premier_jours=3&rappel_suivant_jours=3&motif=Intrus&q=a&confirme=oui";

struct Reponse {
    statut: StatusCode,
    type_contenu: String,
    corps: Vec<u8>,
}

async fn envoyer(site: &Router, methode: &str, chemin: &str, cookie: &str) -> Reponse {
    let mut r = Request::builder().method(methode).uri(chemin).header(header::COOKIE, cookie);
    let corps = if methode == "POST" {
        r = r.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        Body::from(FORMULAIRE)
    } else {
        Body::empty()
    };
    let reponse = site.clone().oneshot(r.body(corps).unwrap()).await.unwrap();
    let statut = reponse.status();
    let type_contenu = reponse.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    Reponse { statut, type_contenu, corps: reponse.into_body().collect().await.unwrap().to_bytes().to_vec() }
}

/// Crée l'organisation B (vide, par `organisations::creer`) et un compte connecté, qui fait l'assistant de démarrage :
/// le cookie de session.
async fn connecter_b(pool: &PgPool, site: &Router) -> String {
    let b = organisations::creer(pool, "Organisation B").await.unwrap();
    assert_eq!(b, 2);
    connexion::ajouter_utilisateur(pool, b, "b@exemple.be", "", "un-long-mot-de-passe").await.unwrap();
    let poster = |chemin: &'static str, corps: &'static str, cookie: Option<String>| {
        let mut r = Request::builder().method("POST").uri(chemin).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(c) = cookie {
            r = r.header(header::COOKIE, c);
        }
        site.clone().oneshot(r.body(Body::from(corps)).unwrap())
    };
    let r = poster("/connexion", "email=b%40exemple.be&mot_de_passe=un-long-mot-de-passe", None).await.unwrap();
    let cookie = r.headers().get(header::SET_COOKIE).expect("connexion de B").to_str().unwrap().split(';').next().unwrap().to_string();
    let demarrage = "nom=Soci%C3%A9t%C3%A9+B&pays=BE&email=b%40exemple.be&telephone=0&regime_tva=assujetti&profil_metier=batiment\
                     &dernier_devis=0&derniere_facture=0&derniere_note_credit=0";
    let r = poster("/demarrage", demarrage, Some(cookie.clone())).await.unwrap();
    assert_eq!(r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap()), Some("/"), "assistant de démarrage de B");
    cookie
}

#[sqlx::test]
async fn b_ne_voit_ni_ne_modifie_rien_de_a_sur_aucune_page(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let site_a = app(AppState { db: pool.clone() });
    donnees::remplir(&pool, &site_a).await;
    // A a coché une bibliothèque de Raven (B, du bâtiment, ne l'a pas) et a mis son prix sur un de ses ouvrages.
    {
        let mut c = pool.acquire().await.unwrap();
        ubifacture::catalogue::cocher(&mut c, "graphisme.graphisme").await.unwrap();
        sqlx::query("UPDATE ouvrages SET prix_vente_e5 = 90_000_000, personnalise = true WHERE description = 'Création de logo'")
            .execute(&mut *c)
            .await
            .unwrap();
    }
    // L'archive légale des factures validées de A (fabriquée au premier téléchargement) et un dossier de preuve de
    // signature : B ne doit voir ni l'une ni l'autre.
    for f in ids(&pool, "SELECT id FROM factures WHERE numero IS NOT NULL AND NOT reprise").await {
        envoyer(&site_a, "GET", &format!("/factures/{f}/pdf"), "").await;
    }
    let archives: i64 = sqlx::query_scalar("SELECT count(*) FROM facture_documents").fetch_one(&pool).await.unwrap();
    assert!(archives > 0, "A a des documents archivés");
    sqlx::query(
        "INSERT INTO devis_signatures (devis_id, nom, verification, texte_accepte, montant_ttc_cents, pdf_nom, pdf, empreinte_pdf)
         SELECT id, 'Signataire de A', 'mode_test', 'Bon pour accord', montant_ttc_cents, 'Devis.pdf', '\\x255044462d', repeat('a', 64)
         FROM devis WHERE statut <> 'brouillon' ORDER BY id LIMIT 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    // L'équipe de A : un propriétaire, un membre et une invitation en attente.
    connexion::ajouter_utilisateur(&pool, 1, "patron.a@exemple.be", "Patron de A", "un-long-mot-de-passe").await.unwrap();
    connexion::ajouter_utilisateur(&pool, 1, "ouvrier.a@exemple.be", "Ouvrier de A", "un-long-mot-de-passe").await.unwrap();
    sqlx::query(
        "INSERT INTO invitations (organisation_id, email, empreinte, expire_le) VALUES (1, 'invite.a@exemple.be', '\\x01', now() + interval '7 days')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let site = application(AppState { db: pool.clone() });
    let cookie = connecter_b(&pool, &site).await;
    let avant = empreinte_de_a(&pool).await;
    let textes = textes_de_a(&pool).await;
    let adresses = adresses_de_a(&pool).await;
    assert!(adresses.len() > 1000, "{} adresses", adresses.len());
    assert_eq!(envoyer(&site, "GET", "/", &cookie).await.statut, StatusCode::OK, "B est bien connecté");

    let mut fuites = Vec::new();
    for (methode, chemin) in &adresses {
        let r = envoyer(&site, methode, chemin, &cookie).await;
        let texte = String::from_utf8_lossy(&r.corps);
        if let Some(t) = textes.iter().find(|t| texte.contains(t.as_str())) {
            fuites.push(format!("{methode} {chemin} montre « {t} »"));
        }
        let fichier = !r.type_contenu.starts_with("text/html") && !r.corps.is_empty();
        if *methode == "GET" && r.statut == StatusCode::OK && fichier && chemin.chars().any(|c| c.is_ascii_digit()) {
            fuites.push(format!("GET {chemin} sert un fichier ({}, {} octets)", r.type_contenu, r.corps.len()));
        }
        if r.statut == StatusCode::INTERNAL_SERVER_ERROR && *methode == "GET" {
            fuites.push(format!("GET {chemin} : erreur du serveur"));
        }
    }
    assert!(fuites.is_empty(), "{} fuite(s) :\n{}", fuites.len(), fuites.join("\n"));
    assert_eq!(empreinte_de_a(&pool).await, avant, "les données de A n'ont pas bougé");
}

#[sqlx::test]
async fn une_nouvelle_organisation_a_tout_ce_qu_il_faut_et_sa_propre_numerotation(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    donnees::remplir(&pool, &app(AppState { db: pool.clone() })).await;
    let modele_a: String = sqlx::query_scalar("SELECT corps FROM modeles_email WHERE code = 'devis'").fetch_one(&pool).await.unwrap();
    let site = application(AppState { db: pool.clone() });
    let cookie = connecter_b(&pool, &site).await;
    assert_eq!(organisations::creer(&pool, "  ").await.unwrap_err(), "Le nom de l'organisation est obligatoire.");

    // Ce que la création a préparé.
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let reglages: i64 = sqlx::query_scalar("SELECT count(*) FROM reglages").fetch_one(&mut *b).await.unwrap();
    let compteurs: Vec<(String, i64)> = sqlx::query_as("SELECT nom, valeur FROM compteurs ORDER BY nom").fetch_all(&mut *b).await.unwrap();
    let modeles: Vec<(String, String)> = sqlx::query_as("SELECT code, corps FROM modeles_email ORDER BY position").fetch_all(&mut *b).await.unwrap();
    let neutre: String = sqlx::query_scalar("SELECT corps FROM modeles_email_defaut WHERE code = 'devis'").fetch_one(&mut *b).await.unwrap();
    drop(b);
    assert_eq!(reglages, 1);
    let attendus = [("chantier", 0), ("devis", 0), ("facture", 0), ("note_credit", 0)].map(|(n, v)| (n.to_string(), v));
    assert_eq!(compteurs, attendus.to_vec());
    assert_eq!(modeles.len(), 7);
    assert_eq!(modeles[0].1, neutre, "les textes neutres de départ");
    assert_ne!(modeles[0].1, modele_a, "jamais les textes d'Ubique");

    // Toutes les pages principales s'ouvrent pour B.
    for chemin in [
        "/", "/devis", "/factures", "/chantiers", "/entreprises", "/entreprises/nouveau", "/contacts", "/contacts/nouveau", "/bibliotheque",
        "/bibliotheque/nouveau", "/statistiques", "/exports", "/factures/rappels", "/factures/recurrences", "/achats", "/parametres/societe",
        "/parametres/emails",
    ] {
        let r = envoyer(&site, "GET", chemin, &cookie).await;
        assert_eq!(r.statut, StatusCode::OK, "GET {chemin}");
    }

    // Sa numérotation commence à 1, quelle que soit celle de A.
    let poster = |chemin: String, corps: String| {
        let site = site.clone();
        let cookie = cookie.clone();
        async move {
            let requete = Request::builder()
                .method("POST")
                .uri(chemin)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(corps))
                .unwrap();
            let r = site.oneshot(requete).await.unwrap();
            r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
        }
    };
    let numero = |chemin: &str| chemin.trim_start_matches(|c: char| !c.is_ascii_digit()).split(|c: char| !c.is_ascii_digit()).next().unwrap().parse::<i64>().unwrap();
    let devis = numero(&poster("/devis".to_string(), String::new()).await);
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let (numero_devis,): (i64,) = sqlx::query_as("SELECT numero FROM devis WHERE id = $1").bind(devis).fetch_one(&mut *b).await.unwrap();
    assert_eq!(numero_devis, 1);
    let client: i64 = sqlx::query_scalar("INSERT INTO entreprises (nom) VALUES ('Client de B') RETURNING id").fetch_one(&mut *b).await.unwrap();
    let conditions: i64 = sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7")
        .fetch_one(&mut *b)
        .await
        .unwrap();
    let tva: i64 = sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = 2100").fetch_one(&mut *b).await.unwrap();
    let aujourdhui: String =
        sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&mut *b).await.unwrap();
    drop(b);
    let facture = numero(&poster("/factures".to_string(), String::new()).await);
    poster(format!("/factures/{facture}"), format!("date={aujourdhui}&reference=Premi%C3%A8re&client=e{client}&conditions_paiement_id={conditions}")).await;
    poster(format!("/factures/{facture}/lignes"), "genre=poste".to_string()).await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let ligne: i64 = sqlx::query_scalar("SELECT id FROM facture_lignes WHERE facture_id = $1").bind(facture).fetch_one(&mut *b).await.unwrap();
    drop(b);
    poster(format!("/factures/{facture}/lignes/{ligne}"), format!("description=Travail&quantite=1&unite=h&prix_unitaire=100&taux_tva_id={tva}")).await;
    poster(format!("/factures/{facture}/valider"), String::new()).await;
    let mut b = Base::ouvrir(&pool, 2).await.unwrap();
    let (numero_facture, vcs): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT numero, vcs FROM factures WHERE id = $1").bind(facture).fetch_one(&mut *b).await.unwrap();
    assert_eq!(numero_facture, Some(1), "la première facture de B est la n° 1");
    assert!(vcs.is_some_and(|v| v.starts_with("+++")));
    drop(b);
    let max_a: i64 = sqlx::query_scalar("SELECT max(numero) FROM factures WHERE genre <> 'note_credit'").fetch_one(&pool).await.unwrap();
    assert!(max_a > 1, "A garde sa propre numérotation");
}
