use che_rest::{Filter, FilterSetSpec};

use super::models::Task;

#[derive(Clone, Copy, Default)]
pub struct TaskFilterSet;

impl FilterSetSpec for TaskFilterSet {
    type Model = Task;

    fn filters(&self) -> &'static [Filter<Task>] {
        &[]
    }
}
