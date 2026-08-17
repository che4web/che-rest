use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use cli_fullstack::apps::{self, notifications::models::Notification, tasks::models::Task};
use futures_util::{SinkExt, StreamExt};
use reqwest::header::SET_COOKIE;
use serde_json::json;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

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

    let state = che_rest::AppState::from_config_file(&config_path)
        .await
        .unwrap();
    state
        .database()
        .create_table::<che_rest::auth::User>()
        .await
        .unwrap();
    state
        .database()
        .create_table::<che_rest::auth::AuthToken>()
        .await
        .unwrap();
    state
        .database()
        .create_table::<che_rest::auth::AuthSession>()
        .await
        .unwrap();
    state.database().create_table::<Task>().await.unwrap();
    state
        .database()
        .create_table::<Notification>()
        .await
        .unwrap();

    let password_hash = che_rest::auth::hash_password("secret").unwrap();
    state
        .database()
        .create::<che_rest::auth::User>()
        .set(che_rest::auth::User::USERNAME, "admin")
        .set(che_rest::auth::User::PASSWORD_HASH, password_hash)
        .set(che_rest::auth::User::IS_ACTIVE, true)
        .set(che_rest::auth::User::IS_ADMIN, true)
        .execute()
        .await
        .unwrap();
    state
        .database()
        .create::<che_rest::auth::User>()
        .set(che_rest::auth::User::USERNAME, "assignee")
        .set(
            che_rest::auth::User::PASSWORD_HASH,
            che_rest::auth::hash_password("secret").unwrap(),
        )
        .set(che_rest::auth::User::IS_ACTIVE, true)
        .execute()
        .await
        .unwrap();

    let server_state = state.clone();
    let app = che_rest::Server::new(server_state)
        .install(apps::installed_apps())
        .api_prefix("/v1")
        .build()
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let base_url = format!("http://{address}");

    let swagger = client.get(format!("{base_url}/v1/")).send().await.unwrap();
    assert_eq!(swagger.status(), reqwest::StatusCode::OK);
    let swagger_html = swagger.text().await.unwrap();
    assert!(swagger_html.contains("SwaggerUIBundle"));
    assert!(swagger_html.contains("withCredentials: true"));
    assert!(swagger_html.contains("X-CSRF-Token"));

    let openapi = client
        .get(format!("{base_url}/v1/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(openapi.status(), reqwest::StatusCode::OK);
    let openapi_payload = openapi.json::<serde_json::Value>().await.unwrap();
    assert_eq!(openapi_payload["servers"][0]["url"], "/v1");
    assert_eq!(
        openapi_payload["paths"]["/api-session-auth/login/"]["post"]["servers"][0]["url"],
        "/"
    );
    assert!(openapi_payload["components"]["securitySchemes"]["TokenAuth"].is_object());
    assert!(openapi_payload["components"]["securitySchemes"]["SessionCookie"].is_object());
    assert!(openapi_payload["paths"]["/api-session-auth/login/"].is_object());
    assert!(openapi_payload["paths"]["/tasks/"]["post"]["requestBody"].is_object());
    assert!(
        openapi_payload["components"]["schemas"]["TaskSerializerCreate"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "status")
    );
    assert_eq!(
        openapi_payload["components"]["schemas"]["TaskSerializerResponse"]["properties"]["status"]
            ["enum"],
        serde_json::json!(["draft", "in_progress", "done"])
    );
    assert_eq!(
        openapi_payload["components"]["schemas"]["TaskSerializerResponse"]["properties"]["author"]
            ["type"],
        "object"
    );
    assert_eq!(
        openapi_payload["paths"]["/tasks/"]["get"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|parameter| parameter["name"] == "status")
            .unwrap()["schema"]["enum"],
        serde_json::json!(["draft", "in_progress", "done"])
    );

    assert_eq!(
        client
            .get(format!("{base_url}/v1/tasks/"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let login = client
        .post(format!("{base_url}/api-session-auth/login/"))
        .json(&json!({"username": "admin", "password": "secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), reqwest::StatusCode::OK);
    let csrf = login
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix("csrf_token=")
                .and_then(|value| value.split(';').next())
        })
        .unwrap()
        .to_owned();
    let session = login
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix("che_rest_session=")
                .and_then(|value| value.split(';').next())
        })
        .unwrap()
        .to_owned();

    let ws_url = format!("ws://{address}/v1/ws/");
    let mut request = ws_url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Cookie",
        format!("che_rest_session={session}; csrf_token={csrf}")
            .parse()
            .unwrap(),
    );
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(Message::Text(
            json!({"action": "subscribe", "signal": "tasks.created"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let subscribed = socket.next().await.unwrap().unwrap().into_text().unwrap();
    let subscribed: serde_json::Value = serde_json::from_str(&subscribed).unwrap();
    assert_eq!(subscribed["type"], "subscribed");
    assert_eq!(subscribed["signal"], "tasks.created");

    let relation_users = client
        .get(format!("{base_url}/v1/auth/users/"))
        .send()
        .await
        .unwrap();
    assert_eq!(relation_users.status(), reqwest::StatusCode::OK);
    assert_eq!(
        relation_users.json::<serde_json::Value>().await.unwrap()["count"],
        2
    );

    let task = client
        .post(format!("{base_url}/v1/tasks/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"name": "REST task", "status": "draft", "assignee_id": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(task.status(), reqwest::StatusCode::CREATED);
    let task_payload = task.json::<serde_json::Value>().await.unwrap();
    assert_eq!(task_payload["author"]["username"], "admin");
    assert_eq!(task_payload["status"], "draft");
    assert_eq!(task_payload["assignee_id"], 2);
    let task_id = task_payload["id"].as_i64().unwrap();
    let signal = socket.next().await.unwrap().unwrap().into_text().unwrap();
    let signal: serde_json::Value = serde_json::from_str(&signal).unwrap();
    assert_eq!(signal["type"], "signal");
    assert_eq!(signal["signal"], "tasks.created");
    assert_eq!(signal["payload"]["id"], task_id);

    let second_task = client
        .post(format!("{base_url}/v1/tasks/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"name": "A first task", "status": "in_progress"}))
        .send()
        .await
        .unwrap();
    assert_eq!(second_task.status(), reqwest::StatusCode::CREATED);
    let filtered = client
        .get(format!("{base_url}/v1/tasks/?status=in_progress"))
        .send()
        .await
        .unwrap();
    assert_eq!(filtered.status(), reqwest::StatusCode::OK);
    let filtered_payload = filtered.json::<serde_json::Value>().await.unwrap();
    assert_eq!(filtered_payload["count"], 1);
    assert_eq!(filtered_payload["results"][0]["name"], "A first task");

    let invalid_filter = client
        .get(format!("{base_url}/v1/tasks/?status=unknown"))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_filter.status(), reqwest::StatusCode::BAD_REQUEST);
    let ordered = client
        .get(format!("{base_url}/v1/tasks/?ordering=name"))
        .send()
        .await
        .unwrap();
    assert_eq!(ordered.status(), reqwest::StatusCode::OK);
    let ordered_payload = ordered.json::<serde_json::Value>().await.unwrap();
    assert_eq!(ordered_payload["results"][0]["name"], "A first task");

    let updated = client
        .put(format!("{base_url}/v1/tasks/{task_id}/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"name": "Updated task", "status": "done", "assignee_id": null}))
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), reqwest::StatusCode::OK);
    assert_eq!(
        updated.json::<serde_json::Value>().await.unwrap()["name"],
        "Updated task"
    );

    let patched = client
        .patch(format!("{base_url}/v1/tasks/{task_id}/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({"name": "Patched task", "status": "done"}))
        .send()
        .await
        .unwrap();
    assert_eq!(patched.status(), reqwest::StatusCode::OK);
    assert_eq!(
        patched.json::<serde_json::Value>().await.unwrap()["name"],
        "Patched task"
    );

    let empty_patch = client
        .patch(format!("{base_url}/v1/tasks/{task_id}/"))
        .header("X-CSRF-Token", &csrf)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(empty_patch.status(), reqwest::StatusCode::BAD_REQUEST);

    let me = client
        .get(format!("{base_url}/api-session-auth/me/"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), reqwest::StatusCode::OK);
    assert_eq!(
        me.json::<serde_json::Value>().await.unwrap()["user"]["username"],
        "admin"
    );

    server.abort();
    let _ = fs::remove_file(database_path);
    let _ = fs::remove_file(config_path);
}
