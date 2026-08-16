use che_rest::{AppState, Server};
use cli_fullstack::apps;
use cli_fullstack::apps::{notifications::models::Notification, tasks::models::Task};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState::from_config_file("app.toml").await?;
    state.database().create_table::<che_rest::auth::User>().await?;
    state.database().create_table::<che_rest::auth::AuthToken>().await?;
    state.database().create_table::<che_rest::auth::AuthSession>().await?;
    state.database().create_table::<Task>().await?;
    state.database().create_table::<Notification>().await?;
    let server_config = state.config.server.clone();
    let app = Server::new(state)
        .install(apps::installed_apps())
        .api_prefix(&server_config.api_prefix)
        .build()
        .await?;

    let address = format!("{}:{}", server_config.host, server_config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}
