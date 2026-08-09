use std::sync::{Arc, RwLock};
use std::{collections::HashMap, fmt};

use serde_json::Value;

use crate::{AppError, AppResult, AppState, auth::CurrentUser};

#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub user: CurrentUser,
    pub payload: Value,
}

#[async_trait::async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    async fn handle(&self, state: &AppState, command: Command) -> AppResult<()>;
}

#[derive(Clone, Default)]
pub struct Commands {
    handlers: Arc<RwLock<HashMap<String, Arc<dyn CommandHandler>>>>,
}

impl fmt::Debug for Commands {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Commands").finish_non_exhaustive()
    }
}

impl Commands {
    pub(crate) fn from_handlers(handlers: HashMap<String, Arc<dyn CommandHandler>>) -> Self {
        Self {
            handlers: Arc::new(RwLock::new(handlers)),
        }
    }

    pub async fn dispatch(
        &self,
        state: &AppState,
        name: impl Into<String>,
        user: CurrentUser,
        payload: Value,
    ) -> AppResult<()> {
        let name = name.into();
        let handler = self
            .handlers
            .read()
            .expect("command registry lock is poisoned")
            .get(&name)
            .cloned()
            .ok_or_else(|| AppError::BadRequest(format!("unknown command: {name}")))?;
        handler
            .handle(
                state,
                Command {
                    name,
                    user,
                    payload,
                },
            )
            .await
    }
}
