pub mod models;
pub mod views;

use axum::{
    body::Body,
    extract::State,
    http::{Method, Request, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::{AppModule, AppState, ModuleContext, Permission, ViewAction};

pub use models::{AuthSession, AuthToken, User, hash_password, verify_password};

#[derive(Debug, Clone, serde::Serialize)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub is_staff: bool,
    pub is_admin: bool,
    pub is_superuser: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IsAuthenticated;

impl<M: che_orm2::Model> Permission<M> for IsAuthenticated {
    fn check(
        &self,
        _state: &AppState,
        user: Option<&CurrentUser>,
        _action: ViewAction,
    ) -> crate::AppResult<()> {
        user.map(|_| ()).ok_or(crate::AppError::Unauthorized(
            "authentication credentials were not provided".into(),
        ))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IsAdminUser;

impl<M: che_orm2::Model> Permission<M> for IsAdminUser {
    fn check(
        &self,
        _state: &AppState,
        user: Option<&CurrentUser>,
        _action: ViewAction,
    ) -> crate::AppResult<()> {
        if user.is_some_and(|user| user.is_admin || user.is_superuser) {
            Ok(())
        } else {
            Err(crate::AppError::Forbidden(
                "admin permissions are required".into(),
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct CurrentSession {
    pub id: i64,
    pub user_id: i64,
    pub csrf_hash: String,
    pub revision: i64,
}

#[derive(Clone, Copy)]
pub struct AuthModule;

pub fn module() -> AuthModule {
    AuthModule
}

impl AppModule for AuthModule {
    fn name(&self) -> &'static str {
        "auth"
    }

    fn schema(&self) -> che_orm2::SchemaSet {
        che_orm2::SchemaSet::new()
            .model::<User>()
            .model::<AuthToken>()
            .model::<AuthSession>()
    }

    fn init(&self, context: &mut ModuleContext) {
        context.route_at_root(views::routes());
    }

    fn middleware(&self, router: axum::Router, _state: &AppState) -> axum::Router {
        router.layer(axum::middleware::from_fn_with_state(
            _state.clone(),
            auth_middleware,
        ))
    }
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(token) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Token "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Ok(Some(auth_token)) = state
            .database()
            .query::<AuthToken>()
            .filter(AuthToken::KEY_HASH.eq(token_hash(token)))
            .first(state.database())
            .await
        else {
            return crate::AppError::Unauthorized("invalid token".into()).into_response();
        };
        let Ok(Some(user)) = state.database().get::<User>(auth_token.user_id).await else {
            return crate::AppError::Unauthorized("invalid token".into()).into_response();
        };
        if !user.is_active {
            return crate::AppError::Unauthorized("inactive user".into()).into_response();
        }
        request.extensions_mut().insert(current_user(&user));
        return next.run(request).await;
    }

    let Some(session_key) = cookie(&request, &state.config.auth.session.cookie_name) else {
        return next.run(request).await;
    };
    let Some((session, user)) = load_session(&state, &session_key).await else {
        return next.run(request).await;
    };
    if unsafe_method(request.method()) && !csrf_valid(&request, &session) {
        return crate::AppError::Forbidden("CSRF validation failed".into()).into_response();
    }
    request.extensions_mut().insert(current_user(&user));
    request.extensions_mut().insert(session);
    next.run(request).await
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

async fn load_session(state: &AppState, key: &str) -> Option<(CurrentSession, User)> {
    let session = state
        .database()
        .query::<AuthSession>()
        .filter(AuthSession::KEY_HASH.eq(token_hash(key)))
        .first(state.database())
        .await
        .ok()??;
    if session.expires_at <= OffsetDateTime::now_utc() {
        return None;
    }
    let user = state.database().get::<User>(session.user_id).await.ok()??;
    if !user.is_active {
        return None;
    }
    Some((
        CurrentSession {
            id: session.id,
            user_id: session.user_id,
            csrf_hash: session.csrf_hash,
            revision: session.revision,
        },
        user,
    ))
}

fn cookie(request: &Request<Body>, name: &str) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|item| item.strip_prefix(&format!("{name}=")).map(str::to_owned))
}

fn unsafe_method(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

fn csrf_valid(request: &Request<Body>, session: &CurrentSession) -> bool {
    request
        .headers()
        .get("X-CSRF-Token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| token_hash(value) == session.csrf_hash)
}

fn cookie_header(
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

pub fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    #[test]
    fn password_hash_round_trip_and_token_hash_are_stable() {
        let hash = hash_password("secret").unwrap();
        assert!(verify_password("secret", &hash));
        assert!(!verify_password("wrong", &hash));
        assert_eq!(token_hash("token"), token_hash("token"));
        assert_ne!(token_hash("token"), token_hash("other"));
    }

    #[test]
    fn csrf_requires_matching_token_header() {
        let token = "csrf-token";
        let session = CurrentSession {
            id: 1,
            user_id: 2,
            csrf_hash: token_hash(token),
            revision: 0,
        };
        let request = Request::builder()
            .header("X-CSRF-Token", token)
            .body(Body::empty())
            .unwrap();
        assert!(csrf_valid(&request, &session));

        let request = Request::builder()
            .header("X-CSRF-Token", "wrong-token")
            .body(Body::empty())
            .unwrap();
        assert!(!csrf_valid(&request, &session));
    }

    #[test]
    fn cookie_parser_handles_multiple_cookies() {
        let request = Request::builder()
            .header(header::COOKIE, "other=value; che_rest_session=session-key")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            cookie(&request, "che_rest_session"),
            Some("session-key".into())
        );
        assert_eq!(cookie(&request, "missing"), None);
    }

    #[test]
    fn admin_permission_requires_admin_or_superuser() {
        let state = AppState::from_database(che_orm2::Database::connect(":memory:").unwrap());
        let regular = CurrentUser {
            id: 1,
            username: "regular".into(),
            is_staff: false,
            is_admin: false,
            is_superuser: false,
        };
        assert!(
            <IsAdminUser as Permission<User>>::check(
                &IsAdminUser,
                &state,
                Some(&regular),
                ViewAction::List,
            )
            .is_err()
        );

        let admin = CurrentUser {
            is_admin: true,
            ..regular
        };
        assert!(
            <IsAdminUser as Permission<User>>::check(
                &IsAdminUser,
                &state,
                Some(&admin),
                ViewAction::List,
            )
            .is_ok()
        );
    }
}
