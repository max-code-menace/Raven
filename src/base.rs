//! La connexion à la base d'une page : toujours liée à **une organisation**.
//!
//! Chaque page reçoit une [`Base`] au lieu de la connexion commune : une connexion prise dans la réserve, sur
//! laquelle le réglage `app.organisation_id` vaut l'organisation de la session. Les règles de sécurité de
//! PostgreSQL (étape 1-2) s'appuient sur ce réglage pour ne montrer que les lignes de cette organisation.
//!
//! - L'organisation vient de la session (`connexion::exiger_connexion`), ou de l'organisation n° 1 pour les tests
//!   des écrans (`crate::app`). Sans organisation connue, la page s'arrête : jamais de connexion « sans organisation ».
//! - Le réglage est remis à zéro quand la connexion retourne dans la réserve (`db::connecter`).

use std::ops::{Deref, DerefMut};

use axum::{extract::FromRequestParts, http::request::Parts};
use sqlx::{PgConnection, PgPool, Postgres, Transaction, pool::PoolConnection};

use crate::{AppState, Erreur};

/// L'organisation de la page, posée dans la requête par la connexion (ou par `crate::app` pour les tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Organisation(pub i64);

/// Une connexion à la base liée à une organisation.
pub struct Base {
    conn: PoolConnection<Postgres>,
    pub organisation: i64,
}

impl Base {
    /// Prend une connexion dans la réserve et la lie à l'organisation.
    pub async fn ouvrir(pool: &PgPool, organisation: i64) -> Result<Base, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT set_config('app.organisation_id', $1, false)")
            .bind(organisation.to_string())
            .execute(&mut *conn)
            .await?;
        Ok(Base { conn, organisation })
    }

    /// Commence une transaction sur cette connexion (le réglage d'organisation reste valable).
    pub async fn begin(&mut self) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
        sqlx::Connection::begin(&mut *self.conn).await
    }
}

impl Deref for Base {
    type Target = PgConnection;
    fn deref(&self) -> &PgConnection {
        &self.conn
    }
}

impl DerefMut for Base {
    fn deref_mut(&mut self) -> &mut PgConnection {
        &mut self.conn
    }
}

impl FromRequestParts<AppState> for Base {
    type Rejection = Erreur;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Base, Erreur> {
        let Some(&Organisation(id)) = parts.extensions.get::<Organisation>() else {
            eprintln!("Page {} demandée sans organisation : refusée.", parts.uri.path());
            return Err(Erreur::interne());
        };
        Ok(Base::ouvrir(&state.db, id).await?)
    }
}
