use std::path::Path;

use che_orm::SqliteBackend;

use crate::{config::AppConfig, error::AppResult};

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: AppConfig,
    db: SqliteBackend,
}

impl AppState {
    pub async fn from_config_file(path: impl AsRef<Path>) -> AppResult<Self> {
        let config = AppConfig::from_file(path)?;
        let db = SqliteBackend::connect(&config.database.url).await?;

        Ok(Self { config, db })
    }

    pub fn db(&self) -> &SqliteBackend {
        &self.db
    }
}
