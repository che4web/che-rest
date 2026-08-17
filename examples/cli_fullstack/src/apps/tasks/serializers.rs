use super::models::{Task, TaskAssigneeRelation, TaskAuthorRelation, TaskStatus};

#[derive(che_orm2::ModelSerializer)]
#[serializer(model = che_rest::auth::models::User)]
pub struct AuthorSerializer {
    #[serializer(read_only)]
    pub id: i64,
    pub username: String,
}

#[derive(che_orm2::ModelSerializer)]
#[serializer(model = Task)]
pub struct TaskSerializer {
    #[serializer(read_only)]
    pub id: i64,
    #[serializer(one = che_rest::auth::models::User, relation = TaskAuthorRelation)]
    pub author: AuthorSerializer,
    #[serializer(foreign_key = che_rest::auth::models::User, relation = TaskAssigneeRelation)]
    pub assignee_id: Option<i64>,
    pub name: String,
    pub status: TaskStatus,
    #[serializer(read_only)]
    pub created_at: time::OffsetDateTime,
    #[serializer(read_only)]
    pub updated_at: time::OffsetDateTime,
}
