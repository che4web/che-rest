use che_rest::{AllowAny, Model, ViewSet};

use super::{filters::TaskFilterSet, models::Task, serializers::TaskSerializer};

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type QuerySet = che_orm2::DatabaseQuery<Task>;
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;

    fn get_queryset(&self) -> Self::QuerySet {
        che_orm2::DatabaseQuery::new(Task::query())
    }

    fn path(&self) -> &'static str {
        "/tasks"
    }
}
