use super::models::Task;

#[derive(che_orm::ModelSerializer)]
#[serializer(model = Task)]
pub struct TaskSerializer {
    #[serializer(read_only)]
    pub id: i64,
    pub title: String,
    pub completed: bool,
}
