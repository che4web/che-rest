use axum::{
    Extension, Json, Router,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use che_orm::Model;
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppError, AppResult,
    auth::{cookie_header, generate_secret, load_session, session_expiry},
    state::AppState,
};

use super::{
    generate_token,
    models::{AuthToken, User, verify_password},
    token_hash,
};

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

pub fn routes() -> Router {
    Router::new()
        .route("/api-token-auth/", post(login))
        .route("/api-session-auth/login/", post(session_login))
        .route("/api-session-auth/logout/", post(session_logout))
        .route("/api-session-auth/me/", get(session_me))
}

async fn session_login(
    Extension(state): Extension<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Response> {
    let users = User::objects(state.db())
        .query()
        .eq("username", payload.username)
        .limit(1)
        .all()
        .await?;
    let Some(user) = users.into_iter().next() else {
        return Err(AppError::Unauthorized(
            "unable to log in with provided credentials".to_string(),
        ));
    };
    if !user.is_active || !verify_password(&payload.password, &user.password_hash) {
        return Err(AppError::Unauthorized(
            "unable to log in with provided credentials".to_string(),
        ));
    }

    let session_key = generate_secret();
    let csrf_token = generate_secret();
    sqlx::query("INSERT INTO auth_sessions (user_id, key_hash, csrf_hash, data, revision, expires_at) VALUES (?1, ?2, ?3, '{}', 0, ?4)")
        .bind(user.id).bind(super::token_hash(&session_key)).bind(super::token_hash(&csrf_token)).bind(session_expiry(&state))
        .execute(state.db().pool()).await.map_err(|error| AppError::Orm(error.into()))?;
    let mut response = Json(json!({ "user": user_payload(&user) })).into_response();
    set_cookie(
        &mut response,
        cookie_header(
            &state,
            &state.config.auth.session.cookie_name,
            &session_key,
            state.config.auth.session.ttl_seconds,
            true,
        ),
    )?;
    set_cookie(
        &mut response,
        cookie_header(
            &state,
            &state.config.auth.session.csrf_cookie_name,
            &csrf_token,
            state.config.auth.session.ttl_seconds,
            false,
        ),
    )?;
    Ok(response)
}

async fn session_logout(
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
) -> AppResult<Response> {
    if let Some(key) = header_cookie(&headers, &state.config.auth.session.cookie_name) {
        if let Some((session, _)) = load_session(&state, &key).await {
            let csrf_valid = headers
                .get("X-CSRF-Token")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| token_hash(value) == session.csrf_hash);
            if !csrf_valid {
                return Err(AppError::Forbidden("CSRF validation failed".to_string()));
            }
            sqlx::query("DELETE FROM auth_sessions WHERE id = ?1")
                .bind(session.id)
                .execute(state.db().pool())
                .await
                .map_err(|error| AppError::Orm(error.into()))?;
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    set_cookie(
        &mut response,
        cookie_header(&state, &state.config.auth.session.cookie_name, "", 0, true),
    )?;
    set_cookie(
        &mut response,
        cookie_header(
            &state,
            &state.config.auth.session.csrf_cookie_name,
            "",
            0,
            false,
        ),
    )?;
    Ok(response)
}

async fn session_me(
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let key = header_cookie(&headers, &state.config.auth.session.cookie_name).ok_or_else(|| {
        AppError::Unauthorized("an authenticated session is required".to_string())
    })?;
    let Some((session, user)) = load_session(&state, &key).await else {
        return Err(AppError::Unauthorized(
            "an authenticated session is required".to_string(),
        ));
    };
    Ok(Json(json!({ "user": user_payload(&user), "data": session.data })).into_response())
}

fn user_payload(user: &User) -> serde_json::Value {
    json!({ "id": user.id, "username": user.username, "is_staff": user.is_staff, "is_admin": user.is_admin, "is_superuser": user.is_superuser })
}

fn header_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|item| item.strip_prefix(&format!("{name}=")).map(str::to_string))
}

fn set_cookie(response: &mut Response, value: String) -> AppResult<()> {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&value).map_err(|error| AppError::BadRequest(error.to_string()))?,
    );
    Ok(())
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
