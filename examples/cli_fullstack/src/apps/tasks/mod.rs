pub mod filters;
pub mod models;
pub mod serializers;
pub mod views;

use che_orm::ModelEvent;
use che_rest::{AppModule, AppResult, AppState, Command, CommandHandler, ModuleContext};

pub fn module() -> TasksModule {
    TasksModule
}

pub struct TasksModule;

struct CreateTask;

#[che_rest::async_trait]
impl CommandHandler for CreateTask {
    async fn handle(&self, state: &AppState, command: Command) -> AppResult<()> {
        let name = command
            .payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| che_rest::AppError::BadRequest("name is required".to_string()))?;

        state
            .db()
            .create::<models::Task>()
            .set("author_id", command.user.id)
            .set("name", name)
            .execute()
            .await?;
        Ok(())
    }
}

impl AppModule for TasksModule {
    fn name(&self) -> &'static str {
        "tasks"
    }

    fn init(&self, ctx: &mut ModuleContext) {
        ctx.viewset_with("/tasks", views::TaskViewSet);
        ctx.command_handler("tasks.create", CreateTask);
    }

    fn subscribe(&self, state: &AppState) {
        let mut signals = state.db().signals().subscribe::<models::Task>();
        let channels = state.app_channels().clone();

        tokio::spawn(async move {
            loop {
                match signals.recv().await {
                    Ok(ModelEvent::PostSave(event)) if event.created => {
                        channels.publish("tasks.created", event.object);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        eprintln!("tasks signal bridge lagged, dropped {count} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}
