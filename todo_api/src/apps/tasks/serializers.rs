use che_rest::{Field, Serializer};

use super::models::Task;

static TASK_FIELDS: &[Field] = &[
    Field::new("id").read_only(),
    Field::new("name"),
    Field::new("created_at").read_only(),
    Field::new("updated_at").read_only(),
];

#[derive(Clone, Copy, Default)]
pub struct TaskSerializer;

#[che_rest::async_trait]
impl Serializer for TaskSerializer {
    type Model = Task;

    fn fields(&self) -> &'static [Field] {
        TASK_FIELDS
    }
}
