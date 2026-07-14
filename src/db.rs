use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::ApiError;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

pub async fn connect(database_url: &str) -> Result<PgPool, ApiError> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .map_err(ApiError::from)
}

pub async fn migrate(pool: &PgPool) -> Result<(), ApiError> {
    MIGRATOR.run(pool).await.map_err(ApiError::from)
}
