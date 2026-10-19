// Ce que les tests partagent : leur réserve de connexions.

use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

/// La réserve d'un test : chaque connexion travaille dans l'organisation n° 1 (les tests écrivent et lisent
/// directement dans la base pour préparer et vérifier leurs données), et revient à l'organisation n° 1 quand elle
/// est rendue (une page a pu la régler sur une autre organisation).
pub async fn reserve(options: PgPoolOptions, connexion: PgConnectOptions) -> PgPool {
    options
        .after_release(|conn, _| Box::pin(async move { Ok(sqlx::query("RESET app.organisation_id").execute(conn).await.is_ok()) }))
        .connect_with(connexion.options([("app.organisation_id", "1")]))
        .await
        .expect("connexion à la base de test")
}
