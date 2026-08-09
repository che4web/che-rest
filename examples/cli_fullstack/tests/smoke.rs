use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use che_orm::Model;
use cli_fullstack::apps;
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{COOKIE, SET_COOKIE};
use serde_json::json;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

#[tokio::test]
async fn fullstack_session_rest_and_websocket_smoke() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let database_path = std::env::temp_dir().join(format!("cli_fullstack_smoke_{suffix}.sqlite"));
    let config_path = std::env::temp_dir().join(format!("cli_fullstack_smoke_{suffix}.toml"));
    fs::write(
        &config_path,
        format!(
            "[database]\nurl = \"sqlite://{}?mode=rwc\"\n\n[auth.session]\ncookie_name = \"cli_fullstack_session\"\ncsrf_cookie_name = \"csrf_token\"\n",
            database_path.display()
        ),
    )
    .unwrap();

    let state = che_rest::AppState::from_config_file(&config_path)
        .await
        .unwrap();
    for app in ["auth", "tasks", "notifications"] {
        state
            .db()
            .apply_migrations_dir_with_namespace(
                app,
                root.join("src/apps").join(app).join("migrations"),
            )
            .await
            .unwrap();
    }

    let mut events = state.app_channels().subscribe("tasks.created");
    let app = che_rest::Server::new(state)
        .install(apps::installed_apps())
        .build()
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base_url = format!("http://{address}");
    assert_eq!(events.recv().await.unwrap()["kind"], "startup");

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login = client
        .post(format!("{base_url}/api-session-auth/login/"))
        .json(&json!({"username": "admin", "password": "secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), reqwest::StatusCode::UNAUTHORIZED);

    let password_hash = che_rest::auth::models::hash_password("secret").unwrap();
    let user_state = state_for_user(&config_path).await;
    let user = che_rest::auth::models::User::objects(user_state.db())
        .create()
        .set("username", "admin")
        .set("password_hash", password_hash)
        .set("is_active", true)
        .set("is_staff", true)
        .set("is_admin", true)
        .set("is_superuser", true)
        .execute()
        .await
        .unwrap();

    let login = client
        .post(format!("{base_url}/api-session-auth/login/"))
        .json(&json!({"username": "admin", "password": "secret"}))
        .send()
        .await
        .unwrap();
    assert!(login.status().is_success());
    let cookies = login
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap().to_string())
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("csrf_token="))
        .unwrap();
    let cookie_header = cookies.join("; ");

    let task = client
        .post(format!("{base_url}/api/tasks/"))
        .header("X-CSRF-Token", csrf)
        .json(&json!({"name": "REST task"}))
        .send()
        .await
        .unwrap();
    assert_eq!(task.status(), reqwest::StatusCode::CREATED);
    assert_eq!(
        task.json::<serde_json::Value>().await.unwrap()["author"]["username"],
        "admin"
    );
    let rest_event = events.recv().await.unwrap();
    assert_eq!(rest_event["name"], "REST task");
    assert_eq!(rest_event["author_id"], user.id);

    let mut request = format!("ws://{address}/api/ws/")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(COOKIE, cookie_header.parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(Message::Text(
            json!({
                "action": "publish",
                "event": "tasks.create",
                "payload": {"name": "WebSocket task"}
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let published = socket.next().await.unwrap().unwrap();
    let published: serde_json::Value = serde_json::from_str(published.to_text().unwrap()).unwrap();
    assert_eq!(published["type"], "published");
    assert_eq!(published["event"], "tasks.create");
    let websocket_event = events.recv().await.unwrap();
    assert_eq!(websocket_event["name"], "WebSocket task");
    assert_eq!(websocket_event["author_id"], user.id);

    server.abort();
    let _ = fs::remove_file(database_path);
    let _ = fs::remove_file(config_path);
}

async fn state_for_user(config_path: &PathBuf) -> che_rest::AppState {
    che_rest::AppState::from_config_file(config_path)
        .await
        .unwrap()
}
