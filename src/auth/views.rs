use axum::{
    Extension, Json, Router,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};
use tokio::sync::Semaphore;

use super::{
    AuthSession, AuthToken, CurrentPrincipal, CurrentSession, User, cookie_header, generate_token,
    token_hash, verify_password,
};
use crate::{AppError, AppResult, AppState};

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
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

pub fn api_routes() -> Router {
    Router::new().route("/auth/users/create/", post(create_user))
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
    current: Option<Extension<CurrentPrincipal>>,
    session: Option<Extension<CurrentSession>>,
) -> AppResult<impl IntoResponse> {
    let current = current.ok_or(AppError::Unauthorized("invalid credentials".into()))?;
    Ok(Json(json!({
        "user": current.0.auth_user(),
        "session": session.map(|session| json!({
            "id": session.0.id,
            "user_id": session.0.user_id,
            "revision": session.0.revision,
            "expires_at": session.0.expires_at,
        }))
    })))
}

async fn create_user(
    Extension(state): Extension<AppState>,
    current: Option<Extension<CurrentPrincipal>>,
    Json(payload): Json<CreateUserRequest>,
) -> AppResult<impl IntoResponse> {
    let current =
        current.ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
    let user = current.0.auth_user();
    if !user.is_admin && !user.is_superuser {
        return Err(AppError::Forbidden("admin permissions are required".into()));
    }

    let username = payload.username.trim();
    if username.is_empty() {
        return Err(AppError::BadRequest("username is required".into()));
    }
    if payload.password.is_empty() {
        return Err(AppError::BadRequest("password is required".into()));
    }
    if state
        .database()
        .query::<User>()
        .filter(User::USERNAME.eq(username.to_owned()))
        .first(state.database())
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest("username is already taken".into()));
    }

    let password_hash = password_work(move || super::hash_password(&payload.password))
        .await?
        .map_err(|error| AppError::BadRequest(format!("could not hash password: {error}")))?;
    let created = state
        .database()
        .create::<User>()
        .set(User::USERNAME, username.to_owned())
        .set(User::PASSWORD_HASH, password_hash)
        .set(User::IS_ACTIVE, true)
        .set(User::IS_STAFF, false)
        .set(User::IS_ADMIN, false)
        .set(User::IS_SUPERUSER, false)
        .execute()
        .await?;

    Ok((StatusCode::CREATED, Json(user_payload(&created))))
}

async fn find_user(state: &AppState, payload: LoginRequest) -> AppResult<User> {
    let user = state
        .database()
        .query::<User>()
        .filter(User::USERNAME.eq(payload.username))
        .first(state.database())
        .await?
        .ok_or(AppError::Unauthorized("invalid session".into()))?;
    if !user.is_active {
        return Err(AppError::Unauthorized("invalid session".into()));
    }
    let password_hash = user.password_hash.clone();
    if !password_work(move || verify_password(&payload.password, &password_hash)).await? {
        return Err(AppError::Unauthorized("invalid session".into()));
    }
    Ok(user)
}

// Bound hashing and verification together: Argon2 is both CPU- and memory-intensive.
static PASSWORD_WORKERS: Semaphore = Semaphore::const_new(4);

async fn password_work<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> AppResult<T> {
    password_work_limited(&PASSWORD_WORKERS, work).await
}

async fn password_work_limited<T: Send + 'static>(
    workers: &'static Semaphore,
    work: impl FnOnce() -> T + Send + 'static,
) -> AppResult<T> {
    let permit = workers
        .acquire()
        .await
        .expect("password worker semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        // Retain the permit even if the HTTP request is cancelled while hashing.
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| {
        AppError::Io(std::io::Error::other(format!(
            "password worker failed: {error}"
        )))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn password_work_is_offloaded_and_cancelled_requests_keep_their_permit() {
        static WORKERS: Semaphore = Semaphore::const_new(1);
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let first = tokio::spawn(password_work_limited(&WORKERS, move || {
            started.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }));
        tokio::time::timeout(std::time::Duration::from_secs(2), ready)
            .await
            .unwrap()
            .unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        let (started, mut ready) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(password_work_limited(&WORKERS, move || {
            started.send(()).unwrap();
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut ready)
                .await
                .is_err()
        );
        // This send can run on the single async worker while hashing is blocked.
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), ready)
            .await
            .unwrap()
            .unwrap();
        second.await.unwrap().unwrap();
    }
}
