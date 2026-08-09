use che_rest::{AppState, Server};
use cli_fullstack::apps;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState::from_config_file("app.toml").await?;
    let app = Server::new(state)
        .install(apps::installed_apps())
        .openapi_title("CLI Fullstack Example")
        .build()
        .await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3001").await?;
    println!("listening on http://127.0.0.1:3001");
    axum::serve(listener, app).await?;
    Ok(())
}
