use che_rest::Management;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Management::new(cli_fullstack::apps::installed_apps()).run().await
}
