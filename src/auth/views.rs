use axum::{
    Extension, Json, Router,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};

use super::{
    AuthSession, AuthToken, CurrentSession, CurrentUser, User, cookie_header, generate_token,
    token_hash, verify_password,
};
use crate::{AppError, AppResult, AppState};

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

async fn login(
    Extension(state): Extension<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let user = find_user(&state, payload).await?;
    let token = generate_token();
    state
        .database()
        .create::<AuthToken>()
        .set(AuthToken::USER_ID, user.id)
        .set(AuthToken::KEY_HASH, token_hash(&token))
        .execute()
        .await?;
    Ok(Json(json!({ "token": token })))
}

async fn session_login(
    Extension(state): Extension<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Response> {
    let user = find_user(&state, payload).await?;
    let session_key = generate_token();
    let csrf_token = generate_token();
    let config = &state.config.auth.session;
    let session_ttl = if config.absolute_ttl_seconds > 0 {
        config.ttl_seconds.min(config.absolute_ttl_seconds)
    } else {
        config.ttl_seconds
    };
    let expires_at = OffsetDateTime::now_utc() + Duration::seconds(session_ttl);
    state
        .database()
        .create::<AuthSession>()
        .set(AuthSession::USER_ID, user.id)
        .set(AuthSession::KEY_HASH, token_hash(&session_key))
        .set(AuthSession::CSRF_HASH, token_hash(&csrf_token))
        .set(AuthSession::DATA, "{}")
        .set(AuthSession::REVISION, 0_i64)
        .set(AuthSession::EXPIRES_AT, expires_at)
        .execute()
        .await?;

    let mut response = Json(json!({
        "user": user_payload(&user),
        "expires_at": expires_at,
    }))
    .into_response();
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
    session: Option<Extension<CurrentSession>>,
) -> AppResult<Response> {
    if let Some(Extension(session)) = session {
        state.database().delete::<AuthSession>(session.id).await?;
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
    user: Option<Extension<CurrentUser>>,
    session: Option<Extension<CurrentSession>>,
) -> AppResult<impl IntoResponse> {
    let user = user.ok_or(AppError::Unauthorized("invalid credentials".into()))?;
    Ok(Json(json!({
        "user": user.0,
        "session": session.map(|session| json!({
            "id": session.0.id,
            "user_id": session.0.user_id,
            "revision": session.0.revision,
            "expires_at": session.0.expires_at,
        }))
    })))
}

async fn find_user(state: &AppState, payload: LoginRequest) -> AppResult<User> {
    let user = state
        .database()
        .query::<User>()
        .filter(User::USERNAME.eq(payload.username))
        .first(state.database())
        .await?
        .ok_or(AppError::Unauthorized("invalid session".into()))?;
    if !user.is_active || !verify_password(&payload.password, &user.password_hash) {
        return Err(AppError::Unauthorized("invalid session".into()));
    }
    Ok(user)
}

fn user_payload(user: &User) -> serde_json::Value {
    json!({
        "id": user.id,
        "username": user.username,
        "is_staff": user.is_staff,
        "is_admin": user.is_admin,
        "is_superuser": user.is_superuser,
    })
}

fn set_cookie(response: &mut Response, value: String) -> AppResult<()> {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&value).map_err(|error| AppError::BadRequest(error.to_string()))?,
    );
    Ok(())
}
