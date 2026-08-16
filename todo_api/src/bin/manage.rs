use che_rest::Management;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Management::new(todo_api::apps::installed_apps())
        .run()
        .await
}
