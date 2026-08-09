use axum::http::Extensions;
use che_rest::auth::CurrentUser;
use che_rest::{AppError, AppResult, AppState, IsAuthenticated, ViewSet};
use serde_json::{Map, Value, json};

use super::{filters::TaskFilterSet, models::Task, serializers::TaskSerializer};

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

#[che_rest::async_trait]
impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type FilterSet = TaskFilterSet;
    type Permission = IsAuthenticated;

    async fn system_create_values(
        &self,
        _state: &AppState,
        extensions: &Extensions,
    ) -> AppResult<Map<String, Value>> {
        let user = extensions.get::<CurrentUser>().ok_or_else(|| {
            AppError::Unauthorized("an authenticated user is required".to_string())
        })?;
        Ok(Map::from_iter([("author_id".to_string(), json!(user.id))]))
    }
}
