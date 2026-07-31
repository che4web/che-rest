pub mod models;
pub mod views;

use axum::{
    Extension,
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use che_orm::Model;
use rand::RngCore;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{AppModule, ModuleContext, state::AppState};

use self::models::{AuthToken, User};

#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub is_staff: bool,
    pub is_admin: bool,
    pub is_superuser: bool,
}

pub fn module() -> AuthModule {
    AuthModule
}

pub struct AuthModule;

impl AppModule for AuthModule {
    fn name(&self) -> &'static str {
        "auth"
    }

    fn init(&self, ctx: &mut ModuleContext) {
        ctx.model::<User>();
        ctx.model::<AuthToken>();
        ctx.enable_auth();
    }
}

pub async fn auth_middleware(
    Extension(state): Extension<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let Some(token) = request_token(&request) else {
        return unauthorized("authentication credentials were not provided");
    };

    let token_hash = token_hash(&token);
    let tokens = match AuthToken::objects(state.db())
        .query()
        .eq("key_hash", token_hash)
        .limit(1)
        .all()
        .await
    {
        Ok(tokens) => tokens,
        Err(_) => return unauthorized("invalid authentication token"),
    };
    let Some(auth_token) = tokens.into_iter().next() else {
        return unauthorized("invalid authentication token");
    };

    let user = match User::objects(state.db()).get(auth_token.user_id).await {
        Ok(user) if user.is_active => user,
        _ => return unauthorized("invalid authentication token"),
    };

    request.extensions_mut().insert(CurrentUser {
        id: user.id,
        username: user.username,
        is_staff: user.is_staff,
        is_admin: user.is_admin,
        is_superuser: user.is_superuser,
    });

    next.run(request).await
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
