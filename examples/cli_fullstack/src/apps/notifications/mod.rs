pub mod models;

use che_rest::{AppModule, AppState, ModuleContext};

pub fn module() -> NotificationsModule {
    NotificationsModule
}

pub struct NotificationsModule;

impl AppModule for NotificationsModule {
    fn name(&self) -> &'static str {
        "notifications"
    }

    fn init(&self, ctx: &mut ModuleContext) {
        ctx.model::<models::Notification>();
    }

    fn subscribe(&self, state: &AppState) {
        let mut events = state.app_channels().subscribe("tasks.created");
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => println!("tasks.created: {event}"),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        eprintln!("notifications lagged, dropped {count} events")
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn start(&self, state: &AppState) {
        state
            .app_channels()
            .publish("tasks.created", serde_json::json!({ "kind": "startup" }));
    }
}
