use super::PushRuntime;
use super::models::PushDevice;
use crate::{AppError, AppResult, AppState, CurrentPrincipal};
use axum::{
    Extension, Json, Router,
    routing::{get, post},
};
use che_orm::Uuid;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

#[derive(Deserialize)]
pub struct RegisterDevice {
    pub id: Uuid,
    pub token: String,
    pub server_key: String,
}
#[derive(Deserialize)]
pub struct DeviceId {
    pub id: Uuid,
}
pub fn routes(runtime: PushRuntime) -> Router {
    Router::new()
        .route("/push/status/", get(status))
        .route("/push/devices/register/", post(register))
        .route("/push/devices/unregister/", post(unregister))
        .layer(Extension(runtime))
}
fn user(current: Option<Extension<CurrentPrincipal>>) -> AppResult<i64> {
    current
        .map(|current| current.auth_user().id)
        .ok_or_else(|| AppError::Unauthorized("authentication required".into()))
}
async fn status(
    Extension(runtime): Extension<PushRuntime>,
    current: Option<Extension<CurrentPrincipal>>,
) -> AppResult<Json<Value>> {
    user(current)?;
    Ok(Json(json!({"enabled":runtime.config.enabled})))
}
async fn register(
    Extension(runtime): Extension<PushRuntime>,
    Extension(state): Extension<AppState>,
    current: Option<Extension<CurrentPrincipal>>,
    Json(input): Json<RegisterDevice>,
) -> AppResult<Json<Value>> {
    let user_id = user(current)?;
    if !runtime.config.enabled {
        return Err(AppError::BadRequest(
            "Push is not configured on this server".into(),
        ));
    }
    register_device(state.database(), user_id, input, runtime.source).await?;
    Ok(Json(json!({"registered":true})))
}
pub async fn register_device(
    db: &che_orm::Database,
    user_id: i64,
    input: RegisterDevice,
    source: std::sync::Arc<dyn super::PushSource>,
) -> AppResult<()> {
    if input.token.len() < 10
        || input.token.len() > 4096
        || input.token.chars().any(char::is_whitespace)
        || input.server_key.len() != 64
        || !input
            .server_key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || input.id.is_nil()
    {
        return Err(AppError::BadRequest("Invalid push registration".into()));
    }
    db.transaction(move |tx| {
        Box::pin(async move {
            source.initialize(tx).await?;
            let now = OffsetDateTime::now_utc();
            if let Some(mut device) = tx.get::<PushDevice>(input.id).await? {
                let changed = device.user_id != user_id
                    || device.token != input.token
                    || device.server_key != input.server_key
                    || !device.enabled;
                if changed {
                    device.generation += 1;
                    device.registered_at = now;
                }
                device.user_id = user_id;
                device.token = input.token;
                device.server_key = input.server_key;
                device.enabled = true;
                device.last_seen_at = now;
                tx.save(&device).await?;
            } else {
                tx.create::<PushDevice>()
                    .set(PushDevice::ID, input.id)
                    .set(PushDevice::USER_ID, user_id)
                    .set(PushDevice::TOKEN, input.token)
                    .set(PushDevice::SERVER_KEY, input.server_key)
                    .set(PushDevice::ENABLED, true)
                    .set(PushDevice::GENERATION, 1_i64)
                    .set(PushDevice::REGISTERED_AT, now)
                    .set(PushDevice::LAST_SEEN_AT, now)
                    .execute()
                    .await?;
            }
            Ok(())
        })
    })
    .await?;
    Ok(())
}
async fn unregister(
    Extension(state): Extension<AppState>,
    current: Option<Extension<CurrentPrincipal>>,
    Json(input): Json<DeviceId>,
) -> AppResult<Json<Value>> {
    let user_id = user(current)?;
    unregister_device(state.database(), user_id, input.id).await?;
    Ok(Json(json!({"unregistered":true})))
}
pub async fn unregister_device(db: &che_orm::Database, user_id: i64, id: Uuid) -> AppResult<()> {
    db.transaction(move |tx| {
        Box::pin(async move {
            if let Some(mut device) = tx.get::<PushDevice>(id).await?
                && device.user_id == user_id
            {
                device.enabled = false;
                device.generation += 1;
                tx.save(&device).await?;
            }
            Ok(())
        })
    })
    .await?;
    Ok(())
}
