//! Disposable benchmark fixture, not application startup or migration history.
use che_orm::{
    Database, Migration, MigrationGraph, MigrationId, MigrationOperation, ProjectState, SchemaSet,
};
use cli_fullstack::apps::tasks::models::Task;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args().nth(1).ok_or("expected SQLite URL")?;
    let database = Database::connect(&url).await?;
    let schema = SchemaSet::new().model::<Task>();
    let state = ProjectState::from_app_schemas([("tasks", &schema)])?;
    let mut migrations = che_rest::auth::migrations();
    migrations.push(Migration::new(
        MigrationId {
            app: "tasks".into(),
            name: "0001_benchmark".into(),
        },
        vec![MigrationId {
            app: "auth".into(),
            name: "0002_access".into(),
        }],
        state
            .models()
            .iter()
            .cloned()
            .map(|model| MigrationOperation::CreateModel { model })
            .collect(),
    ));
    database.migrate(MigrationGraph::new(migrations)?).await?;
    Ok(())
}
