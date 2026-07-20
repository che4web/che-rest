use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::filters::FilterError;
use crate::serializer::SerializerError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Orm(#[from] che_orm::Error),

    #[error(transparent)]
    Serializer(#[from] SerializerError),

    #[error(transparent)]
    Filter(#[from] FilterError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

pub type AppResult<T> = std::result::Result<T, AppError>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Serializer(_) | Self::Filter(_) => StatusCode::BAD_REQUEST,
            Self::Orm(che_orm::Error::UnknownField(_)) => StatusCode::BAD_REQUEST,
            Self::Orm(che_orm::Error::ReadonlyField(_)) => StatusCode::BAD_REQUEST,
            Self::Orm(che_orm::Error::EmptyUpdate) => StatusCode::BAD_REQUEST,
            Self::Orm(che_orm::Error::Database(sqlx::Error::RowNotFound)) => StatusCode::NOT_FOUND,
            Self::Orm(_) | Self::Io(_) | Self::Toml(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, Json(json!({ "detail": self.to_string() }))).into_response()
    }
}
