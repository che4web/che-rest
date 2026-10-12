#[path = "../examples/todo_sync/support.rs"]
mod todo;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::Uuid;
use che_rest::{ApiSchema, Server};
use che_sync::protocol::*;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn app() -> (Router, che_rest::AppState, String) {
    let (state, token) = todo::demo_state().await.unwrap();
    let router = Server::new(state.clone())
        .install(todo::apps())
        .build()
        .await
        .unwrap();
    (router, state, token)
}

async fn request(
    app: &Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    data: Value,
) -> (StatusCode, Value) {
    let mut method = method.to_owned();
    let mut path = path.to_owned();
    let mut data = data;
    let mut sync_headers = axum::http::HeaderMap::new();
    if data.get("operation").is_some() {
        let operation: PushRequest = serde_json::from_value(data).unwrap();
        assert_eq!(operation.operation.mutations.len(), 1);
        let mutation = &operation.operation.mutations[0];
        for (name, value) in [
            ("x-sync-protocol", operation.protocol_version.to_string()),
            ("x-sync-replica", operation.replica_id.to_string()),
            ("idempotency-key", operation.operation.id.to_string()),
            ("x-sync-scope", operation.scope),
            ("x-sync-version", mutation.schema_version.to_string()),
            ("x-sync-entity", mutation.id.to_string()),
            ("if-match", format!("\"{}\"", mutation.base_revision)),
        ] {
            sync_headers.insert(
                axum::http::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
        }
        method = if mutation.changes.is_none() {
            "DELETE"
        } else if mutation.base_revision == 0 {
            "POST"
        } else {
            "PATCH"
        }
        .into();
        if method != "POST" {
            path = format!("{}{}/", path, mutation.id);
        }
        data = mutation
            .changes
            .as_ref()
            .map(|changes| {
                Value::Object(
                    changes
                        .iter()
                        .map(|(name, value)| {
                            let value = serde_json::to_value(value).unwrap();
                            (
                                if name == "project" {
                                    "project_id"
                                } else {
                                    name
                                }
                                .to_owned(),
                                value.get("value").cloned().unwrap_or(Value::Null),
                            )
                        })
                        .collect(),
                )
            })
            .unwrap_or(Value::Null);
    }
    let mut request = Request::builder()
        .method(method.as_str())
        .uri(path)
        .header("Content-Type", "application/json");
    request.headers_mut().unwrap().extend(sync_headers);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Token {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(data.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn push(id: Uuid, title: &str) -> PushRequest {
    PushRequest {
        protocol_version: PROTOCOL_VERSION,
        scope: "todo-board".into(),
        replica_id: Uuid::now_v7(),
        operation: Operation {
            id: Uuid::now_v7(),
            mutations: vec![Mutation {
                model: "todo.todo".into(),
                id,
                schema_version: 1,
                base_revision: 0,
                changes: Some(Record::from([
                    (
                        "title".into(),
                        che_sync::protocol::Value::Text(title.into()),
                    ),
                    ("done".into(), che_sync::protocol::Value::Boolean(false)),
                ])),
            }],
        },
    }
}

#[tokio::test]
async fn authenticated_push_replay_pull_and_scope_validation() {
    let (app, state, token) = app().await;
    for (method, path, payload) in [
        ("GET", "/api/todos/sync/snapshot/", Value::Null),
        (
            "POST",
            "/api/todos/",
            serde_json::to_value(push(Uuid::now_v7(), "offline")).unwrap(),
        ),
        (
            "POST",
            "/api/todos/sync/pull/",
            json!({"protocol_version":1,"cursor":{"scope":"todo-board","epoch":Uuid::now_v7(),"sequence":0},"limit":10}),
        ),
    ] {
        assert_eq!(
            request(&app, None, method, path, payload).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (_, initial) = request(
        &app,
        Some(&token),
        "GET",
        "/api/todos/sync/snapshot/",
        Value::Null,
    )
    .await;
    let initial: Snapshot = serde_json::from_value(initial).unwrap();
    let id = Uuid::now_v7();
    let push = push(id, "offline");
    let payload = serde_json::to_value(&push).unwrap();
    let (status, accepted) =
        request(&app, Some(&token), "POST", "/api/todos/", payload.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(
        request(&app, Some(&token), "POST", "/api/todos/", payload)
            .await
            .1,
        accepted
    );
    assert_eq!(
        state
            .database()
            .get::<todo::Todo>(id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "offline"
    );
    let (_, page) = request(
        &app,
        Some(&token),
        "POST",
        "/api/todos/sync/pull/",
        serde_json::to_value(PullRequest {
            protocol_version: 1,
            cursor: Some(initial.cursor),
            limit: 10,
        })
        .unwrap(),
    )
    .await;
    let PullResult::Changes {
        groups,
        cursor,
        has_more,
    } = serde_json::from_value(page).unwrap()
    else {
        panic!("expected page")
    };
    assert_eq!(groups.len(), 1);
    assert_eq!(cursor.sequence, 1);
    assert!(!has_more);
    let mut patch = push.clone();
    patch.operation.id = Uuid::now_v7();
    patch.operation.mutations[0].base_revision = 1;
    patch.operation.mutations[0].changes = Some(Record::from([(
        "done".into(),
        che_sync::protocol::Value::Boolean(true),
    )]));
    let (status, updated) = request(
        &app,
        Some(&token),
        "PATCH",
        "/api/todos/",
        serde_json::to_value(&patch).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"][0]["revision"], "2");
    patch.operation.id = Uuid::now_v7();
    let (status, conflict) = request(
        &app,
        Some(&token),
        "PATCH",
        "/api/todos/",
        serde_json::to_value(&patch).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(conflict["data"][0]["revision"], "2");
    patch.operation.id = Uuid::now_v7();
    patch.operation.mutations[0].base_revision = 2;
    patch.operation.mutations[0].changes = None;
    let deletion = serde_json::to_value(&patch).unwrap();
    let (status, deleted) = request(
        &app,
        Some(&token),
        "DELETE",
        "/api/todos/",
        deletion.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["data"][0]["record"].is_null());
    let (status, replay) = request(&app, Some(&token), "DELETE", "/api/todos/", deletion).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, deleted);
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &format!("/api/todos/{id}/"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, snapshot) = request(
        &app,
        Some(&token),
        "GET",
        "/api/todos/sync/snapshot/",
        Value::Null,
    )
    .await;
    assert_eq!(snapshot["cursor"]["sequence"], "3");
    let mut wrong = push.clone();
    wrong.scope = "other-scope".into();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/todos/",
            serde_json::to_value(wrong).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut changed = push;
    changed.operation.mutations[0]
        .changes
        .as_mut()
        .unwrap()
        .insert(
            "title".into(),
            che_sync::protocol::Value::Text("changed retry".into()),
        );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/todos/",
            serde_json::to_value(changed).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn ordinary_uuid_crud_is_journaled_and_delete_leaves_tombstone() {
    let (app, state, token) = app().await;
    let (status, created) = request(
        &app,
        Some(&token),
        "POST",
        "/api/todos/",
        json!({"title":"from REST","done":false}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let detail = format!("/api/todos/{id}/");
    assert_eq!(
        request(&app, Some(&token), "GET", &detail, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let (status, patched) =
        request(&app, Some(&token), "PATCH", &detail, json!({"done":true})).await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["done"], true);
    let (status, updated) = request(
        &app,
        Some(&token),
        "PUT",
        &detail,
        json!({"title":"updated","done":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let (_, before) = request(
        &app,
        Some(&token),
        "GET",
        "/api/todos/sync/snapshot/",
        Value::Null,
    )
    .await;
    let before: Snapshot = serde_json::from_value(before).unwrap();
    assert_eq!(before.cursor.sequence, 3);
    assert_eq!(before.entities[0].revision, 3);
    assert_eq!(
        request(&app, Some(&token), "DELETE", &detail, Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(
        state
            .database()
            .get::<todo::Todo>(id)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_some()
    );
    let (_, after) = request(
        &app,
        Some(&token),
        "GET",
        "/api/todos/sync/snapshot/",
        Value::Null,
    )
    .await;
    let after: Snapshot = serde_json::from_value(after).unwrap();
    assert_eq!(after.cursor.sequence, 4);
    assert_eq!(after.entities[0].revision, 4);
    assert!(after.entities[0].record.is_none());
}

#[tokio::test]
async fn atomic_rejection_and_server_owned_fields_preserve_database_and_cursor() {
    let (app, state, token) = app().await;
    let id = Uuid::now_v7();
    let mut operation = push(id, "first");
    operation.operation.mutations[0]
        .changes
        .as_mut()
        .unwrap()
        .insert("id".into(), che_sync::protocol::Value::Uuid(id));
    let (status, _) = request(
        &app,
        Some(&token),
        "POST",
        "/api/todos/",
        serde_json::to_value(operation).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        state
            .database()
            .get::<todo::Todo>(id)
            .await
            .unwrap()
            .is_none()
    );
    let (_, snapshot) = request(
        &app,
        Some(&token),
        "GET",
        "/api/todos/sync/snapshot/",
        Value::Null,
    )
    .await;
    assert_eq!(snapshot["cursor"]["sequence"], "0");
}

#[tokio::test]
async fn sync_openapi_and_migration_state_are_registered() {
    let (app, state, _) = app().await;
    let metadata = todo::apps().api_endpoints(state);
    let endpoint = metadata.iter().find(|e| e.resource == "todos").unwrap();
    assert_eq!(endpoint.extensions[0].operations.len(), 2);
    for operation in &endpoint.extensions[0].operations {
        assert!(operation.openapi.get("security").is_some());
    }
    let desired = todo::apps().migration_state().unwrap();
    assert!(
        desired
            .models()
            .iter()
            .any(|model| model.key.app == "che_sync")
    );
    assert_eq!(
        desired
            .models()
            .iter()
            .filter(|model| model.key.app == "che_sync")
            .count(),
        4
    );
    let (status, document) = request(&app, None, "GET", "/api/openapi.json", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(document["servers"][0]["url"], "/api");
    assert!(document["paths"].get("/todos/sync/push/").is_none());
    assert_eq!(PushRequest::api_schema().name, "SyncPushRequest");
    let out = std::env::temp_dir().join(format!("todo-sync-types-{}", Uuid::now_v7()));
    che_rest::generate_ts::generate(&out, &metadata, &[], true).unwrap();
    let api = std::fs::read_to_string(out.join("api.ts")).unwrap();
    let models = std::fs::read_to_string(out.join("models.ts")).unwrap();
    assert!(!api.contains("async push("));
    assert!(models.contains("export interface SyncEntity"));
    assert_eq!(
        request(&app, None, "POST", "/api/todos/sync/push/", json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(api.contains("async snapshot(): Promise<SyncSnapshot>"));
    assert!(models.contains("export interface Todo {\n  id: string;"));
    std::fs::remove_dir_all(out).unwrap();
}

#[tokio::test]
async fn http_body_limits_and_incompatible_protocol_are_enforced() {
    let (app, _, token) = app().await;
    let mut incompatible = push(Uuid::now_v7(), "old protocol");
    incompatible.protocol_version = 99;
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/todos/",
            serde_json::to_value(incompatible).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let huge = push(Uuid::now_v7(), &"x".repeat(2 * 1024 * 1024));
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/todos/",
            serde_json::to_value(huge).unwrap()
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

struct DenyWrites;
impl che_sync::AccessPolicy for DenyWrites {
    fn authorize_scope(&self, principal: &str, _: &str) -> Result<(), che_orm::OrmError> {
        if principal == "1" {
            Ok(())
        } else {
            Err(che_orm::OrmError::Configuration("scope denied".into()))
        }
    }
    fn validate(&self, _: &str, _: &str, _: &Entity, _: &Entity) -> Result<(), che_orm::OrmError> {
        Err(che_orm::OrmError::Configuration("writes denied".into()))
    }
}

#[derive(Clone)]
struct RestrictedTodos;
impl che_rest::ViewSet for RestrictedTodos {
    type Model = todo::Todo;
    type Serializer = todo::TodoSerializer;
    type ListSerializer = Self::Serializer;
    type QuerySet = che_orm::DatabaseQuery<todo::Todo>;
    type FilterSet = che_rest::FilterSet<todo::Todo>;
    type Permission = che_rest::IsAuthenticated;
    fn path(&self) -> &'static str {
        "/todos"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        <todo::TodoViewSet as che_rest::ViewSet>::get_queryset(&todo::TodoViewSet)
    }
    fn configure(&self, config: &mut che_rest::ViewSetConfig<Self>) -> che_rest::AppResult<()> {
        config.extend(che_rest::SyncEndpoints::new(self.clone()))
    }
}
impl che_rest::SyncViewSet for RestrictedTodos {
    fn sync_registry(&self) -> che_rest::AppResult<che_sync::Registry> {
        todo::registry()
    }
    fn sync_scope(
        &self,
        _: &che_rest::AppState,
        _: &che_rest::CurrentPrincipal,
    ) -> che_rest::AppResult<String> {
        Ok("todo-board".into())
    }
    fn sync_policy(&self) -> std::sync::Arc<dyn che_sync::AccessPolicy> {
        std::sync::Arc::new(DenyWrites)
    }
}
struct RestrictedApp;
impl che_rest::AppModule for RestrictedApp {
    fn name(&self) -> &'static str {
        "todo"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new().model::<todo::Todo>()
    }
    fn init(&self, context: &mut che_rest::ModuleContext) {
        context.viewset_with(RestrictedTodos);
    }
}

#[tokio::test]
async fn scope_denial_returns_forbidden_and_crud_policy_rejection_rolls_back() {
    use che_rest::auth::{AuthToken, User, generate_token, token_hash};
    let (state, token) = todo::demo_state().await.unwrap();
    let app = Server::new(state.clone())
        .install(
            che_rest::InstalledApps::new()
                .add(che_rest::auth::module())
                .add(todo::ProjectApp)
                .add(RestrictedApp),
        )
        .build()
        .await
        .unwrap();
    let (status, rejected) = request(
        &app,
        Some(&token),
        "POST",
        "/api/todos/",
        json!({"title":"forbidden write","done":false}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    let (_, snapshot) = request(
        &app,
        Some(&token),
        "GET",
        "/api/todos/sync/snapshot/",
        Value::Null,
    )
    .await;
    assert_eq!(snapshot["cursor"]["sequence"], "0");
    assert_eq!(snapshot["entities"], json!([]));
    assert!(
        state
            .database()
            .query::<todo::Todo>()
            .all(state.database())
            .await
            .unwrap()
            .is_empty()
    );
    let other = state
        .database()
        .create::<User>()
        .set(User::USERNAME, "other")
        .set(User::PASSWORD_HASH, "unused")
        .execute()
        .await
        .unwrap();
    let other_token = generate_token();
    state
        .database()
        .create::<AuthToken>()
        .set(AuthToken::USER_ID, other.id)
        .set(AuthToken::KEY_HASH, token_hash(&other_token))
        .execute()
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&other_token),
            "GET",
            "/api/todos/sync/snapshot/",
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
