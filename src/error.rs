#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Orm(#[from] che_orm2::OrmError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found")]
    NotFound,
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::NotFound => (
                axum::http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({ "detail": "not found" })),
            )
                .into_response(),
            Self::Unauthorized(detail) => (
                axum::http::StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "detail": detail })),
            )
                .into_response(),
            Self::Forbidden(detail) => (
                axum::http::StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "detail": detail })),
            )
                .into_response(),
            Self::BadRequest(detail) => (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "detail": detail })),
            )
                .into_response(),
            error => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "detail": error.to_string() })),
            )
                .into_response(),
        }
    }
}
