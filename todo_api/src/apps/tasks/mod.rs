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

    fn init(&self, ctx: &mut ModuleContext) {
        ctx.viewset_with("/task", views::TaskViewSet);
    }
}
