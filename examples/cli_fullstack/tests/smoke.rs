use std::{fs, time::{SystemTime, UNIX_EPOCH}};

use cli_fullstack::apps::{self, notifications::models::Notification, tasks::models::Task};
use reqwest::header::SET_COOKIE;
use serde_json::json;

#[tokio::test]
async fn fullstack_session_rest_smoke() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let database_path = std::env::temp_dir().join(format!("cli_fullstack_smoke_{suffix}.sqlite"));
    let config_path = std::env::temp_dir().join(format!("cli_fullstack_smoke_{suffix}.toml"));
    fs::write(
        &config_path,
        format!(
            "[database]\nurl = \"sqlite://{}?mode=rwc\"\n",
            database_path.display()
        ),
    )
    .unwrap();

    let state = che_rest::AppState::from_config_file(&config_path).await.unwrap();
    state.database().create_table::<che_rest::auth::User>().await.unwrap();
    state.database().create_table::<che_rest::auth::AuthToken>().await.unwrap();
    state.database().create_table::<che_rest::auth::AuthSession>().await.unwrap();
    state.database().create_table::<Task>().await.unwrap();
    state.database().create_table::<Notification>().await.unwrap();

    let password_hash = che_rest::auth::hash_password("secret").unwrap();
    state.database().create::<che_rest::auth::User>()
        .set(che_rest::auth::User::USERNAME, "admin")
        .set(che_rest::auth::User::PASSWORD_HASH, password_hash)
        .set(che_rest::auth::User::IS_ACTIVE, true)
        .execute().await.unwrap();

    let server_state = state.clone();
    let app = che_rest::Server::new(server_state).install(apps::installed_apps()).build().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let base_url = format!("http://{address}");

    assert_eq!(client.get(format!("{base_url}/api/tasks/")).send().await.unwrap().status(), reqwest::StatusCode::OK);
    let login = client.post(format!("{base_url}/api-session-auth/login/")
        ).json(&json!({"username": "admin", "password": "secret"}))
        .send().await.unwrap();
    assert_eq!(login.status(), reqwest::StatusCode::OK);
    let csrf = login.headers().get_all(SET_COOKIE).iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.strip_prefix("csrf_token=").and_then(|value| value.split(';').next()))
        .unwrap().to_owned();

    let task = client.post(format!("{base_url}/api/tasks/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"author_id": 1, "name": "REST task"}))
        .send().await.unwrap();
    assert_eq!(task.status(), reqwest::StatusCode::CREATED);
    let task_id = task.json::<serde_json::Value>().await.unwrap()["id"]
        .as_i64().unwrap();

    let updated = client.put(format!("{base_url}/api/tasks/{task_id}/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"author_id": 1, "name": "Updated task"}))
        .send().await.unwrap();
    assert_eq!(updated.status(), reqwest::StatusCode::OK);
    assert_eq!(updated.json::<serde_json::Value>().await.unwrap()["name"], "Updated task");

    let patched = client.patch(format!("{base_url}/api/tasks/{task_id}/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"name": "Patched task"}))
        .send().await.unwrap();
    assert_eq!(patched.status(), reqwest::StatusCode::OK);
    assert_eq!(patched.json::<serde_json::Value>().await.unwrap()["name"], "Patched task");

    let empty_patch = client.patch(format!("{base_url}/api/tasks/{task_id}/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({}))
        .send().await.unwrap();
    assert_eq!(empty_patch.status(), reqwest::StatusCode::BAD_REQUEST);

    let me = client.get(format!("{base_url}/api-session-auth/me/")).send().await.unwrap();
    assert_eq!(me.status(), reqwest::StatusCode::OK);
    assert_eq!(me.json::<serde_json::Value>().await.unwrap()["user"]["username"], "admin");

    server.abort();
    let _ = fs::remove_file(database_path);
    let _ = fs::remove_file(config_path);
}
