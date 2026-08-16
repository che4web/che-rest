#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Orm(#[from] che_orm2::OrmError),
    #[error(transparent)]
    Rest(#[from] che_orm2_rest::RestError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error("bad request: {0}")]
    BadRequest(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Rest(error) => error.into_response(),
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
