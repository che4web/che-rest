use che_rest::{Field, Serializer};

use super::models::Task;

#[derive(Clone, Copy, Default)]
pub struct AuthorSerializer;

#[che_rest::async_trait]
impl Serializer for AuthorSerializer {
    type Model = che_rest::auth::models::User;

    fn fields(&self) -> &'static [Field] {
        static FIELDS: &[Field] = &[
            Field::new("id").read_only(),
            Field::new("username").read_only(),
        ];
        FIELDS
    }
}

static TASK_FIELDS: &[Field] = &[
    Field::new("id").read_only(),
    Field::related::<AuthorSerializer>("author", "author_id"),
    Field::new("author_id").system(),
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
