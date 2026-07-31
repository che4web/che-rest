use axum::{Extension, Json, Router, response::IntoResponse, routing::post};
use che_orm::Model;
use serde::Deserialize;
use serde_json::json;

use crate::{AppResult, auth::models::verify_password, state::AppState};

use super::{generate_token, models::AuthToken, models::User, token_hash};

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

pub fn routes() -> Router {
    Router::new().route("/api-token-auth/", post(login))
}

async fn login(
    Extension(state): Extension<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let users = User::objects(state.db())
        .query()
        .eq("username", payload.username)
        .limit(1)
        .all()
        .await?;
    let Some(user) = users.into_iter().next() else {
        return Err(crate::AppError::Unauthorized(
            "unable to log in with provided credentials".to_string(),
        ));
    };

    if !user.is_active || !verify_password(&payload.password, &user.password_hash) {
        return Err(crate::AppError::Unauthorized(
            "unable to log in with provided credentials".to_string(),
        ));
    }

    let token = generate_token();
    AuthToken::objects(state.db())
        .create()
        .set("user_id", user.id)
        .set("key_hash", token_hash(&token))
        .execute()
        .await?;

    Ok(Json(json!({ "token": token })))
}
