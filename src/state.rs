use std::path::Path;

use che_orm::Database;

use crate::{app_channels::AppChannels, channels::Channels, config::AppConfig, error::AppResult};

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: AppConfig,
    db: Database,
    channels: Channels,
    app_channels: AppChannels,
}

impl AppState {
    pub async fn from_config_file(path: impl AsRef<Path>) -> AppResult<Self> {
        let mut config = AppConfig::from_file(path)?;
        if let Ok(max_connections) = std::env::var("CHE_REST_DATABASE_MAX_CONNECTIONS")
            && let Ok(max_connections) = max_connections.parse()
        {
            config.database.max_connections = max_connections;
        }
        let db = Database::connect_with_max_connections(
            &config.database.url,
            config.database.max_connections,
        )
        .await?;

        Ok(Self {
            config,
            db,
            channels: Channels::new(),
            app_channels: AppChannels::new(),
        })
    }

    pub fn db(&self) -> &Database {
        &self.db
    }

    pub fn channels(&self) -> &Channels {
        &self.channels
    }

    pub fn app_channels(&self) -> &AppChannels {
        &self.app_channels
    }
}
