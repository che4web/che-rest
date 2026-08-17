use std::path::Path;

use che_orm2::Database;

use crate::{AppConfig, AppResult};
use crate::{app_channels::AppChannels, channels::Channels};

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    database: Database,
    channels: Channels,
    app_channels: AppChannels,
}

impl AppState {
    pub fn from_database(database: Database) -> Self {
        Self {
            config: AppConfig::default(),
            database,
            channels: Channels::new(),
            app_channels: AppChannels::new(),
        }
    }

    pub async fn from_config_file(path: impl AsRef<Path>) -> AppResult<Self> {
        let config = AppConfig::from_file(path)?;
        let database = Database::connect_with_pool_size(
            sqlite_path(&config.database.url),
            config.database.max_connections as usize,
        )?;
        Ok(Self {
            config,
            database,
            channels: Channels::new(),
            app_channels: AppChannels::new(),
        })
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn channels(&self) -> &Channels {
        &self.channels
    }

    pub fn app_channels(&self) -> &AppChannels {
        &self.app_channels
    }
}

fn sqlite_path(url: &str) -> String {
    url.strip_prefix("sqlite://")
        .and_then(|value| value.split('?').next())
        .filter(|value| !value.is_empty())
        .unwrap_or(url)
        .to_string()
}
