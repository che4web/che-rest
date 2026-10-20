#![cfg(feature = "sync-http")]
#[path = "../examples/model_sync/support.rs"]
mod notes;

use che_orm::{OrmError, Uuid};
use che_rest::{
    Server,
    sync_http::{RestModelTransport, RestResource},
};
use che_sync::{ModelLogClient, ModelLogTransport, SchemaRole, TransportFuture, model_log::*};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

struct LostReply {
    inner: RestModelTransport,
    lose: AtomicBool,
    requests: Arc<Mutex<Vec<RestModelWrite>>>,
}
impl ModelLogTransport for LostReply {
    fn encode_write(&self, write: RestModelWrite) -> Result<RestModelWrite, OrmError> {
        self.inner.encode_write(write)
    }
    fn write<'a>(&'a self, write: &'a RestModelWrite) -> TransportFuture<'a, ModelWriteResult> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(write.clone());
            let result = self.inner.write(write).await?;
            if self.lose.swap(false, Ordering::SeqCst) {
                Err(OrmError::Interaction("lost reply after commit".into()))
            } else {
                Ok(result)
            }
        })
    }
    fn changes<'a>(
        &'a self,
        model: &'a str,
        after: i64,
        limit: u32,
    ) -> TransportFuture<'a, ModelChanges> {
        self.inner.changes(model, after, limit)
    }
}

#[tokio::test]
async fn offline_coalescing_lost_reply_and_restart_use_immutable_rest_requests() {
    let (state, token) = notes::demo_state().await.unwrap();
    let router = Server::new(state.clone())
        .install(notes::apps())
        .build()
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder()
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("authorization", format!("Token {token}").parse().unwrap());
            headers
        })
        .build()
        .unwrap();
    let transport = || {
        RestModelTransport::new(
            &base,
            http.clone(),
            vec![RestResource::new::<notes::Note, notes::NoteSerializer>("notes").unwrap()],
        )
        .unwrap()
    };
    let db = notes::database(SchemaRole::Client).await.unwrap();
    let client = ModelLogClient::open(db.clone(), notes::registry(SchemaRole::Client))
        .await
        .unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let engine = client.engine(LostReply {
        inner: transport(),
        lose: AtomicBool::new(true),
        requests: requests.clone(),
    });
    engine.pull().await.unwrap();
    let mut note = notes::Note {
        id: Uuid::now_v7(),
        title: "first".into(),
        done: false,
        document: Some(che_orm::JsonField(json!(null))),
        number: i64::MAX,
    };
    for n in 0..10 {
        note.title = format!("offline {n}");
        client.save(&note).await.unwrap();
    }
    assert!(engine.flush().await.is_err());
    assert_eq!(
        state
            .database()
            .get::<notes::Note>(note.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "offline 9"
    );
    // A later edit must not change a request whose server outcome is unknown.
    note.title = "after submission".into();
    client.save(&note).await.unwrap();
    drop(engine);
    drop(client);
    let client = ModelLogClient::open(db, notes::registry(SchemaRole::Client))
        .await
        .unwrap();
    // Authentication failures are retryable and must not settle the outbox.
    let unauthenticated = client.engine(
        RestModelTransport::new(
            &base,
            reqwest::Client::new(),
            vec![RestResource::new::<notes::Note, notes::NoteSerializer>("notes").unwrap()],
        )
        .unwrap(),
    );
    assert!(unauthenticated.flush().await.is_err());
    assert!(client.conflicts().await.unwrap().is_empty());
    drop(unauthenticated);
    let engine = client.engine(LostReply {
        inner: transport(),
        lose: AtomicBool::new(false),
        requests: requests.clone(),
    });
    assert_eq!(engine.flush().await.unwrap().accepted_operations, 1);
    {
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
    }
    let page = transport().changes("notes.note", 0, 100).await.unwrap();
    assert_eq!(page.operations.len(), 1);
    assert_eq!(page.operations[0].kind, ChangeKind::Create);
    assert_eq!(engine.flush().await.unwrap().accepted_operations, 1);
    let saved = state
        .database()
        .get::<notes::Note>(note.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.title, "after submission");
    assert_eq!(saved.number, i64::MAX);
    assert_eq!(saved.document, Some(che_orm::JsonField(json!(null))));
    let page = transport().changes("notes.note", 0, 100).await.unwrap();
    assert_eq!(page.operations.len(), 2);
    assert_eq!(page.operations[1].diff.len(), 1);
    assert_eq!(page.operations[1].diff[0].field, "title");
    engine.pull().await.unwrap();
    // A create/delete that has never been submitted vanishes from the queue.
    let temporary = notes::Note {
        id: Uuid::now_v7(),
        ..note.clone()
    };
    client.save(&temporary).await.unwrap();
    client.delete::<notes::Note>(temporary.id).await.unwrap();
    assert_eq!(engine.flush().await.unwrap().accepted_operations, 0);
    assert_eq!(requests.lock().unwrap().len(), 3);
    client.delete::<notes::Note>(note.id).await.unwrap();
    assert_eq!(engine.flush().await.unwrap().accepted_operations, 1);
    let page = transport().changes("notes.note", 0, 100).await.unwrap();
    assert_eq!(page.operations.len(), 3);
    assert_eq!(page.operations[2].kind, ChangeKind::Delete);
    server.abort();
}

#[tokio::test]
async fn model_journal_routes_require_auth_and_generate_typed_changes_clients() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (state, token) = notes::demo_state().await.unwrap();
    let metadata = notes::apps().api_endpoints(state.clone());
    let endpoint = metadata
        .iter()
        .find(|endpoint| endpoint.resource == "notes")
        .unwrap();
    assert_eq!(endpoint.extensions.len(), 1);
    assert_eq!(endpoint.extensions[0].id, "model-sync");
    assert_eq!(endpoint.extensions[0].operations.len(), 1);
    let output = std::env::temp_dir().join(format!("model-sync-types-{}", Uuid::now_v7()));
    che_rest::generate_ts::generate(&output, &metadata, &[], false).unwrap();
    let models = std::fs::read_to_string(output.join("models.ts")).unwrap();
    assert!(models.contains("export interface ModelChanges"));
    assert!(models.contains("old_value?: JsonValue"));
    let api = std::fs::read_to_string(output.join("api.ts")).unwrap();
    assert!(api.contains("sync/changes/"));
    assert!(!api.contains("sync/pull/"));
    std::fs::remove_dir_all(output).unwrap();
    let router = Server::new(state)
        .install(notes::apps())
        .build()
        .await
        .unwrap();
    for (query, authenticated, expected) in [
        ("after=0&limit=1", false, StatusCode::UNAUTHORIZED),
        ("after=0&limit=1", true, StatusCode::OK),
        ("after=-1&limit=1", true, StatusCode::BAD_REQUEST),
        ("after=0&limit=0", true, StatusCode::BAD_REQUEST),
        ("after=invalid", true, StatusCode::BAD_REQUEST),
    ] {
        let mut request = Request::builder().uri(format!("/api/notes/sync/changes/?{query}"));
        if authenticated {
            request = request.header("authorization", format!("Token {token}"));
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{query}");
        if expected == StatusCode::OK {
            let page: ModelChanges =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(page.model, "notes.note");
            assert_eq!(page.next_cursor, 0);
            assert!(page.operations.is_empty());
            assert!(!page.has_more);
        }
    }
}

#[derive(Clone)]
struct EffectNotes {
    order: Vec<&'static str>,
    fail: Arc<AtomicBool>,
    completed: Arc<Mutex<Vec<String>>>,
}
struct CleanupHook {
    id: &'static str,
    fail: Arc<AtomicBool>,
    completed: Arc<Mutex<Vec<String>>>,
}
impl che_rest::MutationHook<EffectNotes> for CleanupHook {
    fn id(&self) -> &'static str {
        self.id
    }
    fn prepare_delete(&self, note: &notes::Note) -> serde_json::Value {
        json!({"version": 1, "owner": self.id, "key": note.id})
    }
    fn after_delete(
        &self,
        _: &che_rest::AppState,
        payload: &serde_json::Value,
    ) -> che_rest::AppResult<()> {
        assert_eq!(payload["version"], 1);
        assert_eq!(
            payload["owner"], self.id,
            "cleanup payload changed ownership"
        );
        assert!(payload["key"].as_str().unwrap().parse::<Uuid>().is_ok());
        if self.fail.load(Ordering::SeqCst) {
            return Err(che_rest::AppError::BadRequest(
                "external service unavailable".into(),
            ));
        }
        self.completed.lock().unwrap().push(self.id.into());
        Ok(())
    }
}
impl che_rest::ViewSet for EffectNotes {
    type Model = notes::Note;
    type Serializer = notes::NoteSerializer;
    type ListSerializer = notes::NoteSerializer;
    type QuerySet = che_orm::DatabaseQuery<notes::Note>;
    type FilterSet = che_rest::FilterSet<notes::Note>;
    type Permission = che_rest::IsAuthenticated;
    fn path(&self) -> &'static str {
        "/notes"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        use che_orm::Model;
        che_orm::DatabaseQuery::new(notes::Note::query())
    }
    fn configure(&self, config: &mut che_rest::ViewSetConfig<Self>) -> che_rest::AppResult<()> {
        for id in &self.order {
            config.mutation_hook(CleanupHook {
                id,
                fail: self.fail.clone(),
                completed: self.completed.clone(),
            })?;
        }
        config.extend(che_rest::ModelSyncEndpoints::new(self.clone()))
    }
}
impl che_rest::SyncViewSet for EffectNotes {
    fn sync_registry(&self) -> che_rest::AppResult<che_sync::Registry> {
        Ok(notes::registry(SchemaRole::Server))
    }
}
impl che_rest::AppModule for EffectNotes {
    fn name(&self) -> &'static str {
        "notes"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new().server_model::<notes::Note>()
    }
    fn sync_registry(&self) -> Option<che_sync::Registry> {
        Some(notes::registry(SchemaRole::Server))
    }
    fn init(&self, context: &mut che_rest::ModuleContext) {
        context.viewset_with(self.clone());
    }
}

#[tokio::test]
async fn cleanup_survives_restart_missing_handlers_and_hook_reordering_without_new_requests() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (state, token) = notes::demo_state().await.unwrap();
    let hooks = EffectNotes {
        order: vec!["storage-cleanup", "search-cleanup"],
        fail: Arc::new(AtomicBool::new(true)),
        completed: Arc::default(),
    };
    let router = Server::new(state.clone())
        .install(
            che_rest::InstalledApps::new()
                .add(che_rest::auth::module())
                .add(hooks.clone()),
        )
        .build()
        .await
        .unwrap();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/notes/")
                .header("authorization", format!("Token {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"cleanup", "done":false, "number":17, "document":null})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let response = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/notes/{}/", created["id"].as_str().unwrap()))
                .header("authorization", format!("Token {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let pending = state
        .database()
        .query_sql("SELECT payload FROM _sync_effects ORDER BY id", vec![])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String>(0).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 2);
    assert!(hooks.completed.lock().unwrap().is_empty());
    // Restart with handlers temporarily removed: durable effects must survive.
    let restarted = che_rest::AppState::from_database(state.database().clone());
    let _router = Server::new(restarted.clone())
        .install(che_rest::InstalledApps::new().add(EffectNotes {
            order: vec![],
            ..hooks.clone()
        }))
        .build()
        .await
        .unwrap();
    assert_eq!(
        che_rest::sync::retry_model_effects(&restarted)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        restarted
            .database()
            .query_sql("SELECT payload FROM _sync_effects ORDER BY id", vec![])
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String>(0).unwrap())
            .collect::<Vec<_>>(),
        pending
    );
    let restarted = che_rest::AppState::from_database(state.database().clone());
    hooks.fail.store(false, Ordering::SeqCst);
    let _router = Server::new(restarted.clone())
        .install(che_rest::InstalledApps::new().add(EffectNotes {
            order: vec!["search-cleanup", "storage-cleanup"],
            ..hooks.clone()
        }))
        .build()
        .await
        .unwrap();
    assert_eq!(
        che_rest::sync::retry_model_effects(&restarted)
            .await
            .unwrap(),
        2
    );
    let mut completed = hooks.completed.lock().unwrap().clone();
    completed.sort();
    assert_eq!(completed, vec!["search-cleanup", "storage-cleanup"]);
    assert_eq!(
        che_rest::sync::retry_model_effects(&restarted)
            .await
            .unwrap(),
        0
    );
    assert!(
        restarted
            .database()
            .query_sql("SELECT id FROM _sync_effects", vec![])
            .await
            .unwrap()
            .is_empty()
    );
    // Background retries cannot append mutations or new journal operations.
    assert_eq!(
        restarted
            .database()
            .query_sql("SELECT id FROM notes_sync_log", vec![])
            .await
            .unwrap()
            .len(),
        2
    );
}
