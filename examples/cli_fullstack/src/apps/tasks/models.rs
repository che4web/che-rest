use che_orm::{Model, NaiveDateTime};
use che_rest::auth::models::User as AuthUser;

#[derive(Debug, Clone, Model)]
#[model(table = "tasks_task")]
pub struct Task {
    #[field(primary_key)]
    pub id: i64,

    #[field(foreign_key = AuthUser)]
    pub author_id: i64,

    pub name: String,

    #[field(auto_now_add)]
    pub created_at: NaiveDateTime,

    #[field(auto_now)]
    pub updated_at: NaiveDateTime,
}
