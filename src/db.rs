use sqlx::{PgPool, postgres::PgPoolOptions};

/// Ouvre la connexion à PostgreSQL (adresse dans `DATABASE_URL`) et applique les migrations.
pub async fn connecter() -> Result<PgPool, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL manquante : copie .env.example en .env et remplis le mot de passe")?;
    let pool = PgPoolOptions::new().max_connections(5).connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
