use std::{
    ops::Deref,
    path::Path,
    sync::{Arc, Weak},
};

use che_orm::SqliteBackend;

use crate::{channels::Channels, config::AppConfig, error::AppResult, events::EventBus};

pub struct AppState {
    inner: Arc<AppStateInner>,
}

pub struct AppStateInner {
    pub config: AppConfig,
    db: SqliteBackend,
    channels: Channels,
    events: EventBus,
}

impl AppState {
    pub async fn from_config_file(path: impl AsRef<Path>) -> AppResult<Self> {
        let config = AppConfig::from_file(path)?;
        let db = SqliteBackend::connect(&config.database.url).await?;

        Ok(Self::from_inner(Arc::new(AppStateInner {
            config,
            db,
            channels: Channels::new(),
            events: EventBus::default(),
        })))
    }

    pub fn db(&self) -> &SqliteBackend {
        &self.db
    }

    pub fn channels(&self) -> &Channels {
        &self.channels
    }

    pub fn events(&self) -> &EventBus {
        &self.events
    }

    pub(crate) fn downgrade(&self) -> Weak<AppStateInner> {
        Arc::downgrade(&self.inner)
    }

    pub(crate) fn from_inner(inner: Arc<AppStateInner>) -> Self {
        Self { inner }
    }
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Deref for AppState {
    type Target = AppStateInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
