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
