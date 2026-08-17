use che_rest::{InstalledApps, Management};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Management::new(InstalledApps::new()).run().await
}
