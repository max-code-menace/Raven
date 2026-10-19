// Tests de la base : chaque test reçoit une base temporaire avec les migrations déjà appliquées.
// Ils demandent DATABASE_URL (fichier .env) avec un utilisateur autorisé à créer des bases.

mod commun;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test]
async fn les_taux_de_tva_de_depart_existent(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let taux: Vec<(i32, bool)> =
        sqlx::query_as("SELECT taux_centiemes, autoliquidation FROM taux_tva ORDER BY taux_centiemes")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(taux, vec![(0, true), (600, false), (2100, false)]);
}

#[sqlx::test]
async fn les_dix_conditions_de_paiement_existent(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM conditions_paiement")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 10);
}

#[sqlx::test]
async fn paiement_comptant_doit_avoir_zero_jour(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let res = sqlx::query("INSERT INTO conditions_paiement (type, jours) VALUES ('comptant', 5)")
        .execute(&pool)
        .await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn une_seule_societe_par_organisation(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let insert = "INSERT INTO societe (nom, nom_abrege, email, telephone) VALUES ('A', 'A', 'a@a.be', '0')";
    sqlx::query(insert).execute(&pool).await.unwrap();
    assert!(sqlx::query(insert).execute(&pool).await.is_err(), "une deuxième société dans la même organisation");
    sqlx::query("INSERT INTO organisations (nom) VALUES ('B')").execute(&pool).await.unwrap();
    let ailleurs = "INSERT INTO societe (organisation_id, nom, nom_abrege, email, telephone) VALUES (2, 'B', 'B', 'b@b.be', '0')";
    assert!(sqlx::query(ailleurs).execute(&pool).await.is_err(), "jamais d'écriture dans une autre organisation");
}

#[sqlx::test]
async fn un_seul_compte_bancaire_par_defaut(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let insert = "INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ($1, true)";
    sqlx::query(insert).bind("BE00 0000 0000 0001").execute(&pool).await.unwrap();
    assert!(sqlx::query(insert).bind("BE00 0000 0000 0002").execute(&pool).await.is_err());
}

#[sqlx::test]
async fn un_profil_inconnu_est_refuse(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let res = sqlx::query("INSERT INTO entreprises (nom, profils) VALUES ('X', ARRAY['inconnu'])")
        .execute(&pool)
        .await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn entreprise_et_contact_lies(options: PgPoolOptions, connexion: PgConnectOptions) {
    let pool = commun::reserve(options, connexion).await;
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO entreprises (nom, profils, numero_tva) VALUES ('Dupont SRL', ARRAY['client'], 'BE0123456789') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO contacts (civilite, nom, prenom, entreprise_id, profils) VALUES ('M.', 'Dupont', 'Jean', $1, ARRAY['client'])")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    // Supprimer l'entreprise ne supprime pas le contact : il perd seulement son lien.
    sqlx::query("DELETE FROM entreprises WHERE id = $1").bind(id).execute(&pool).await.unwrap();
    let (lien,): (Option<i64>,) = sqlx::query_as("SELECT entreprise_id FROM contacts WHERE nom = 'Dupont'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(lien, None);
}
