use che_rest::{Filter, FilterSetSpec};

use super::models::{Task, TaskFields};

static TASK_FILTERS: &[Filter<Task>] = &[
    Filter::exact(TaskFields::NAME),
    Filter::contains(TaskFields::NAME),
    Filter::gte(TaskFields::CREATED_AT),
    Filter::lte(TaskFields::CREATED_AT),
    Filter::gte(TaskFields::UPDATED_AT),
    Filter::lte(TaskFields::UPDATED_AT),
];

#[derive(Clone, Copy, Default)]
pub struct TaskFilterSet;

impl FilterSetSpec for TaskFilterSet {
    type Model = Task;

    fn filters(&self) -> &'static [Filter<Task>] {
        TASK_FILTERS
    }
}
