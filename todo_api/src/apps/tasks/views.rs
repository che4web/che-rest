use che_rest::{AllowAny, ViewSet};

use super::{filters::TaskFilterSet, models::Task, serializers::TaskSerializer};

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

#[che_rest::async_trait]
impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;
}
