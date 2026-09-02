pub mod models;
pub mod views;

use std::{any::Any, sync::Arc};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Method, Request, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use che_orm::Model;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::{
    AppModule, AppState, Filter, FilterSetSpec, ModuleContext, Permission, ViewAction, ViewSet,
};

pub use models::{AuthSession, AuthToken, User, hash_password, verify_password};

#[derive(che_orm::ModelSerializer)]
#[serializer(model = User)]
pub struct AdminUserSerializer {
    #[serializer(read_only)]
    pub id: i64,
    #[serializer(read_only)]
    pub username: String,
}

#[derive(Clone, Copy, Default)]
pub struct AdminUserViewSet;

#[derive(Clone, Copy, Default)]
pub struct AdminUserFilterSet;

static ADMIN_USER_FILTERS: &[Filter<User>] = &[Filter::contains(User::USERNAME)];

impl FilterSetSpec for AdminUserFilterSet {
    type Model = User;

    fn filters(&self) -> &'static [Filter<Self::Model>] {
        ADMIN_USER_FILTERS
    }
}

impl ViewSet for AdminUserViewSet {
    type Model = User;
    type Serializer = AdminUserSerializer;
    type QuerySet = che_orm::DatabaseQuery<User>;
    type FilterSet = AdminUserFilterSet;
    type Permission = ReadOnlyAdminUser;

    fn get_queryset(&self) -> Self::QuerySet {
        che_orm::DatabaseQuery::new(User::query())
    }

    fn path(&self) -> &'static str {
        "/auth/users"
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub is_staff: bool,
    pub is_admin: bool,
    pub is_superuser: bool,
}

#[derive(Clone)]
pub struct CurrentPrincipal {
    auth_user: CurrentUser,
    app_user: Option<Arc<dyn Any + Send + Sync>>,
}

impl CurrentPrincipal {
    pub fn auth_user(&self) -> &CurrentUser {
        &self.auth_user
    }

    pub fn app<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.app_user
            .as_deref()
            .and_then(|user| user.downcast_ref::<T>())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IsAuthenticated;

impl<M: che_orm::Model> Permission<M> for IsAuthenticated {
    fn check(
        &self,
        _state: &AppState,
        current: Option<&CurrentPrincipal>,
        _action: ViewAction,
    ) -> crate::AppResult<()> {
        current.map(|_| ()).ok_or(crate::AppError::Unauthorized(
            "authentication credentials were not provided".into(),
        ))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IsAdminUser;

impl<M: che_orm::Model> Permission<M> for IsAdminUser {
    fn check(
        &self,
        _state: &AppState,
        current: Option<&CurrentPrincipal>,
        _action: ViewAction,
    ) -> crate::AppResult<()> {
        if current.is_some_and(|current| {
            let user = current.auth_user();
            user.is_admin || user.is_superuser
        }) {
            Ok(())
        } else {
            Err(crate::AppError::Forbidden(
                "admin permissions are required".into(),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOnlyAdminUser;

impl<M: che_orm::Model> Permission<M> for ReadOnlyAdminUser {
    fn check(
        &self,
        state: &AppState,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
    ) -> crate::AppResult<()> {
        <IsAdminUser as Permission<M>>::check(&IsAdminUser, state, current, action)?;
        if matches!(action, ViewAction::List | ViewAction::Retrieve) {
            Ok(())
        } else {
            Err(crate::AppError::Forbidden(
                "user relation endpoint is read-only".into(),
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
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
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

    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new()
            .model::<User>()
            .model::<AuthToken>()
            .model::<AuthSession>()
    }

    fn init(&self, context: &mut ModuleContext) {
        context.route_at_root(views::routes());
        context.route(views::api_routes());
        context.viewset_with(AdminUserViewSet);
    }

    fn middleware(&self, router: axum::Router, _state: &AppState) -> axum::Router {
        router.layer(axum::middleware::from_fn_with_state(
            _state.clone(),
            auth_middleware,
        ))
    }

    fn openapi(&self, state: &AppState) -> serde_json::Value {
        serde_json::json!({
            "paths": {
                "/api-token-auth/": {"post": {"servers": [{"url": "/"}], "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LoginRequest"}}}}, "responses": {"200": {"description": "Token issued"}, "401": {"$ref": "#/components/responses/Unauthorized"}}}},
                "/api-session-auth/login/": {"post": {"servers": [{"url": "/"}], "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LoginRequest"}}}}, "responses": {"200": {"description": "Session cookie issued"}, "401": {"$ref": "#/components/responses/Unauthorized"}}}},
                "/api-session-auth/logout/": {"post": {"servers": [{"url": "/"}], "responses": {"204": {"description": "Logged out"}}}},
                "/api-session-auth/me/": {"get": {"servers": [{"url": "/"}], "security": [{"SessionCookie": []}], "responses": {"200": {"description": "Current session"}, "401": {"$ref": "#/components/responses/Unauthorized"}}}}
            },
            "components": {
                "schemas": {
                    "LoginRequest": {"type": "object", "required": ["username", "password"], "properties": {"username": {"type": "string"}, "password": {"type": "string", "format": "password"}}}
                },
                "responses": {
                    "Unauthorized": {"description": "Unauthorized", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}}
                },
                "securitySchemes": {
                    "TokenAuth": {"type": "apiKey", "in": "header", "name": "Authorization", "description": "Use the format: Token <key>"},
                    "SessionCookie": {"type": "apiKey", "in": "cookie", "name": state.config.auth.session.cookie_name},
                    "CsrfToken": {"type": "apiKey", "in": "header", "name": "X-CSRF-Token"}
                }
            }
        })
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
        let principal = match current_principal(&state, &user).await {
            Ok(principal) => principal,
            Err(error) => return error.into_response(),
        };
        request.extensions_mut().insert(principal);
        return next.run(request).await;
    }

    let Some(session_key) = cookie(&request, &state.config.auth.session.cookie_name) else {
        return next.run(request).await;
    };
    let Some((session, user)) = load_session(&state, &session_key).await else {
        return next.run(request).await;
    };
    let csrf_token = cookie(&request, &state.config.auth.session.csrf_cookie_name)
        .filter(|token| token_hash(token) == session.csrf_hash);
    if unsafe_method(request.method()) && !csrf_valid(&request, &session) {
        return crate::AppError::Forbidden("CSRF validation failed".into()).into_response();
    }
    let principal = match current_principal(&state, &user).await {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };
    request.extensions_mut().insert(principal);
    request.extensions_mut().insert(session.clone());
    let mut response = next.run(request).await;
    renew_session(
        &state,
        &session,
        &session_key,
        csrf_token.as_deref(),
        &mut response,
    )
    .await;
    response
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

async fn current_principal(state: &AppState, user: &User) -> crate::AppResult<CurrentPrincipal> {
    Ok(CurrentPrincipal {
        auth_user: current_user(user),
        app_user: state.resolve_current_user(user).await?,
    })
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
            created_at: session.created_at,
            expires_at: session.expires_at,
        },
        user,
    ))
}

async fn renew_session(
    state: &AppState,
    session: &CurrentSession,
    session_key: &str,
    csrf_token: Option<&str>,
    response: &mut Response,
) {
    let now = OffsetDateTime::now_utc();
    let Some(csrf_token) = csrf_token else {
        return;
    };
    let Some(expires_at) = renewed_expiry(state, session, now) else {
        return;
    };
    let Ok(Some(_)) = state
        .database()
        .update::<AuthSession>(session.id)
        .set(AuthSession::EXPIRES_AT, expires_at)
        .execute()
        .await
    else {
        return;
    };

    let max_age = (expires_at - now).whole_seconds();
    append_cookie(
        response,
        cookie_header(
            state,
            &state.config.auth.session.cookie_name,
            session_key,
            max_age,
            true,
        ),
    );
    append_cookie(
        response,
        cookie_header(
            state,
            &state.config.auth.session.csrf_cookie_name,
            csrf_token,
            max_age,
            false,
        ),
    );
}

fn renewed_expiry(
    state: &AppState,
    session: &CurrentSession,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    let config = &state.config.auth.session;
    if config.ttl_seconds <= 0
        || config.renewal_window_seconds <= 0
        || config.absolute_ttl_seconds <= 0
        || session.expires_at > now + time::Duration::seconds(config.renewal_window_seconds)
    {
        return None;
    }

    let absolute_expiry = session.created_at + time::Duration::seconds(config.absolute_ttl_seconds);
    let expires_at = (now + time::Duration::seconds(config.ttl_seconds)).min(absolute_expiry);
    (expires_at > session.expires_at).then_some(expires_at)
}

fn append_cookie(response: &mut Response, value: String) {
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use axum::{
        Extension, Json, Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
        routing::get,
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::{AllowAny, AppModule, CurrentUserResolverFuture, ModuleContext, Server, ViewSet};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ApplicationUser {
        user_id: i64,
        organization_id: i64,
    }

    struct TestModule;

    #[derive(Clone, Copy, Default)]
    struct ApplicationUserPermission;

    impl Permission<User> for ApplicationUserPermission {
        fn check(
            &self,
            _state: &AppState,
            current: Option<&CurrentPrincipal>,
            _action: ViewAction,
        ) -> crate::AppResult<()> {
            current
                .and_then(|current| current.app::<ApplicationUser>())
                .ok_or_else(|| crate::AppError::Unauthorized("profile required".into()))?;
            Ok(())
        }
    }

    #[derive(Clone, Copy, Default)]
    struct ApplicationUserViewSet;

    impl ViewSet for ApplicationUserViewSet {
        type Model = User;
        type Serializer = AdminUserSerializer;
        type QuerySet = che_orm::DatabaseQuery<User>;
        type FilterSet = AdminUserFilterSet;
        type Permission = AllowAny;

        fn path(&self) -> &'static str {
            "/profiles"
        }

        fn get_queryset(&self) -> Self::QuerySet {
            che_orm::DatabaseQuery::new(User::query())
        }

        fn prepare_create(
            &self,
            _state: &AppState,
            current: Option<&CurrentPrincipal>,
            write: che_orm::ValidatedWrite<Self::Model>,
        ) -> crate::AppResult<che_orm::ValidatedWrite<Self::Model>> {
            current
                .and_then(|current| current.app::<ApplicationUser>())
                .ok_or_else(|| crate::AppError::Unauthorized("profile required".into()))?;
            Ok(write)
        }
    }

    impl AppModule for TestModule {
        fn name(&self) -> &'static str {
            "test"
        }

        fn schema(&self) -> che_orm::SchemaSet {
            che_orm::SchemaSet::new()
        }

        fn init(&self, context: &mut ModuleContext) {
            context.route_at_root(
                Router::new()
                    .route("/protected", get(protected))
                    .route("/public", get(|| async { StatusCode::OK })),
            );
        }
    }

    async fn protected(current: Option<Extension<CurrentPrincipal>>) -> Json<serde_json::Value> {
        let current = current.map(|current| current.0);
        Json(json!({
            "authenticated": current.is_some(),
            "id": current.as_ref().map(|current| current.auth_user().id),
            "has_profile": current
                .as_ref()
                .is_some_and(|current| current.app::<ApplicationUser>().is_some()),
        }))
    }

    fn profile_resolver(
        calls: Arc<AtomicUsize>,
    ) -> impl for<'a> Fn(&'a AppState, &'a User) -> CurrentUserResolverFuture<'a, ApplicationUser>
    + Send
    + Sync
    + 'static {
        move |_state, user| {
            let calls = calls.clone();
            let user_id = user.id;
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Some(ApplicationUser {
                    user_id,
                    organization_id: 7,
                }))
            })
        }
    }

    fn no_profile_resolver<'a>(
        _state: &'a AppState,
        _user: &'a User,
    ) -> CurrentUserResolverFuture<'a, ApplicationUser> {
        Box::pin(async { Ok(None) })
    }

    async fn auth_app(state: AppState) -> axum::Router {
        Server::new(state)
            .install(crate::InstalledApps::new().add(module()).add(TestModule))
            .build()
            .await
            .unwrap()
    }

    async fn setup_auth(state: &AppState) -> User {
        state.database().create_table::<User>().await.unwrap();
        state.database().create_table::<AuthToken>().await.unwrap();
        state
            .database()
            .create_table::<AuthSession>()
            .await
            .unwrap();
        state
            .database()
            .create::<User>()
            .set(User::USERNAME, "user")
            .set(User::PASSWORD_HASH, "unused")
            .set(User::IS_ACTIVE, true)
            .set(User::IS_ADMIN, true)
            .execute()
            .await
            .unwrap()
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

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
            created_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
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
    fn session_is_renewed_only_in_the_renewal_window() {
        let state = AppState::from_database(che_orm::Database::connect(":memory:").unwrap());
        let now = OffsetDateTime::now_utc();
        let session = CurrentSession {
            id: 1,
            user_id: 2,
            csrf_hash: String::new(),
            revision: 0,
            created_at: now - time::Duration::days(5),
            expires_at: now + time::Duration::hours(1),
        };
        let renewed = renewed_expiry(&state, &session, now).unwrap();
        assert!(renewed > now + time::Duration::days(6));

        let session = CurrentSession {
            expires_at: now + time::Duration::days(2),
            ..session
        };
        assert!(renewed_expiry(&state, &session, now).is_none());
    }

    #[test]
    fn session_renewal_does_not_exceed_absolute_ttl() {
        let state = AppState::from_database(che_orm::Database::connect(":memory:").unwrap());
        let now = OffsetDateTime::now_utc();
        let created_at = now - time::Duration::days(29);
        let session = CurrentSession {
            id: 1,
            user_id: 2,
            csrf_hash: String::new(),
            revision: 0,
            created_at,
            expires_at: now + time::Duration::hours(1),
        };
        assert_eq!(
            renewed_expiry(&state, &session, now),
            Some(created_at + time::Duration::days(30))
        );
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
        let state = AppState::from_database(che_orm::Database::connect(":memory:").unwrap());
        let regular = CurrentUser {
            id: 1,
            username: "regular".into(),
            is_staff: false,
            is_admin: false,
            is_superuser: false,
        };
        let regular = CurrentPrincipal {
            auth_user: regular,
            app_user: None,
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

        let admin = CurrentPrincipal {
            auth_user: CurrentUser {
                is_admin: true,
                ..regular.auth_user.clone()
            },
            app_user: None,
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

    #[tokio::test]
    async fn resolver_builds_principal_with_framework_and_application_users() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap())
            .with_current_user_resolver(profile_resolver(calls.clone()));
        let user = User {
            id: 42,
            username: "user".into(),
            password_hash: String::new(),
            is_active: true,
            is_staff: false,
            is_admin: true,
            is_superuser: false,
        };

        let principal = current_principal(&state, &user).await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(principal.auth_user().id, 42);
        assert!(principal.auth_user().is_admin);
        assert_eq!(
            principal.app::<ApplicationUser>().unwrap().organization_id,
            7
        );
        assert!(principal.app::<String>().is_none());
    }

    #[tokio::test]
    async fn resolver_returning_none_keeps_framework_authentication() {
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap())
            .with_current_user_resolver(no_profile_resolver);
        let user = User {
            id: 42,
            username: "user".into(),
            password_hash: String::new(),
            is_active: true,
            is_staff: false,
            is_admin: false,
            is_superuser: false,
        };

        let principal = current_principal(&state, &user).await.unwrap();

        assert_eq!(principal.auth_user().id, user.id);
        assert!(principal.app::<ApplicationUser>().is_none());
        assert!(
            <IsAuthenticated as Permission<User>>::check(
                &IsAuthenticated,
                &state,
                Some(&principal),
                ViewAction::List,
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn admin_user_creation_is_mounted_under_api_prefix() {
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap());
        setup_auth(&state).await;
        let app = auth_app(state).await;
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/auth/users/create/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"username":"new-user","password":"secret"}"#))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn custom_permissions_and_viewset_hooks_can_access_application_user() {
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap())
            .with_current_user_resolver(profile_resolver(Arc::new(AtomicUsize::new(0))));
        let principal = current_principal(
            &state,
            &User {
                id: 1,
                username: "user".into(),
                password_hash: String::new(),
                is_active: true,
                is_staff: false,
                is_admin: false,
                is_superuser: false,
            },
        )
        .await
        .unwrap();
        let write = <AdminUserSerializer as che_orm::ModelWriteSerializer>::is_valid(
            json!({}),
            che_orm::WriteMode::Create,
        )
        .unwrap();

        <ApplicationUserPermission as Permission<User>>::check(
            &ApplicationUserPermission,
            &state,
            Some(&principal),
            ViewAction::Create,
        )
        .unwrap();
        ApplicationUserViewSet
            .prepare_create(&state, Some(&principal), write)
            .unwrap();
    }

    #[tokio::test]
    async fn token_and_session_authentication_resolve_application_user() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap())
            .with_current_user_resolver(profile_resolver(calls.clone()));
        let user = setup_auth(&state).await;
        let token = "token";
        state
            .database()
            .create::<AuthToken>()
            .set(AuthToken::USER_ID, user.id)
            .set(AuthToken::KEY_HASH, token_hash(token))
            .execute()
            .await
            .unwrap();
        let session_key = "session";
        state
            .database()
            .create::<AuthSession>()
            .set(AuthSession::USER_ID, user.id)
            .set(AuthSession::KEY_HASH, token_hash(session_key))
            .set(AuthSession::CSRF_HASH, token_hash("csrf"))
            .set(AuthSession::DATA, "{}")
            .set(AuthSession::REVISION, 0_i64)
            .set(
                AuthSession::EXPIRES_AT,
                OffsetDateTime::now_utc() + time::Duration::hours(1),
            )
            .execute()
            .await
            .unwrap();
        let app = auth_app(state.clone()).await;

        let token_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(header::AUTHORIZATION, "Token token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(token_response.status(), StatusCode::OK);
        assert_eq!(response_json(token_response).await["has_profile"], true);

        let session_response = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(header::COOKIE, "che_rest_session=session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(session_response.status(), StatusCode::OK);
        assert_eq!(response_json(session_response).await["id"], user.id);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn authentication_succeeds_without_a_profile_or_resolver() {
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap());
        let user = setup_auth(&state).await;
        state
            .database()
            .create::<AuthToken>()
            .set(AuthToken::USER_ID, user.id)
            .set(AuthToken::KEY_HASH, token_hash("token"))
            .execute()
            .await
            .unwrap();
        let app = auth_app(state).await;

        let public = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/public")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::OK);
        let protected = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(header::AUTHORIZATION, "Token token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let payload = response_json(protected).await;
        assert_eq!(payload["authenticated"], true);
        assert_eq!(payload["has_profile"], false);
    }
}
