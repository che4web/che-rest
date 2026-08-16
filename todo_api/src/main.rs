use che_rest::{AppState, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState::from_config_file("app.toml").await?;
    let server_config = state.config.server.clone();
    let app = Server::new(state)
        .install(todo_api::apps::installed_apps())
        .api_prefix(&server_config.api_prefix)
        .build()
        .await?;

    let address = format!("{}:{}", server_config.host, server_config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
