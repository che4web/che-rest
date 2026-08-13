pub mod models;
pub mod views;

use axum::{
    Extension,
    body::Body,
    extract::{FromRequestParts, Request},
    http::{Method, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use che_orm::chrono::{Duration, Utc};
use rand::RngCore;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{AppError, AppModule, AppResult, ModuleContext, state::AppState};

use self::models::{AuthSession, AuthSessionFields, AuthToken, AuthTokenFields, User};

#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub is_staff: bool,
    pub is_admin: bool,
    pub is_superuser: bool,
}

#[derive(Debug, Clone)]
pub struct CurrentSession {
    pub id: i64,
    pub user_id: i64,
    pub key_hash: String,
    pub csrf_hash: String,
    pub data: Value,
    pub revision: i64,
}

pub trait SessionData: Serialize + DeserializeOwned + Default + Send + Sync + 'static {}

pub struct Session<T: SessionData> {
    pub id: i64,
    pub user_id: i64,
    pub data: T,
    state: AppState,
    revision: i64,
}

impl<T: SessionData> Session<T> {
    pub async fn save(&mut self) -> AppResult<()> {
        let data = serde_json::to_string(&self.data)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        let result = sqlx::query(
            "UPDATE auth_sessions SET data = ?1, revision = revision + 1 WHERE id = ?2 AND revision = ?3",
        )
        .bind(data)
        .bind(self.id)
        .bind(self.revision)
        .execute(self.state.db().pool())
        .await
        .map_err(|error| AppError::Orm(error.into()))?;
        if result.rows_affected() != 1 {
            return Err(AppError::BadRequest(
                "session was modified by another request".to_string(),
            ));
        }
        self.revision += 1;
        Ok(())
    }
}

impl<S, T> FromRequestParts<S> for Session<T>
where
    S: Send + Sync,
    T: SessionData,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let current = parts
            .extensions
            .get::<CurrentSession>()
            .cloned()
            .ok_or_else(|| {
                AppError::Unauthorized("an authenticated session is required".to_string())
            });
        let state =
            parts.extensions.get::<AppState>().cloned().ok_or_else(|| {
                AppError::BadRequest("application state is unavailable".to_string())
            });
        async move { deserialize_session(current?, state?) }
    }
}

#[derive(Clone, Copy)]
pub struct AuthModule;

pub fn module() -> AuthModule {
    AuthModule
}

pub fn module_with_session<T: SessionData>() -> AuthModule {
    let _ = std::marker::PhantomData::<T>;
    AuthModule
}

impl AppModule for AuthModule {
    fn name(&self) -> &'static str {
        "auth"
    }

    fn init(&self, ctx: &mut ModuleContext) {
        ctx.model::<User>();
        ctx.model::<AuthToken>();
        ctx.model::<AuthSession>();
        ctx.enable_auth();
    }
}

pub async fn auth_middleware(
    Extension(state): Extension<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(token) = request_token(&request) {
        let Some(user) = load_token_user(&state, &token).await else {
            return unauthorized("invalid authentication token");
        };
        request.extensions_mut().insert(current_user(&user));
        return next.run(request).await;
    }

    let Some(session_key) = request_cookie(&request, &state.config.auth.session.cookie_name) else {
        return next.run(request).await;
    };
    let Some((session, user)) = load_session(&state, &session_key).await else {
        return unauthorized("invalid authentication session");
    };
    request.extensions_mut().insert(current_user(&user));
    request.extensions_mut().insert(session.clone());
    if is_unsafe_method(request.method()) && !valid_csrf(&request, &session) {
        return forbidden("CSRF validation failed");
    }
    next.run(request).await
}

async fn load_token_user(state: &AppState, token: &str) -> Option<User> {
    let tokens = state
        .db()
        .query::<AuthToken>()
        .filter(AuthTokenFields::KEY_HASH.eq(token_hash(token)))
        .limit(1)
        .all()
        .await
        .ok()?;
    let auth_token = tokens.into_iter().next()?;
    let user = state.db().get::<User>(auth_token.user_id).await.ok()?;
    user.is_active.then_some(user)
}

fn current_user(user: &User) -> CurrentUser {
    CurrentUser {
        id: user.id,
        username: user.username.clone(),
        is_staff: user.is_staff,
        is_admin: user.is_admin,
        is_superuser: user.is_superuser,
    }
}

pub(crate) async fn load_session(state: &AppState, key: &str) -> Option<(CurrentSession, User)> {
    let sessions = state
        .db()
        .query::<AuthSession>()
        .filter(AuthSessionFields::KEY_HASH.eq(token_hash(key)))
        .limit(1)
        .all()
        .await
        .ok()?;
    let session = sessions.into_iter().next()?;
    if session.expires_at <= Utc::now().naive_utc() {
        return None;
    }
    let data = serde_json::from_str(&session.data).ok()?;
    let user = state.db().get::<User>(session.user_id).await.ok()?;
    if !user.is_active {
        return None;
    }
    Some((
        CurrentSession {
            id: session.id,
            user_id: session.user_id,
            key_hash: session.key_hash,
            csrf_hash: session.csrf_hash,
            data,
            revision: session.revision,
        },
        user,
    ))
}

fn is_unsafe_method(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

fn valid_csrf(request: &Request<Body>, session: &CurrentSession) -> bool {
    request
        .headers()
        .get("X-CSRF-Token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| token_hash(value) == session.csrf_hash)
}

pub(crate) fn session_expiry(state: &AppState) -> che_orm::NaiveDateTime {
    (Utc::now() + Duration::seconds(state.config.auth.session.ttl_seconds)).naive_utc()
}

fn request_cookie(request: &Request<Body>, name: &str) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|item| item.strip_prefix(&format!("{name}=")).map(str::to_string))
}

pub(crate) fn cookie_header(
    state: &AppState,
    name: &str,
    value: &str,
    max_age: i64,
    http_only: bool,
) -> String {
    let mut result = format!(
        "{name}={value}; Path=/; Max-Age={max_age}; SameSite={}",
        state.config.auth.session.same_site
    );
    if http_only {
        result.push_str("; HttpOnly");
    }
    if state.config.auth.session.secure {
        result.push_str("; Secure");
    }
    result
}

pub(crate) fn generate_secret() -> String {
    generate_token()
}

pub(crate) fn deserialize_session<T: SessionData>(
    current: CurrentSession,
    state: AppState,
) -> AppResult<Session<T>> {
    let data = serde_json::from_value(current.data)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    Ok(Session {
        id: current.id,
        user_id: current.user_id,
        data,
        state,
        revision: current.revision,
    })
}

pub async fn admin_required_middleware(request: Request<Body>, next: Next) -> Response {
    let is_admin = request
        .extensions()
        .get::<CurrentUser>()
        .is_some_and(|user| user.is_admin || user.is_superuser);

    if !is_admin {
        return forbidden("admin permissions are required");
    }

    next.run(request).await
}

pub fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn request_token(request: &Request<Body>) -> Option<String> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    value
        .strip_prefix("Token ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn unauthorized(detail: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Token")],
        axum::Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn forbidden(detail: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(json!({ "detail": detail })),
    )
        .into_response()
}
