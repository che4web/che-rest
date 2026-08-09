use che_rest::{Filter, FilterSetSpec};

use super::models::{Task, TaskFields};

static TASK_FILTERS: &[Filter<Task>] = &[
    Filter::exact(TaskFields::NAME),
    Filter::contains(TaskFields::NAME),
];

#[derive(Clone, Copy, Default)]
pub struct TaskFilterSet;

impl FilterSetSpec for TaskFilterSet {
    type Model = Task;

    fn filters(&self) -> &'static [Filter<Task>] {
        TASK_FILTERS
    }
}
