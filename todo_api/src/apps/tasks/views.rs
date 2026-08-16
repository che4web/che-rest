use che_rest::{AllowAny, ViewSet};

use super::{filters::TaskFilterSet, models::Task, serializers::TaskSerializer};

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;

    fn path(&self) -> &'static str {
        "/tasks"
    }
}
