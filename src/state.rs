use std::{any::Any, future::Future, path::Path, pin::Pin, sync::Arc};

use che_orm::Database;

use crate::{AppConfig, AppResult, auth::User};
use crate::{app_channels::AppChannels, channels::Channels, signals::SignalBus};

pub type CurrentUserResolver = Arc<
    dyn for<'a> Fn(
            &'a AppState,
            &'a User,
        ) -> Pin<
            Box<dyn Future<Output = AppResult<Option<Arc<dyn Any + Send + Sync>>>> + Send + 'a>,
        > + Send
        + Sync,
>;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    database: Database,
    app_channels: AppChannels,
    signals: SignalBus,
    current_user_resolver: Option<CurrentUserResolver>,
}

impl AppState {
    pub fn from_database(database: Database) -> Self {
        Self {
            config: AppConfig::default(),
            database,
            app_channels: AppChannels::new(),
            signals: SignalBus::new(),
            current_user_resolver: None,
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
            app_channels: AppChannels::new(),
            signals: SignalBus::new(),
            current_user_resolver: None,
        })
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn channels(&self) -> &Channels {
        &self.signals
    }

    pub fn app_channels(&self) -> &AppChannels {
        &self.app_channels
    }

    pub fn signals(&self) -> &SignalBus {
        &self.signals
    }

    pub fn with_current_user_resolver<T, F>(mut self, resolver: F) -> Self
    where
        T: Send + Sync + 'static,
        F: for<'a> Fn(&'a AppState, &'a User) -> CurrentUserResolverFuture<'a, T>
            + Send
            + Sync
            + 'static,
    {
        self.current_user_resolver = Some(Arc::new(move |state, user| {
            let future = resolver(state, user);
            Box::pin(async move {
                future
                    .await
                    .map(|user| user.map(|user| Arc::new(user) as Arc<dyn Any + Send + Sync>))
            })
        }));
        self
    }

    pub(crate) async fn resolve_current_user(
        &self,
        user: &User,
    ) -> AppResult<Option<Arc<dyn Any + Send + Sync>>> {
        match &self.current_user_resolver {
            Some(resolver) => resolver(self, user).await,
            None => Ok(None),
        }
    }
}

pub type CurrentUserResolverFuture<'a, T> =
    Pin<Box<dyn Future<Output = AppResult<Option<T>>> + Send + 'a>>;

fn sqlite_path(url: &str) -> String {
    url.strip_prefix("sqlite://")
        .and_then(|value| value.split('?').next())
        .filter(|value| !value.is_empty())
        .unwrap_or(url)
        .to_string()
}
