use che_rest::{AllowAny, CurrentPrincipal, Model, SignalAccess, ViewAction, ViewSet};

use super::{filters::TaskFilterSet, models::Task, serializers::TaskSerializer};

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type QuerySet = che_orm::SelectRelatedQuery<
        Task,
        che_rest::auth::models::User,
        super::models::TaskAuthorRelation,
    >;
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;

    fn get_queryset(&self) -> Self::QuerySet {
        che_orm::DatabaseQuery::new(Task::query()).select_related(Task::AUTHOR)
    }

    fn prepare_create(
        &self,
        _state: &che_rest::AppState,
        current: Option<&CurrentPrincipal>,
        write: che_rest::ValidatedWrite<Self::Model>,
    ) -> che_rest::AppResult<che_rest::ValidatedWrite<Self::Model>> {
        let user = current
            .map(CurrentPrincipal::auth_user)
            .ok_or_else(|| che_rest::AppError::Unauthorized("authentication required".into()))?;
        Ok(write.set(Task::AUTHOR_ID, user.id))
    }

    fn path(&self) -> &'static str {
        "/tasks"
    }

    fn signal_access(&self, action: ViewAction) -> Option<SignalAccess> {
        match action {
            ViewAction::Create | ViewAction::Update | ViewAction::Patch | ViewAction::Delete => {
                Some(SignalAccess::Authenticated)
            }
            ViewAction::List | ViewAction::Retrieve => None,
        }
    }
}
