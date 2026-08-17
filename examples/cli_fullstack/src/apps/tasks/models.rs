use che_rest::auth::models::User as AuthUser;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, che_orm2::DbEnum)]
pub enum TaskStatus {
    Draft,
    #[db_enum(rename = "in_progress")]
    InProgress,
    Done,
}

#[derive(Debug, che_orm2::Model)]
#[orm(table = "tasks_task", index("author_id"))]
pub struct Task {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(foreign_key = AuthUser, on_delete = "cascade")]
    pub author_id: i64,
    pub name: String,
    pub status: TaskStatus,
    #[orm(auto_now_add)]
    pub created_at: OffsetDateTime,
    #[orm(auto_now)]
    pub updated_at: OffsetDateTime,
}
