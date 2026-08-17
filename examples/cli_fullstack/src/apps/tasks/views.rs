use che_rest::{AllowAny, CurrentUser, Model, ViewSet};

use super::{filters::TaskFilterSet, models::Task, serializers::TaskSerializer};

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type QuerySet = che_orm2::SelectRelatedQuery<
        Task,
        che_rest::auth::models::User,
        super::models::TaskAuthorRelation,
    >;
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;

    fn get_queryset(&self) -> Self::QuerySet {
        che_orm2::DatabaseQuery::new(Task::query()).select_related(Task::AUTHOR)
    }

    fn prepare_create(
        &self,
        _state: &che_rest::AppState,
        user: Option<&CurrentUser>,
        write: che_rest::ValidatedWrite<Self::Model>,
    ) -> che_rest::AppResult<che_rest::ValidatedWrite<Self::Model>> {
        let user =
            user.ok_or_else(|| che_rest::AppError::Unauthorized("authentication required".into()))?;
        Ok(write.set(Task::AUTHOR_ID, user.id))
    }

    fn path(&self) -> &'static str {
        "/tasks"
    }
}
