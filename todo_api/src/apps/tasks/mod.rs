mod filters;
mod models;
mod serializers;
mod views;

pub use models::Task;

pub struct Tasks;

pub fn module() -> Tasks {
    Tasks
}

impl che_rest::AppModule for Tasks {
    fn name(&self) -> &'static str {
        "tasks"
    }

    fn schema(&self) -> che_orm2::SchemaSet {
        che_orm2::SchemaSet::new().model::<Task>()
    }

    fn init(&self, context: &mut che_rest::ModuleContext) {
        context.viewset_with("/tasks", views::TaskViewSet);
    }
}
