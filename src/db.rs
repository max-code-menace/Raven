use sqlx::{PgPool, postgres::PgPoolOptions};

/// La taille de la réserve de connexions sans `DB_CONNEXIONS`.
const CONNEXIONS_PAR_DEFAUT: u32 = 10;

/// Ouvre la connexion à PostgreSQL (adresse dans `DATABASE_URL`) et applique les migrations.
/// `DB_CONNEXIONS` règle la taille de la réserve (10 par défaut) : chaque page garde sa connexion jusqu'à la fin (PDF et
/// envoi d'e-mail compris), la réserve doit donc suivre le nombre de pages servies en même temps, sans dépasser le nombre
/// de connexions que l'offre PostgreSQL de l'hébergeur permet.
pub async fn connecter() -> Result<PgPool, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL manquante : copie .env.example en .env et remplis le mot de passe")?;
    let taille = match std::env::var("DB_CONNEXIONS") {
        Err(_) => CONNEXIONS_PAR_DEFAUT,
        Ok(v) => v.trim().parse::<u32>().ok().filter(|n| (1..=500).contains(n)).ok_or("DB_CONNEXIONS doit être un nombre de 1 à 500")?,
    };
    let pool = options().max_connections(taille).connect(&url).await?;
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
