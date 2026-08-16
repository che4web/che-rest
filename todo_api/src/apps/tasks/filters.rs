use che_rest::{Filter, FilterSetSpec};

use super::models::Task;

static TASK_FILTERS: &[Filter<Task>] = &[
    Filter::contains(Task::TITLE),
    Filter::exact(Task::COMPLETED),
];

#[derive(Clone, Copy, Default)]
pub struct TaskFilterSet;

impl FilterSetSpec for TaskFilterSet {
    type Model = Task;

    fn filters(&self) -> &'static [Filter<Task>] {
        TASK_FILTERS
    }
}
