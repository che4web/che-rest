#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Orm(#[from] che_orm::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

pub type AppResult<T> = std::result::Result<T, AppError>;
