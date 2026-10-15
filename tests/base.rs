// Tests de la base : chaque test reçoit une base temporaire avec les migrations déjà appliquées.
// Ils demandent DATABASE_URL (fichier .env) avec un utilisateur autorisé à créer des bases.

use sqlx::PgPool;

#[sqlx::test]
async fn les_taux_de_tva_de_depart_existent(pool: PgPool) {
    let taux: Vec<(i32, bool)> =
        sqlx::query_as("SELECT taux_centiemes, autoliquidation FROM taux_tva ORDER BY taux_centiemes")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(taux, vec![(0, true), (600, false), (2100, false)]);
}

#[sqlx::test]
async fn les_dix_conditions_de_paiement_existent(pool: PgPool) {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM conditions_paiement")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 10);
}

#[sqlx::test]
async fn paiement_comptant_doit_avoir_zero_jour(pool: PgPool) {
    let res = sqlx::query("INSERT INTO conditions_paiement (type, jours) VALUES ('comptant', 5)")
        .execute(&pool)
        .await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn la_societe_n_a_qu_une_seule_ligne(pool: PgPool) {
    let insert = "INSERT INTO societe (id, nom, nom_abrege, email, telephone) VALUES ($1, 'A', 'A', 'a@a.be', '0')";
    sqlx::query(insert).bind(1_i16).execute(&pool).await.unwrap();
    assert!(sqlx::query(insert).bind(2_i16).execute(&pool).await.is_err());
    assert!(sqlx::query(insert).bind(1_i16).execute(&pool).await.is_err());
}

#[sqlx::test]
async fn un_seul_compte_bancaire_par_defaut(pool: PgPool) {
    let insert = "INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ($1, true)";
    sqlx::query(insert).bind("BE00 0000 0000 0001").execute(&pool).await.unwrap();
    assert!(sqlx::query(insert).bind("BE00 0000 0000 0002").execute(&pool).await.is_err());
}

#[sqlx::test]
async fn un_profil_inconnu_est_refuse(pool: PgPool) {
    let res = sqlx::query("INSERT INTO entreprises (nom, profils) VALUES ('X', ARRAY['inconnu'])")
        .execute(&pool)
        .await;
    assert!(res.is_err());
}

#[sqlx::test]
async fn entreprise_et_contact_lies(pool: PgPool) {
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
