use che_rest::Management;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Management::new({{crate_name}}::apps::installed_apps())
        .migrations({{crate_name}}::migrations::all())
        .project_root(env!("CARGO_MANIFEST_DIR"))
        .run()
        .await
}
