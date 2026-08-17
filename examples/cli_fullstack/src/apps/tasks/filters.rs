use che_rest::{Filter, FilterSetSpec};

use super::models::Task;

#[derive(Clone, Copy, Default)]
pub struct TaskFilterSet;

static TASK_FILTERS: &[Filter<Task>] = &[
    Filter::exact(Task::ID),
    Filter::contains(Task::NAME),
    Filter::exact_enum(Task::STATUS),
    Filter::exact(Task::CREATED_AT),
    Filter::exact(Task::UPDATED_AT),
];

impl FilterSetSpec for TaskFilterSet {
    type Model = Task;

    fn filters(&self) -> &'static [Filter<Task>] {
        TASK_FILTERS
    }
}
