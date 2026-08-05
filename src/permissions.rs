use async_trait::async_trait;
use axum::http::Extensions;
use che_orm::SqliteModel;

use crate::{auth::CurrentUser, error::AppResult, state::AppState};

#[derive(Debug, Clone, Copy)]
pub enum ViewAction {
    List,
    Create,
    Retrieve,
    Update,
    Destroy,
}

#[async_trait]
pub trait Permission<M>: Clone + Send + Sync + Default + 'static
where
    M: SqliteModel<Id = i64>,
{
    async fn has_permission(
        &self,
        _state: &AppState,
        _extensions: &Extensions,
        _user: Option<&CurrentUser>,
        _action: ViewAction,
    ) -> AppResult<()> {
        Ok(())
    }

    async fn has_object_permission(
        &self,
        _state: &AppState,
        _extensions: &Extensions,
        _user: Option<&CurrentUser>,
        _action: ViewAction,
        _object: &M,
    ) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
pub struct AllowAny;

impl<M> Permission<M> for AllowAny where M: SqliteModel<Id = i64> {}

#[derive(Clone, Copy, Default)]
pub struct IsAuthenticated;

#[async_trait]
impl<M> Permission<M> for IsAuthenticated
where
    M: SqliteModel<Id = i64>,
{
    async fn has_permission(
        &self,
        _state: &AppState,
        _extensions: &Extensions,
        user: Option<&CurrentUser>,
        _action: ViewAction,
    ) -> AppResult<()> {
        if user.is_some() {
            Ok(())
        } else {
            Err(crate::AppError::Unauthorized(
                "authentication credentials were not provided".to_string(),
            ))
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct IsAdminUser;

#[async_trait]
impl<M> Permission<M> for IsAdminUser
where
    M: SqliteModel<Id = i64>,
{
    async fn has_permission(
        &self,
        _state: &AppState,
        _extensions: &Extensions,
        user: Option<&CurrentUser>,
        _action: ViewAction,
    ) -> AppResult<()> {
        if user.is_some_and(|user| user.is_admin || user.is_superuser) {
            Ok(())
        } else {
            Err(crate::AppError::Forbidden(
                "admin permissions are required".to_string(),
            ))
        }
    }
}
