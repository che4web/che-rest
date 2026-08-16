pub mod filters;
pub mod models;
pub mod serializers;
pub mod views;

use che_rest::{AppModule, ModuleContext};

pub fn module() -> TasksModule {
    TasksModule
}

pub struct TasksModule;

impl AppModule for TasksModule {
    fn name(&self) -> &'static str {
        "tasks"
    }

    fn schema(&self) -> che_orm2::SchemaSet {
        che_orm2::SchemaSet::new().model::<models::Task>()
    }

    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with("/tasks", views::TaskViewSet);
    }
}
