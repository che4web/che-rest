use time::OffsetDateTime;

#[derive(Debug, che_orm::Model)]
#[orm(table = "notifications_notification")]
pub struct Notification {
    #[orm(primary_key)]
    pub id: i64,
    pub message: String,
    #[orm(auto_now_add)]
    pub created_at: OffsetDateTime,
}
