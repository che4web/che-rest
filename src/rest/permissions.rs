//! Asynchronous authorization shared by REST actions and persistence.
use std::future::Future;

pub use super::router::ViewAction;
use crate::{AppResult, AppState, auth::CurrentPrincipal};
use che_orm::{Expr, Model, SelectQuery, Transaction, ValidatedWrite};

/// Query context. Write authorization receives the transaction that commits the write.
#[derive(Clone, Copy)]
pub struct PermissionContext<'a> {
    pub state: &'a AppState,
    pub transaction: Option<&'a Transaction>,
}
impl<'a> From<&'a AppState> for PermissionContext<'a> {
    fn from(state: &'a AppState) -> Self {
        Self {
            state,
            transaction: None,
        }
    }
}
impl<'a> PermissionContext<'a> {
    pub fn in_transaction(state: &'a AppState, transaction: &'a Transaction) -> Self {
        Self {
            state,
            transaction: Some(transaction),
        }
    }
}

pub trait Permission<M: Model>: Clone + Send + Sync + Default + 'static {
    fn check_request(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
    ) -> impl Future<Output = AppResult<()>> + Send;

    /// SQL restriction applied before counting, paging, and loading an object.
    /// Object-scoped policies must override this method; the default fails closed.
    fn scope(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
    ) -> impl Future<Output = AppResult<Option<Expr>>> + Send {
        async move {
            self.check_request(context, current, action).await?;
            if self.object_scoped() {
                return Err(crate::AppError::Forbidden(
                    "object-scoped permissions must provide an explicit SQL scope".into(),
                ));
            }
            Ok(None)
        }
    }
    fn check_object(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
        model: &M,
    ) -> impl Future<Output = AppResult<()>> + Send
    where
        M: Send + Sync + 'static,
    {
        async move {
            if let Some(scope) = self.scope(context, current, action).await? {
                let rows = context
                    .fetch_all(
                        M::query()
                            .filter(M::primary_key().eq(model.primary_key_value()))
                            .filter(scope)
                            .limit(1),
                    )
                    .await?;
                if rows.is_empty() {
                    return Err(crate::AppError::Forbidden(
                        "object permission required".into(),
                    ));
                }
            }
            Ok(())
        }
    }
    /// Runs after server-owned fields have been prepared and inside the write transaction.
    fn check_write(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
        model: Option<&M>,
        _write: Option<&ValidatedWrite<M>>,
    ) -> impl Future<Output = AppResult<()>> + Send
    where
        M: Send + Sync + 'static,
    {
        async move {
            self.check_request(context, current, action).await?;
            if let Some(model) = model {
                self.check_object(context, current, action, model).await?;
            }
            Ok(())
        }
    }
    /// Allows application-owned initialization such as creator grants in the same commit.
    fn after_write(
        &self,
        _context: PermissionContext<'_>,
        _current: Option<&CurrentPrincipal>,
        _action: ViewAction,
        _model: &M,
    ) -> impl Future<Output = AppResult<()>> + Send {
        async { Ok(()) }
    }
    fn validate(&self, _fields: &[che_orm::SerializerField]) -> AppResult<()> {
        Ok(())
    }
    /// Stable model identity used to reject accidental ACL namespace collisions.
    fn model_access_key(&self) -> Option<&'static str> {
        None
    }
    /// Custom policies are scoped by default; global policies explicitly opt out.
    /// Object-scoped policies cannot expose the whole-model protocol-2 journal.
    fn object_scoped(&self) -> bool {
        true
    }
}

#[derive(Clone, Copy, Default)]
pub struct AllowAny;
impl<M: Model> Permission<M> for AllowAny {
    fn object_scoped(&self) -> bool {
        false
    }
    async fn check_request(
        &self,
        _: PermissionContext<'_>,
        _: Option<&CurrentPrincipal>,
        _: ViewAction,
    ) -> AppResult<()> {
        Ok(())
    }
}

impl PermissionContext<'_> {
    pub async fn fetch_all<M: Model + Send + 'static>(
        &self,
        query: SelectQuery<M>,
    ) -> AppResult<Vec<M>> {
        Ok(match self.transaction {
            Some(tx) => tx.fetch_all(query).await?,
            None => self.state.database().fetch_all(query).await?,
        })
    }
}
