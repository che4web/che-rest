use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::{
    Database, DatabaseQuery, Migration, MigrationGraph, MigrationId, MigrationOperation, Model,
    ModelSerializer, SchemaSet, TableState, Uuid,
};
use che_rest::*;
use che_sync::{AccessPolicy, AllowAll, Field, Registry, SchemaRole, SyncModel, protocol::*};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Debug, Model)]
#[orm(table = "lifecycle_parents")]
struct Parent {
    #[orm(primary_key)]
    id: Uuid,
    title: String,
    #[orm(default = "false", db_column = "approval_flag")]
    approved: bool,
    #[orm(default = "false", db_column = "approved")]
    public_approved: bool,
    #[orm(default = "0")]
    count: i64,
    #[orm(default = "0")]
    revision: i64,
    #[orm(auto_now)]
    updated_at: che_orm::time::OffsetDateTime,
    deleted_at: Option<che_orm::time::OffsetDateTime>,
    #[orm(default = "''")]
    sync_scope: String,
}
#[derive(Debug, Model)]
#[orm(table = "lifecycle_children")]
struct Child {
    #[orm(primary_key)]
    id: Uuid,
    #[orm(foreign_key = Parent, on_delete = "cascade")]
    parent_id: Uuid,
    title: String,
    #[orm(default = "0")]
    revision: i64,
    #[orm(auto_now)]
    updated_at: che_orm::time::OffsetDateTime,
    deleted_at: Option<che_orm::time::OffsetDateTime>,
    #[orm(default = "''")]
    sync_scope: String,
}
impl SyncModel for Parent {
    const SYNC_NAME: &'static str = "lifecycle.parent";
    fn sync_fields() -> Vec<Field> {
        vec![
            Field::server_owned("id", "id"),
            Field::writable("title", "title"),
            Field::server_owned("approved", "approval_flag"),
            Field::writable("public_approved", "approved"),
            Field::writable("count", "count"),
        ]
    }
}
impl SyncModel for Child {
    const SYNC_NAME: &'static str = "lifecycle.child";
    fn sync_fields() -> Vec<Field> {
        vec![
            Field::server_owned("id", "id"),
            Field::writable("parent", "parent_id"),
            Field::writable("title", "title"),
        ]
    }
}
#[derive(ModelSerializer)]
#[serializer(model = Parent)]
struct ParentSerializer {
    #[serializer(read_only)]
    id: Uuid,
    title: String,
    #[serializer(read_only)]
    approved: bool,
    count: i64,
}
#[derive(ModelSerializer)]
#[serializer(model = Parent)]
struct UnsafeSerializer {
    #[serializer(read_only)]
    id: Uuid,
    title: String,
    approved: bool,
}
#[derive(ModelSerializer)]
#[serializer(model = Child)]
struct ChildSerializer {
    #[serializer(read_only)]
    id: Uuid,
    parent_id: Uuid,
    title: String,
}
fn registry() -> Registry {
    Registry::new()
        .register::<Parent>()
        .unwrap()
        .register::<Child>()
        .unwrap()
}
type Events = Arc<Mutex<Vec<String>>>;
#[derive(Clone)]
struct Hook(Events);
impl<V: ViewSet> MutationHook<V> for Hook {
    fn id(&self) -> &'static str {
        "lifecycle-test"
    }
    fn prepare_delete_async<'a>(
        &'a self,
        state: &'a AppState,
        model: &'a V::Model,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<Value>> + Send + 'a>> {
        Box::pin(async move {
            assert!(
                state
                    .database()
                    .get::<V::Model>(model.primary_key_value())
                    .await?
                    .is_some(),
                "delete preparation ran after commit"
            );
            Ok(json!(V::Model::table_name()))
        })
    }
    fn after_delete(&self, _: &AppState, payload: &Value) -> AppResult<()> {
        self.0
            .lock()
            .unwrap()
            .push(format!("delete:{}", payload.as_str().unwrap()));
        Ok(())
    }
    fn after_commit(
        &self,
        _: &AppState,
        _: Option<&CurrentPrincipal>,
        action: ViewAction,
        _: &V::Model,
    ) -> AppResult<()> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{action:?}:{}", V::Model::table_name()));
        Ok(())
    }
}
#[derive(Clone)]
struct Parents(Events);
#[derive(Clone)]
struct RetryHook(Events);
impl<V: ViewSet> MutationHook<V> for RetryHook {
    fn id(&self) -> &'static str {
        "retry-test"
    }
    fn after_commit(
        &self,
        _: &AppState,
        _: Option<&CurrentPrincipal>,
        _: ViewAction,
        _: &V::Model,
    ) -> AppResult<()> {
        let mut events = self.0.lock().unwrap();
        if let Some(index) = events.iter().position(|event| event == "@fail-once") {
            events.remove(index);
            events.push("retry:failed".into());
            return Err(AppError::BadRequest("retry later".into()));
        }
        if events.iter().any(|event| event == "retry:failed") {
            events.push("retry:success".into());
        }
        Ok(())
    }
}
#[derive(Clone)]
struct UnsafeParents;
macro_rules! viewset {
    ($view:ty, $model:ty, $serializer:ty, $path:literal) => {
        impl ViewSet for $view {
            type Model = $model;
            type Serializer = $serializer;
            type ListSerializer = $serializer;
            type QuerySet = DatabaseQuery<$model>;
            type FilterSet = FilterSet<$model>;
            type Permission = IsAuthenticated;
            fn path(&self) -> &'static str {
                $path
            }
            fn get_queryset(&self) -> Self::QuerySet {
                DatabaseQuery::new(<$model>::query())
            }
            fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
                config.extend(SyncEndpoints::new(self.clone()))
            }
        }
        impl SyncViewSet for $view {
            fn sync_registry(&self) -> AppResult<Registry> {
                Ok(registry())
            }
            fn sync_scope(&self, _: &AppState, _: &CurrentPrincipal) -> AppResult<String> {
                Ok("lifecycle".into())
            }
            fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
                Arc::new(AllowAll)
            }
        }
    };
}
viewset!(UnsafeParents, Parent, UnsafeSerializer, "/unsafe");
impl ViewSet for Parents {
    type Model = Parent;
    type Serializer = ParentSerializer;
    type ListSerializer = ParentSerializer;
    type QuerySet = DatabaseQuery<Parent>;
    type FilterSet = FilterSet<Parent>;
    type Permission = IsAuthenticated;
    fn path(&self) -> &'static str {
        "/parents"
    }
    fn signal_access(&self, _: ViewAction) -> Option<SignalAccess> {
        Some(SignalAccess::Authenticated)
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Parent::query())
    }
    fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
        config.mutation_hook(RetryHook(self.0.clone()))?;
        config.mutation_hook(Hook(self.0.clone()))?;
        config.extend(SyncEndpoints::new(self.clone()))
    }
    fn prepare_create(
        &self,
        _: &AppState,
        _: Option<&CurrentPrincipal>,
        write: ValidatedWrite<Parent>,
    ) -> AppResult<ValidatedWrite<Parent>> {
        Ok(match write {
            ValidatedWrite::Create(query) => {
                ValidatedWrite::Create(query.set(Parent::APPROVED, true))
            }
            other => other,
        })
    }
}
impl SyncViewSet for Parents {
    fn sync_registry(&self) -> AppResult<Registry> {
        Ok(registry())
    }
    fn sync_scope(&self, _: &AppState, _: &CurrentPrincipal) -> AppResult<String> {
        Ok("lifecycle".into())
    }
    fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
        Arc::new(AllowAll)
    }
}
struct App(Events);
impl AppModule for App {
    fn name(&self) -> &'static str {
        "lifecycle"
    }
    fn schema(&self) -> SchemaSet {
        SchemaSet::new().model::<Parent>().model::<Child>()
    }
    fn sync_registry(&self) -> Option<Registry> {
        Some(registry())
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(Parents(self.0.clone()));
        context.viewset_with(ChildHooks(self.0.clone()));
    }
}
// Attach the same observer to the child so parent-route cascade dispatch is tested.
#[derive(Clone)]
struct ChildHooks(Events);
impl ViewSet for ChildHooks {
    type Model = Child;
    type Serializer = ChildSerializer;
    type ListSerializer = ChildSerializer;
    type QuerySet = DatabaseQuery<Child>;
    type FilterSet = FilterSet<Child>;
    type Permission = IsAuthenticated;
    fn path(&self) -> &'static str {
        "/children"
    }
    fn signal_access(&self, _: ViewAction) -> Option<SignalAccess> {
        Some(SignalAccess::Authenticated)
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Child::query())
    }
    fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
        config.mutation_hook(Hook(self.0.clone()))?;
        config.extend(SyncEndpoints::new(self.clone()))
    }
}
impl SyncViewSet for ChildHooks {
    fn sync_registry(&self) -> AppResult<Registry> {
        Ok(registry())
    }
    fn sync_scope(&self, _: &AppState, _: &CurrentPrincipal) -> AppResult<String> {
        Ok("lifecycle".into())
    }
    fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
        Arc::new(AllowAll)
    }
}

async fn setup() -> (AppState, Router, String, Events) {
    let state = AppState::from_database(Database::connect_in_memory().await.unwrap());
    let mut migrations = auth::migrations();
    let mut operations = vec![
        MigrationOperation::CreateTable {
            table: TableState::from(&Parent::schema()),
        },
        MigrationOperation::CreateTable {
            table: TableState::from(&Child::schema()),
        },
    ];
    operations.extend(registry().migration_operations(SchemaRole::Server));
    migrations.push(Migration::new(
        MigrationId {
            app: "lifecycle".into(),
            name: "0001".into(),
        },
        vec![],
        operations,
    ));
    state
        .database()
        .migrate(MigrationGraph::new(migrations).unwrap())
        .await
        .unwrap();
    let user = state
        .database()
        .create::<auth::User>()
        .set(auth::User::USERNAME, "observer")
        .set(auth::User::PASSWORD_HASH, "unused")
        .execute()
        .await
        .unwrap();
    let token = auth::generate_token();
    state
        .database()
        .create::<auth::AuthToken>()
        .set(auth::AuthToken::USER_ID, user.id)
        .set(auth::AuthToken::KEY_HASH, auth::token_hash(&token))
        .execute()
        .await
        .unwrap();
    let events = Events::default();
    let router = Server::new(state.clone())
        .install(
            InstalledApps::new()
                .add(auth::module())
                .add(App(events.clone())),
        )
        .build()
        .await
        .unwrap();
    (state, router, token, events)
}
async fn post(router: &Router, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let mut method = axum::http::Method::POST;
    let mut path = path.to_owned();
    let mut body = body;
    let mut headers = axum::http::HeaderMap::new();
    if body.get("operation").is_some() {
        let request: PushRequest = serde_json::from_value(body).unwrap();
        let write = match request.operation.mutations[0].model.as_str() {
            "lifecycle.parent" => {
                RestSyncRequest::new::<Parent, ParentSerializer>(request).unwrap()
            }
            "lifecycle.child" => RestSyncRequest::new::<Child, ChildSerializer>(request).unwrap(),
            _ => panic!("unknown model"),
        };
        method = write.method;
        if method != axum::http::Method::POST {
            path = format!("{}{}/", path, write.entity_id);
        }
        headers = write.headers;
        body = write.body;
    }
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Token {token}"))
        .header("Content-Type", "application/json");
    builder.headers_mut().unwrap().extend(headers);
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap(),
    )
}
#[test]
fn writable_server_owned_serializer_is_rejected_at_installation() {
    let mut config = ViewSetConfig::<UnsafeParents>::new("/unsafe");
    assert!(UnsafeParents.configure(&mut config).is_err());
}

#[tokio::test]
async fn failed_effect_is_durable_and_does_not_skip_other_handlers_on_replay() {
    let (state, router, token, events) = setup().await;
    events.lock().unwrap().push("@fail-once".into());
    let body = json!({"protocol_version":1,"scope":"lifecycle","replica_id":Uuid::now_v7(),"operation":{"id":Uuid::now_v7(),"mutations":[{"model":"lifecycle.parent","id":Uuid::now_v7(),"schema_version":1,"base_revision":"0","changes":{"title":{"type":"text","value":"retry cleanup"},"count":{"type":"integer","value":"9223372036854775807"}}}]}});
    let (status, result) = post(&router, &token, "/api/parents/", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    assert_eq!(
        result["data"][0]["record"]["count"]["value"],
        i64::MAX.to_string()
    );
    assert_eq!(
        result["data"][0]["record"]["approved"]["value"], true,
        "sync create must run prepare_create"
    );
    assert_eq!(
        state
            .database()
            .query_sql("SELECT id FROM _sync_effects", vec![])
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "Create:lifecycle_parents")
    );
    let (status, replay) = post(&router, &token, "/api/parents/", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replay, result);
    assert!(
        state
            .database()
            .query_sql("SELECT id FROM _sync_effects", vec![])
            .await
            .unwrap()
            .is_empty()
    );
    post(&router, &token, "/api/parents/", body).await;
    let events = events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.as_str() == "retry:success")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.as_str() == "Create:lifecycle_parents")
            .count(),
        1
    );
}

#[test]
fn generated_sync_contract_uses_lossless_integer_strings() {
    let push = PushRequest::api_schema();
    assert!(push.typescript.contains("type: 'integer'; value: string"));
    assert!(push.typescript.contains("revision: string"));
    assert!(push.typescript.contains("base_revision: string"));
    assert!(push.typescript.contains("sequence: string"));
    let pull = PullRequest::api_schema();
    assert_eq!(
        pull.openapi["properties"]["cursor"]["properties"]["sequence"]["type"],
        "string"
    );
}
#[tokio::test]
async fn trusted_preparation_and_sync_cascade_hooks_work_without_duplicate_replay_events() {
    let (state, router, token, events) = setup().await;
    let (status, parent) = post(
        &router,
        &token,
        "/api/parents/",
        json!({"title":"trusted create", "count":0}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(parent["approved"], true);
    let parent_id: Uuid = serde_json::from_value(parent["id"].clone()).unwrap();
    let child_id = Uuid::now_v7();
    let mut created_signal = state.signals().subscribe("children.created");
    let mut parent_deleted_signal = state.signals().subscribe("parents.deleted");
    let mut child_deleted_signal = state.signals().subscribe("children.deleted");
    let make_request = |mutations| PushRequest {
        protocol_version: PROTOCOL_VERSION,
        scope: "lifecycle".into(),
        replica_id: Uuid::now_v7(),
        operation: Operation {
            id: Uuid::now_v7(),
            mutations,
        },
    };
    let create = make_request(vec![Mutation {
        model: Child::SYNC_NAME.into(),
        id: child_id,
        schema_version: 1,
        base_revision: 0,
        changes: Some(Record::from([
            ("parent".into(), che_sync::protocol::Value::Uuid(parent_id)),
            (
                "title".into(),
                che_sync::protocol::Value::Text("child".into()),
            ),
        ])),
    }]);
    let (_, accepted) = post(
        &router,
        &token,
        "/api/children/",
        serde_json::to_value(&create).unwrap(),
    )
    .await;
    assert_eq!(accepted["status"], "accepted");
    let (_, replay) = post(
        &router,
        &token,
        "/api/children/",
        serde_json::to_value(&create).unwrap(),
    )
    .await;
    assert_eq!(replay, accepted);
    assert_eq!(created_signal.try_recv().unwrap(), json!({"id": child_id}));
    assert!(created_signal.try_recv().is_err());
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["Create:lifecycle_parents", "Create:lifecycle_children"]
    );
    let delete = make_request(vec![Mutation {
        model: Parent::SYNC_NAME.into(),
        id: parent_id,
        schema_version: 1,
        base_revision: 1,
        changes: None,
    }]);
    let mut stale_delete = delete.clone();
    stale_delete.operation.id = Uuid::now_v7();
    stale_delete.operation.mutations[0].base_revision = 0;
    let (_, conflict) = post(
        &router,
        &token,
        "/api/parents/",
        serde_json::to_value(&stale_delete).unwrap(),
    )
    .await;
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(events.lock().unwrap().len(), 2);
    let (_, accepted) = post(
        &router,
        &token,
        "/api/parents/",
        serde_json::to_value(&delete).unwrap(),
    )
    .await;
    assert_eq!(accepted["status"], "accepted");
    let (_, replay) = post(
        &router,
        &token,
        "/api/parents/",
        serde_json::to_value(&delete).unwrap(),
    )
    .await;
    assert_eq!(replay, accepted);
    assert_eq!(
        parent_deleted_signal.try_recv().unwrap(),
        json!({"id": parent_id})
    );
    assert_eq!(
        child_deleted_signal.try_recv().unwrap(),
        json!({"id": child_id})
    );
    assert!(parent_deleted_signal.try_recv().is_err());
    assert!(child_deleted_signal.try_recv().is_err());
    assert!(
        state
            .database()
            .get::<Child>(child_id)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_some()
    );
    let events = events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.as_str() == "delete:lifecycle_parents")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.as_str() == "delete:lifecycle_children")
            .count(),
        1
    );
}
