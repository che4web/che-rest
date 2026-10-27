use time::OffsetDateTime;

#[derive(Debug, che_orm::Model)]
#[orm(app = super::APP_NAME)]
pub struct Task {
    #[orm(primary_key)]
    pub id: i64,
    pub name: String,
    #[orm(auto_now_add)]
    pub created_at: OffsetDateTime,
    #[orm(auto_now)]
    pub updated_at: OffsetDateTime,
}
