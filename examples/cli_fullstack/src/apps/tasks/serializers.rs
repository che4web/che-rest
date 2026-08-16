use super::models::Task;

#[derive(che_orm2::ModelSerializer)]
#[serializer(model = Task)]
pub struct TaskSerializer {
    #[serializer(read_only)]
    pub id: i64,
    pub author_id: i64,
    pub name: String,
    #[serializer(read_only)]
    pub created_at: time::OffsetDateTime,
    #[serializer(read_only)]
    pub updated_at: time::OffsetDateTime,
}
