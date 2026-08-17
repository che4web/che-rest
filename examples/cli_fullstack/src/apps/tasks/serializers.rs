use super::models::{Task, TaskAuthorRelation};

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
    pub name: String,
    #[serializer(read_only)]
    pub created_at: time::OffsetDateTime,
    #[serializer(read_only)]
    pub updated_at: time::OffsetDateTime,
}
