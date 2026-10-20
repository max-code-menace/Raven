use sqlx::{PgPool, postgres::PgPoolOptions};

/// Ouvre la connexion à PostgreSQL (adresse dans `DATABASE_URL`) et applique les migrations.
pub async fn connecter() -> Result<PgPool, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL manquante : copie .env.example en .env et remplis le mot de passe")?;
    let pool = options().connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Les réglages de la réserve de connexions.
pub fn options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(5)
        // Une connexion rendue à la réserve oublie son organisation (voir `base::Base`) ; si l'oubli échoue, elle est fermée.
        .after_release(|conn, _| Box::pin(async move { Ok(sqlx::query("RESET app.organisation_id").execute(conn).await.is_ok()) }))
}
