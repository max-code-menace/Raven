// Vitrine pour vérifier la mise en page à l'œil, ignorée par `cargo test`.
// Remplit une base TEMPORAIRE avec des données fictives (jamais la vraie base) et sert l'application
// sur http://127.0.0.1:3002 pendant trois heures :
//   cargo test --test vitrine -- --ignored --nocapture

use axum::{
    Router,
    body::Body,
    http::{Request, header},
};
use sqlx::PgPool;
use tower::ServiceExt;
use ubifacture::{AppState, app};

async fn poster(site: &Router, uri: &str, corps: &str) -> Option<String> {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string())
}

async fn un(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_string())).fetch_one(pool).await.unwrap()
}

async fn ligne(pool: &PgPool, devis: i64, position: i32, genre: &str, description: &str, q: Option<i64>, unite: &str, p: Option<i64>, achat: Option<i64>) {
    let chiffre = q.is_some();
    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, taux_tva_id)
         VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8, CASE WHEN $9 THEN (SELECT id FROM taux_tva WHERE taux_centiemes = 600) END)",
    )
    .bind(devis)
    .bind(position)
    .bind(genre)
    .bind(description)
    .bind(q)
    .bind(if chiffre { unite } else { "" })
    .bind(p)
    .bind(achat)
    .bind(chiffre)
    .execute(pool)
    .await
    .unwrap();
}

/// Recalcule les totaux d'un devis en passant par l'écran Encodage (une ligne « modifiée » sans changement).
async fn recalculer(pool: &PgPool, devis: i64) {
    let total: i64 = un(
        pool,
        &format!(
            "SELECT COALESCE(sum(round(quantite_e5::numeric * prix_unitaire_e5 / 100000000)), 0)::bigint FROM devis_lignes WHERE devis_id = {devis} AND quantite_e5 IS NOT NULL"
        ),
    )
    .await;
    sqlx::query("UPDATE devis SET montant_ht_cents = $1, montant_ttc_cents = $1 + round($1 * 0.06)::bigint WHERE id = $2")
        .bind(total)
        .bind(devis)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test]
#[ignore]
async fn vitrine(pool: PgPool) {
    let s = app(AppState { db: pool.clone() });
    sqlx::query(
        "INSERT INTO societe (nom, nom_abrege, rue, numero, code_postal, ville, email, telephone, numero_tva)
         VALUES ('Ubique SRL', 'Ubique', 'Rue de l''Exemple', '1', '1300', 'Wavre', 'info@exemple.be', '010 00 00 00', 'BE0123456789')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ('BE68 5390 0754 7034', true)").execute(&pool).await.unwrap();

    // Contacts et entreprises fictifs (ceux des planches).
    for (nom, rue, cp, ville, tel) in [
        ("Hôtel du Parc SA", "Boulevard du Parc 1", "1000", "Bruxelles", "02 123 45 67"),
        ("Résidence Les Tilleuls", "Avenue des Tilleuls 4", "5000", "Namur", "081 22 33 44"),
        ("Peeters Construct SRL", "Industrieweg 5", "3000", "Louvain", "016 11 22 33"),
        ("ASBL Le Préau", "Rue du Préau 9", "1348", "Louvain-la-Neuve", "010 45 45 45"),
    ] {
        sqlx::query("INSERT INTO entreprises (nom, profils, rue, code_postal, ville, telephone, numero_tva) VALUES ($1, '{client}', $2, $3, $4, $5, 'BE0987654321')")
            .bind(nom)
            .bind(rue)
            .bind(cp)
            .bind(ville)
            .bind(tel)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (nom, prenom, profil, rue, cp, ville, tel, mail) in [
        ("Lambert", "Marc", "client", "Rue des Écoles 12", "1370", "Jodoigne", "0470 12 34 56", "lambert@exemple.be"),
        ("Claes", "Anne", "client", "Rue Haute 21", "1300", "Wavre", "0486 55 44 33", "a.claes@exemple.be"),
        ("Dubois", "Annick", "client", "Place du Marché 2", "5030", "Gembloux", "0499 88 77 66", "annick.dubois@exemple.be"),
        ("Janssens", "Pieter", "prospect", "Rue de la Station 8", "1380", "Lasne", "0471 23 45 67", ""),
        ("Renard", "Paul", "architecte", "Rue Royale 100", "1000", "Bruxelles", "02 640 10 20", "p.renard@exemple.be"),
    ] {
        sqlx::query(
            "INSERT INTO contacts (nom, prenom, profils, rue, code_postal, ville, telephone, email) VALUES ($1, $2, ARRAY[$3], $4, $5, $6, $7, NULLIF($8, ''))",
        )
        .bind(nom)
        .bind(prenom)
        .bind(profil)
        .bind(rue)
        .bind(cp)
        .bind(ville)
        .bind(tel)
        .bind(mail)
        .execute(&pool)
        .await
        .unwrap();
    }
    let lambert = un(&pool, "SELECT id FROM contacts WHERE nom = 'Lambert'").await;
    let conditions = un(&pool, "SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7").await;
    let tva6 = un(&pool, "SELECT id FROM taux_tva WHERE taux_centiemes = 600").await;

    // Des devis dans tous les statuts.
    let mut devis_ids = Vec::new();
    for (numero, date, reference, statut, client) in [
        (50, "2026-08-20", "L63 Toiture", "envoye", "c:Lambert"),
        (55, "2026-10-08", "C44 Volets", "envoye", "e:Résidence Les Tilleuls"),
        (56, "2026-10-12", "PL46 Salle de bain", "accepte", "c:Claes"),
        (57, "2026-10-14", "F28 Joues de lucarnes", "envoye", "c:Lambert"),
        (58, "2026-10-15", "HP2 Chambres 3e étage", "a_envoyer", "e:Hôtel du Parc SA"),
        (54, "2026-10-06", "C44 Climatisation", "refuse", "e:Résidence Les Tilleuls"),
        (59, "2026-10-15", "BS14 Toiture plate", "brouillon", "c:Janssens"),
    ] {
        let (genre, nom) = client.split_once(':').unwrap();
        let (e, c): (Option<i64>, Option<i64>) = if genre == "e" {
            (Some(un(&pool, &format!("SELECT id FROM entreprises WHERE nom = '{}'", nom.replace('\'', "''"))).await), None)
        } else {
            (None, Some(un(&pool, &format!("SELECT id FROM contacts WHERE nom = '{nom}'")).await))
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO devis (numero, date_devis, reference, responsable, statut, client_entreprise_id, client_contact_id, taux_tva_id,
                                conditions_paiement_id, date_refus, motif_refus)
             VALUES ($1, $2::date, $3, 'Max', $4::text::statut_devis, $5, $6, $7, $8,
                     CASE WHEN $4 = 'refuse' THEN $2::date + 4 END, CASE WHEN $4 = 'refuse' THEN 'Autre entreprise choisie' ELSE '' END)
             RETURNING id",
        )
        .bind(numero)
        .bind(date)
        .bind(reference)
        .bind(statut)
        .bind(e)
        .bind(c)
        .bind(tva6)
        .bind(conditions)
        .fetch_one(&pool)
        .await
        .unwrap();
        devis_ids.push(id);
        ligne(&pool, id, 1, "titre", "Toiture", None, "", None, None).await;
        ligne(&pool, id, 2, "main_oeuvre", "Démontage de l'ancienne couverture et évacuation", Some(2_400_000), "h", Some(4_500_000), Some(2_800_000)).await;
        ligne(&pool, id, 3, "fourniture", "Panneaux isolants PIR 12 cm", Some(3_800_000), "m²", Some(4_250_000), Some(2_900_000)).await;
        ligne(&pool, id, 4, "texte", "Garantie de 10 ans sur l'étanchéité.", None, "", None, None).await;
        ligne(&pool, id, 5, "titre", "Évacuations et chantier", None, "", None, None).await;
        ligne(&pool, id, 6, "sous_traitant", "Location d'une nacelle", Some(300_000), "j", Some(28_000_000), Some(22_000_000)).await;
        recalculer(&pool, id).await;
    }
    sqlx::query("UPDATE devis SET statut_date = date_devis + interval '1 day 10 hours'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE compteurs SET valeur = 59 WHERE nom = 'devis'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE devis SET marge_activee = true, frais_generaux_centiemes = 800 WHERE numero = 59").execute(&pool).await.unwrap();
    sqlx::query("UPDATE devis SET client_contact_id = $1 WHERE numero = 50").bind(lambert).execute(&pool).await.unwrap();

    // Le devis 50 est accepté : chantier, avancement facturé, puis un avancement en brouillon.
    let d50 = devis_ids[0];
    poster(&s, &format!("/devis/{d50}/statut/accepter"), "date=2026-08-28").await;
    let d56 = devis_ids[2];
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(d56).execute(&pool).await.unwrap();
    poster(&s, &format!("/devis/{d56}/statut/accepter"), "date=2026-10-15").await;
    let chantier = un(&pool, &format!("SELECT id FROM chantiers WHERE devis_id = {d50}")).await;
    let lignes: Vec<i64> = sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 AND quantite_e5 IS NOT NULL ORDER BY position")
        .bind(d50)
        .fetch_all(&pool)
        .await
        .unwrap();
    let lieu = poster(&s, &format!("/chantiers/{chantier}/avancements"), "").await.unwrap();
    let av: i64 = lieu.rsplit('/').next().unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/chantiers/{chantier}/avancements/{av}/infos"), "date=2026-09-15&reference=Septembre").await;
    poster(&s, &format!("/chantiers/{chantier}/avancements/{av}/lignes/l{}", lignes[0]), "pourcentage=100").await;
    poster(&s, &format!("/chantiers/{chantier}/avancements/{av}/lignes/l{}", lignes[1]), "pourcentage=50").await;
    poster(&s, &format!("/chantiers/{chantier}/avancements/{av}/valider"), "").await;
    let lieu = poster(&s, &format!("/chantiers/{chantier}/avancements/{av}/facturer"), "").await.unwrap();
    let facture: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/factures/{facture}/valider"), "").await;
    poster(&s, &format!("/factures/{facture}/envoyee"), "").await;
    let lieu = poster(&s, &format!("/chantiers/{chantier}/avancements"), "").await.unwrap();
    let av2: i64 = lieu.rsplit('/').next().unwrap().split('?').next().unwrap().parse().unwrap();
    poster(&s, &format!("/chantiers/{chantier}/avancements/{av2}/infos"), "date=2026-10-16&reference=Octobre").await;
    poster(&s, &format!("/chantiers/{chantier}/avancements/{av2}/lignes/l{}", lignes[1]), "pourcentage=20").await;

    // Une facture simple validée et créditée, et une facture simple en brouillon.
    let lieu = poster(&s, "/factures", "").await.unwrap();
    let simple: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(&pool).await.unwrap();
    let claes = un(&pool, "SELECT id FROM contacts WHERE nom = 'Claes'").await;
    poster(&s, &format!("/factures/{simple}"), &format!("date={aujourdhui}&reference=L63+Intervention&responsable=Max&client=c{claes}&conditions_paiement_id={conditions}")).await;
    poster(&s, &format!("/factures/{simple}/lignes"), "genre=poste").await;
    let l: i64 = un(&pool, &format!("SELECT max(id) FROM facture_lignes WHERE facture_id = {simple}")).await;
    poster(&s, &format!("/factures/{simple}/lignes/{l}"), &format!("description=D%C3%A9pannage+fuite&quantite=3&unite=h&prix_unitaire=55&taux_tva_id={tva6}")).await;
    poster(&s, &format!("/factures/{simple}/valider"), "").await;
    poster(&s, &format!("/factures/{simple}/crediter"), "").await;
    let lieu = poster(&s, "/factures", "").await.unwrap();
    let _brouillon = lieu;

    let ecoute = tokio::net::TcpListener::bind("127.0.0.1:3002").await.expect("port 3002 occupé");
    println!("Vitrine prête sur http://127.0.0.1:3002 (chantier {chantier}, facture {facture}, avancement {av2})");
    let serveur = axum::serve(ecoute, s);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3 * 3600), serveur).await;
}
