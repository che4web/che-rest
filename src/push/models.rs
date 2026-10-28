//! Server-only delivery infrastructure: never part of the domain sync registry.
use che_orm::{Model, Uuid};
use time::OffsetDateTime;

#[derive(Debug, Clone, Model)]
#[orm(table = "push_devices", index("user_id"))]
pub struct PushDevice {
    #[orm(primary_key)]
    pub id: Uuid,
    pub user_id: i64,
    pub token: String,
    pub server_key: String,
    pub enabled: bool,
    pub generation: i64,
    pub registered_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
}
#[derive(Debug, Clone, Model)]
#[orm(table = "push_deliveries", index("state"), index("next_attempt_at"))]
pub struct PushDelivery {
    #[orm(primary_key)]
    pub id: Uuid,
    #[orm(unique)]
    pub delivery_key: String,
    pub device_id: Uuid,
    pub device_generation: i64,
    pub user_id: i64,
    pub target_type: String,
    pub target_id: String,
    pub title: String,
    pub body: String,
    pub data: String,
    pub kind: String,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub lease_until: i64,
    pub lease_token: String,
    pub last_error: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Model)]
#[orm(table = "push_cursors")]
pub struct PushCursor {
    #[orm(primary_key)]
    pub id: Uuid,
    #[orm(unique)]
    pub stream: String,
    pub position: i64,
    pub history: String,
}
