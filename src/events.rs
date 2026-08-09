use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use serde_json::Value;
use tokio::sync::broadcast;

use crate::{AppError, AppResult, AppState, auth::CurrentUser};

#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub user: CurrentUser,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct AppEvent {
    pub name: String,
    pub user: Option<CurrentUser>,
    pub payload: Value,
}

#[async_trait::async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    async fn handle(&self, state: &AppState, command: Command) -> AppResult<()>;
}

#[async_trait::async_trait]
pub trait EventHandler: Send + Sync + 'static {
    async fn handle(&self, state: &AppState, event: AppEvent) -> AppResult<()>;
}

#[derive(Clone, Default)]
pub struct EventBus {
    commands: Arc<RwLock<HashMap<String, Arc<dyn CommandHandler>>>>,
    senders: Arc<RwLock<HashMap<String, broadcast::Sender<AppEvent>>>>,
}

impl EventBus {
    pub(crate) fn configure_commands(&self, commands: HashMap<String, Arc<dyn CommandHandler>>) {
        *self.commands.write().expect("event bus lock is poisoned") = commands;
    }

    pub fn subscribe(&self, name: impl Into<String>) -> broadcast::Receiver<AppEvent> {
        let mut senders = self.senders.write().expect("event bus lock is poisoned");
        senders
            .entry(name.into())
            .or_insert_with(|| broadcast::channel(1024).0)
            .subscribe()
    }

    pub async fn dispatch_command(
        &self,
        state: &AppState,
        name: impl Into<String>,
        user: CurrentUser,
        payload: Value,
    ) -> AppResult<()> {
        let name = name.into();
        let handler = self
            .commands
            .read()
            .expect("event bus lock is poisoned")
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

    pub fn emit(&self, event: AppEvent) {
        let sender = self
            .senders
            .read()
            .expect("event bus lock is poisoned")
            .get(&event.name)
            .cloned();
        if let Some(sender) = sender {
            let _ = sender.send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_bus_keeps_multiple_handlers_for_one_event() {
        let bus = EventBus::default();
        let mut first = bus.subscribe("created");
        let mut second = bus.subscribe("created");
        bus.emit(AppEvent {
            name: "created".to_string(),
            user: None,
            payload: serde_json::json!({}),
        });
        assert!(first.try_recv().is_ok());
        assert!(second.try_recv().is_ok());
    }
}
