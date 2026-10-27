pub mod apps;
{% if layout == "single" -%}
pub mod filters;
pub mod migrations;
pub mod models;
pub mod serializers;
pub mod views;

use che_rest::{AppModule, ModuleContext};

pub const APP_NAME: &str = "tasks";

pub fn module() -> TasksModule {
    TasksModule
}

pub struct TasksModule;

impl AppModule for TasksModule {
    fn migrations(&self) -> Vec<che_orm::Migration> {
        migrations::all()
    }

    fn migration_path(&self) -> Option<&'static str> {
        Some(concat!(env!("CARGO_MANIFEST_DIR"), "/src/migrations"))
    }

    fn name(&self) -> &'static str {
        APP_NAME
    }

    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new().model::<models::Task>()
    }

    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(views::TaskViewSet);
    }
}
{% endif -%}
