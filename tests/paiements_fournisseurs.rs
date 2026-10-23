// Paiements des factures fournisseurs reçues (23/10/2026) : payer en une ou plusieurs fois, « À payer »,
// « Partiellement payée », « En retard », « Payée », total à payer de la boîte de réception, pas de paiement sur une
// note de crédit ; et le chantier proposé d'office quand la facture porte sa référence.

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

async fn lire(site: &Router, uri: &str) -> String {
    let (s, _, corps) = envoyer(site, Request::builder().uri(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{uri}");
    corps
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

async fn jour(pool: &PgPool, decalage: i32) -> String {
    sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date + $1)::text").bind(decalage).fetch_one(pool).await.unwrap()
}

async fn virement(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").fetch_one(pool).await.unwrap()
}

/// Une pièce reçue de 1 210,00 € TTC (1 000 € HT), datée d'il y a 40 jours, échue dans `echeance` jours (négatif :
/// déjà échue), avec le XML donné. Renvoie son id.
async fn recue(pool: &PgPool, genre: &str, numero: &str, echeance: i32, xml: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO factures_recues (document_id, genre, fournisseur_nom, numero, date_facture, date_echeance, montant_ht_cents,
             montant_tva_cents, montant_ttc_cents, a_payer_cents, iban, communication, donnees, xml, empreinte, recue_le)
         VALUES ('doc-' || $2, $1, 'Négoce Matériaux', $2, (now() AT TIME ZONE 'Europe/Brussels')::date - 40,
                 (now() AT TIME ZONE 'Europe/Brussels')::date + $3, 100000, 21000, 121000, 121000, 'BE68539007547034',
                 '+++123/4567/89002+++', '{}'::jsonb, $4, repeat('a', 64), now())
         RETURNING id",
    )
    .bind(genre)
    .bind(numero)
    .bind(echeance)
    .bind(xml)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn etat(pool: &PgPool, id: i64) -> (String, i64, i64) {
    sqlx::query_as("SELECT etat, paye_cents, reste_cents FROM factures_recues_etats WHERE facture_recue_id = $1").bind(id).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn payer_une_facture_recue_en_deux_fois(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let id = recue(&pool, "facture", "NM-1", 20, "<Invoice/>").await;
    let m = virement(&pool).await;
    let aujourdhui = jour(&pool, 0).await;
    assert_eq!(etat(&pool, id).await, ("a_payer".to_string(), 0, 121_000));

    let page = lire(&s, &format!("/achats/recues/{id}")).await;
    for attendu in ["Reste à payer", "Ajouter un paiement", "Aucun paiement enregistré.", &format!("1\u{a0}210,00{E}"), "BE68539007547034", "Virement"] {
        assert!(page.contains(attendu), "{attendu}");
    }

    // 400 € par virement : partiellement payée.
    let url = format!("/achats/recues/{id}/paiements");
    let (statut, lieu) = poster(&s, &url, &format!("genre=partiel&date={aujourdhui}&montant=400&methode_id={m}&remarque=Premier+versement")).await;
    assert_eq!((statut, lieu), (StatusCode::SEE_OTHER, Some(format!("/achats/recues/{id}?ok=paiement#paiement"))));
    assert_eq!(etat(&pool, id).await, ("partielle".to_string(), 40_000, 81_000));
    let page = lire(&s, &format!("/achats/recues/{id}?ok=paiement")).await;
    for attendu in ["Le paiement est enregistré.", "Partiellement payée", &format!("400,00{E}"), &format!("810,00{E}"), "Premier versement"] {
        assert!(page.contains(attendu), "{attendu}");
    }

    // Plus que le reste, ou dans le futur : refusé, rien n'est enregistré.
    let (_, lieu) = poster(&s, &url, &format!("genre=partiel&date={aujourdhui}&montant=810,01&methode_id={m}")).await;
    assert_eq!(lieu, Some(format!("/achats/recues/{id}?erreur=paiement_montant&payer=1#paiement")));
    let demain = jour(&pool, 1).await;
    let (_, lieu) = poster(&s, &url, &format!("genre=partiel&date={demain}&montant=10&methode_id={m}")).await;
    assert_eq!(lieu, Some(format!("/achats/recues/{id}?erreur=paiement_date&payer=1#paiement")));
    assert_eq!(etat(&pool, id).await.1, 40_000);
    let page = lire(&s, &format!("/achats/recues/{id}?erreur=paiement_montant&payer=1")).await;
    assert!(page.contains("ne pas dépasser le reste à payer"));

    // « Complet » paie le reste : payée, plus rien à payer.
    let (_, lieu) = poster(&s, &url, &format!("genre=complet&date={aujourdhui}&methode_id={m}")).await;
    assert_eq!(lieu, Some(format!("/achats/recues/{id}?ok=payee#paiement")));
    assert_eq!(etat(&pool, id).await, ("payee".to_string(), 121_000, 0));
    let page = lire(&s, &format!("/achats/recues/{id}?ok=payee")).await;
    assert!(page.contains("la facture est payée"));
    assert!(!page.contains("Ajouter un paiement"));
    let (statut, _) = poster(&s, &url, &format!("genre=complet&date={aujourdhui}&methode_id={m}")).await;
    assert_eq!(statut, StatusCode::CONFLICT);

    // Le paiement de 400 € retiré (saisi par erreur) : de nouveau partiellement payée.
    let premier: i64 = sqlx::query_scalar("SELECT id FROM paiements_fournisseurs WHERE montant_cents = 40000").fetch_one(&pool).await.unwrap();
    let (_, lieu) = poster(&s, &format!("/achats/recues/{id}/paiements/{premier}/supprimer"), "").await;
    assert_eq!(lieu, Some(format!("/achats/recues/{id}?ok=paiement_retire#paiement")));
    assert_eq!(etat(&pool, id).await, ("partielle".to_string(), 81_000, 40_000));
    let (statut, _) = poster(&s, &format!("/achats/recues/{id}/paiements/{premier}/supprimer"), "").await;
    assert_eq!(statut, StatusCode::NOT_FOUND, "déjà retiré");
}

#[sqlx::test]
async fn la_boite_montre_ce_qui_reste_a_payer_et_le_retard(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let m = virement(&pool).await;
    let aujourdhui = jour(&pool, 0).await;
    recue(&pool, "facture", "NM-10", 15, "<Invoice/>").await;
    let echue = recue(&pool, "facture", "NM-11", -12, "<Invoice/>").await;
    let payee = recue(&pool, "facture", "NM-12", 5, "<Invoice/>").await;
    let note = recue(&pool, "note_credit", "NC-3", 5, "<CreditNote/>").await;
    poster(&s, &format!("/achats/recues/{payee}/paiements"), &format!("genre=complet&date={aujourdhui}&methode_id={m}")).await;
    assert_eq!(etat(&pool, echue).await, ("en_retard".to_string(), 0, 121_000));
    assert_eq!(etat(&pool, note).await, ("note_credit".to_string(), 0, 0));

    // « À payer » : les deux factures pas soldées, l'échue d'abord ; ni la payée, ni la note de crédit.
    let page = lire(&s, "/achats/recues?vue=a_payer&ok=filtre").await;
    for attendu in ["NM-10", "NM-11", "En retard", "· 12 jours", "À payer <small>2</small>", "Payées <small>1</small>"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(!page.contains("NM-12") && !page.contains("NC-3"));
    assert!(page.find("NM-11").unwrap() < page.find("NM-10").unwrap(), "la plus urgente d'abord");
    assert!(page.contains(&format!("Total à payer TTC : <strong>2\u{a0}420,00{E}</strong>")));
    assert!(page.contains(&format!("dont en retard : <strong style=\"color:var(--bad)\">1\u{a0}210,00{E}</strong> (1)")));

    let page = lire(&s, "/achats/recues?vue=payees&ok=filtre").await;
    assert!(page.contains("NM-12") && !page.contains("NM-10"));
    let page = lire(&s, "/achats/recues?vue=toutes&ok=filtre").await;
    assert!(page.contains("NC-3") && page.contains("Payée"));

    // La fiche de l'échue dit le retard.
    let page = lire(&s, &format!("/achats/recues/{echue}")).await;
    assert!(page.contains("12 jours de retard"));

    // Une note de crédit ne se paie pas : ni par l'écran, ni dans la base.
    let page = lire(&s, &format!("/achats/recues/{note}")).await;
    assert!(page.contains("Elle ne se paie pas") && !page.contains("Ajouter un paiement"));
    let (statut, _) = poster(&s, &format!("/achats/recues/{note}/paiements"), &format!("genre=complet&date={aujourdhui}&methode_id={m}")).await;
    assert_eq!(statut, StatusCode::CONFLICT);
    let refus = sqlx::query("INSERT INTO paiements_fournisseurs (facture_recue_id, date_paiement, montant_cents, methode_id) VALUES ($1, current_date, 100, $2)")
        .bind(note)
        .bind(m)
        .execute(&pool)
        .await;
    assert!(refus.is_err(), "la base refuse un paiement sur une note de crédit");
    // La base refuse aussi de payer plus que le montant à payer.
    let refus = sqlx::query("INSERT INTO paiements_fournisseurs (facture_recue_id, date_paiement, montant_cents, methode_id) VALUES ($1, current_date, 121001, $2)")
        .bind(echue)
        .bind(m)
        .execute(&pool)
        .await;
    assert!(refus.is_err(), "la base refuse un paiement plus grand que le reste");

    // Paiements › À payer mène à cette vue.
    let page = lire(&s, "/factures/rappels").await;
    assert!(page.contains("href=\"/achats/recues?vue=a_payer\""));
}

#[sqlx::test]
async fn le_chantier_est_propose_d_office_quand_la_facture_porte_sa_reference(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let s = site(&pool);
    let mut chantiers = Vec::new();
    for (numero, reference) in [(1, "L63 Toiture"), (2, "PL46 Salle de bain")] {
        let devis: i64 = sqlx::query_scalar("INSERT INTO devis (numero, date_devis, reference, statut) VALUES ($1, current_date, $2, 'accepte') RETURNING id")
            .bind(numero)
            .bind(reference)
            .fetch_one(&pool)
            .await
            .unwrap();
        let chantier: i64 = sqlx::query_scalar("INSERT INTO chantiers (numero, devis_id, date_debut) VALUES ($1, $2, current_date) RETURNING id")
            .bind(numero)
            .bind(devis)
            .fetch_one(&pool)
            .await
            .unwrap();
        chantiers.push(chantier);
    }
    let xml = r#"<Invoice xmlns:cbc="urn:cbc" xmlns:cac="urn:cac"><cbc:ID>NM-20</cbc:ID><cbc:BuyerReference>Chantier PL-46 Claes</cbc:BuyerReference><cac:OrderReference><cbc:ID>BC 7781</cbc:ID></cac:OrderReference></Invoice>"#;
    let id = recue(&pool, "facture", "NM-20", 30, xml).await;
    let page = lire(&s, &format!("/achats/recues/{id}")).await;
    for attendu in ["Chantier PL-46 Claes", "Bon de commande", "BC 7781", "car la facture porte la référence « PL46 »", "PL46 Salle de bain (n° 2)"] {
        assert!(page.contains(attendu), "{attendu}");
    }
    assert!(page.contains(&format!("<option value=\"{}\" selected>PL46 Salle de bain (n° 2)", chantiers[1])), "le chantier PL46 est choisi d'avance");
    assert!(!page.contains("selected>L63 Toiture"));

    // Sans référence reconnue, rien n'est proposé.
    let autre = recue(&pool, "facture", "NM-21", 30, "<Invoice><BuyerReference>Commande 2026-118</BuyerReference></Invoice>").await;
    let page = lire(&s, &format!("/achats/recues/{autre}")).await;
    assert!(page.contains("Commande 2026-118"));
    assert!(!page.contains("car la facture porte la référence"));
    assert!(!page.contains("selected>L63 Toiture") && !page.contains("selected>PL46 Salle de bain"));

    // Une fois toute la base répartie, plus de proposition.
    poster(&s, &format!("/achats/recues/{id}/rattacher"), &format!("destination={}&montant=1000&categorie=fourniture", chantiers[1])).await;
    let page = lire(&s, &format!("/achats/recues/{id}")).await;
    assert!(page.contains("Rattachée") && !page.contains("car la facture porte la référence"));
}
