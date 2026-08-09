use che_orm::{Model, NaiveDateTime};

#[derive(Debug, Clone, Model)]
#[model(table = "notifications_notification")]
pub struct Notification {
    #[field(primary_key)]
    pub id: i64,

    pub message: String,

    #[field(auto_now_add)]
    pub created_at: NaiveDateTime,
}
