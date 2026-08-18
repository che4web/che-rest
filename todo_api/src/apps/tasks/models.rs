#[derive(Debug, che_orm::Model)]
#[orm(table = "tasks")]
pub struct Task {
    #[orm(primary_key)]
    pub id: i64,
    pub title: String,
    pub completed: bool,
}
