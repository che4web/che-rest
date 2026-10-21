#[path = "object_sync/support.rs"]
mod support;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::{Database, Model, OrmError, Uuid};
use che_rest::{
    Server,
    auth::{Group, access::*},
};
use che_sync::{
    ModelLogClient, ModelLogTransport, SchemaRole, SyncModel, TransportFuture, model_log::*,
    object_log::*,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use support::*;
use tower::ServiceExt;

#[derive(Clone)]
struct Transport {
    router: Router,
    token: &'static str,
    lose: Arc<AtomicBool>,
    sent: Arc<Mutex<Vec<Value>>>,
}
async fn call(
    router: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
    headers: &[(&str, String)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Token {token}"))
        .header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let response = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)))
    };
    (status, value)
}
impl ModelLogTransport for Transport {
    fn id(&self) -> &'static str {
        "test.object.v3"
    }
    fn write<'a>(
        &'a self,
        _: &'a ModelWrite,
        _: &'a Value,
    ) -> TransportFuture<'a, ModelWriteResult> {
        Box::pin(async { Err(OrmError::Configuration("v2 unsupported".into())) })
    }
    fn changes<'a>(&'a self, _: &'a str, _: i64, _: u32) -> TransportFuture<'a, ModelChanges> {
        Box::pin(async { Err(OrmError::Configuration("v2 unsupported".into())) })
    }
    fn object_changes<'a>(
        &'a self,
        _: &'a str,
        cursor: Option<ObjectCursor>,
        limit: u32,
    ) -> TransportFuture<'a, ObjectChanges> {
        Box::pin(async move {
            let (status, value) = call(
                &self.router,
                self.token,
                "POST",
                "/api/notes/sync/objects/",
                json!({"protocol_version":3,"cursor":cursor,"limit":limit}),
                &[],
            )
            .await;
            if status != StatusCode::OK {
                return Err(OrmError::Interaction(format!("{status}: {value}")));
            }
            serde_json::from_value(value).map_err(|e| OrmError::Configuration(e.to_string()))
        })
    }
    fn object_write<'a>(
        &'a self,
        write: &'a ModelWrite,
        payload: &'a Value,
    ) -> TransportFuture<'a, ObjectWriteResult> {
        Box::pin(async move {
            self.sent.lock().unwrap().push(payload.clone());
            let frozen: ModelWrite = serde_json::from_value(payload.clone()).unwrap();
            assert_eq!(&frozen, write);
            let method = match write.kind {
                ChangeKind::Create => "POST",
                ChangeKind::Update => "PATCH",
                ChangeKind::Delete => "DELETE",
            };
            let path = if write.kind == ChangeKind::Create {
                "/api/notes/".into()
            } else {
                format!("/api/notes/{}/", write.record_id)
            };
            let (status, value) = call(
                &self.router,
                self.token,
                method,
                &path,
                write
                    .changes
                    .clone()
                    .map(Value::Object)
                    .unwrap_or(Value::Null),
                &[
                    ("x-sync-protocol", "3".into()),
                    ("x-sync-replica", write.replica_id.to_string()),
                    ("idempotency-key", write.operation_id.to_string()),
                    ("if-match", format!("\"{}\"", write.base_revision)),
                    ("x-sync-version", write.schema_version.to_string()),
                    ("x-sync-entity", write.record_id.to_string()),
                ],
            )
            .await;
            if self.lose.swap(false, Ordering::SeqCst) {
                return Err(OrmError::Interaction("reply lost after commit".into()));
            }
            serde_json::from_value(value.clone())
                .map_err(|e| OrmError::Interaction(format!("{status}: {value}: {e}")))
        })
    }
}
async fn setup() -> (che_rest::AppState, [i64; 3], Router) {
    let (state, ids) = support::state().await;
    let router = Server::new(state.clone())
        .install(apps())
        .build()
        .await
        .unwrap();
    (state, ids, router)
}
fn transport(router: &Router, token: &'static str) -> Transport {
    Transport {
        router: router.clone(),
        token,
        lose: Arc::new(AtomicBool::new(false)),
        sent: Arc::new(Mutex::new(vec![])),
    }
}
async fn client(db: Database, account: &str) -> ModelLogClient {
    ModelLogClient::open_objects(db, registry(SchemaRole::Client), "server-one", account)
        .await
        .unwrap()
}
async fn allow(
    state: &che_rest::AppState,
    user: i64,
    id: Uuid,
    permission: &'static str,
    value: bool,
) {
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                if value {
                    Access::<Note>::grant(tx, Subject::User(user), permission, id).await
                } else {
                    Access::<Note>::revoke(tx, Subject::User(user), permission, id).await
                }
            })
        })
        .await
        .unwrap();
}
async fn create(router: &Router) -> Uuid {
    let (status, body) = call(
        router,
        "alice",
        "POST",
        "/api/notes/",
        json!({"title":"shared","done":false}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body["id"].clone()).unwrap()
}

#[tokio::test]
async fn one_uuid_two_users_common_revisions_merge_and_private_third_user() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    let a = client(database(SchemaRole::Client).await, "alice").await;
    let b = client(database(SchemaRole::Client).await, "bob").await;
    let c = client(database(SchemaRole::Client).await, "carol").await;
    let ea = a.engine(transport(&router, "alice"));
    let eb = b.engine(transport(&router, "bob"));
    let ec = c.engine(transport(&router, "carol"));
    ea.pull().await.unwrap();
    eb.pull().await.unwrap();
    ec.pull().await.unwrap();
    assert!(b.database().get::<Note>(id).await.unwrap().is_none());
    allow(&state, ids[1], id, VIEW, true).await;
    allow(&state, ids[1], id, CHANGE, true).await;
    eb.pull().await.unwrap();
    assert_eq!(
        b.database().get::<Note>(id).await.unwrap().unwrap().title,
        "shared"
    );
    let mut na = a.database().get::<Note>(id).await.unwrap().unwrap();
    let mut nb = b.database().get::<Note>(id).await.unwrap().unwrap();
    na.title = "Alice's title".into();
    nb.done = true;
    a.save(&na).await.unwrap();
    b.save(&nb).await.unwrap();
    assert_eq!(ea.flush().await.unwrap().accepted_operations, 1);
    let report = eb.flush().await.unwrap();
    assert_eq!(report.conflicts, 0);
    assert_eq!(eb.flush().await.unwrap().accepted_operations, 1);
    ea.pull().await.unwrap();
    ec.pull().await.unwrap();
    let merged = a.database().get::<Note>(id).await.unwrap().unwrap();
    assert!(merged.done);
    assert_eq!(merged.title, "Alice's title");
    assert!(c.database().get::<Note>(id).await.unwrap().is_none());
    let (status, body) = call(&router, "carol", "GET", "/api/notes/", Value::Null, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 0);
    assert_eq!(
        state
            .database()
            .query_sql("SELECT revision FROM object_notes", vec![])
            .await
            .unwrap()[0]
            .get::<i64>(0)
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn group_revocation_hides_projection_preserves_draft_and_requires_explicit_resume() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    let bob = ids[1];
    let group = state
        .database()
        .create::<Group>()
        .set(Group::NAME, "readers")
        .execute()
        .await
        .unwrap()
        .id;
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                add_member(tx, bob, group).await?;
                Access::<Note>::grant(tx, Subject::Group(group), VIEW, id).await?;
                Access::<Note>::grant(tx, Subject::Group(group), CHANGE, id).await
            })
        })
        .await
        .unwrap();
    let b = client(database(SchemaRole::Client).await, "bob").await;
    let eb = b.engine(transport(&router, "bob"));
    eb.pull().await.unwrap();
    let mut draft = b.database().get::<Note>(id).await.unwrap().unwrap();
    draft.title = "offline draft".into();
    b.save(&draft).await.unwrap();
    state
        .database()
        .transaction_result(move |tx| Box::pin(async move { remove_member(tx, bob, group).await }))
        .await
        .unwrap();
    assert!(eb.pull().await.unwrap().blocked);
    assert!(b.database().get::<Note>(id).await.unwrap().is_none());
    assert_eq!(b.database().count_query(Note::query()).await.unwrap(), 0);
    assert!(b.save(&draft).await.is_err());
    assert!(eb.flush().await.unwrap().blocked);
    let statuses = b.object_status().await.unwrap();
    assert_eq!(statuses[0].status, "revoked");
    assert!(statuses[0].has_draft);
    assert!(!statuses[0].submitted);
    assert!(
        b.retained_draft(Note::SYNC_NAME, id)
            .await
            .unwrap()
            .is_some()
    );
    state
        .database()
        .transaction_result(move |tx| Box::pin(async move { add_member(tx, bob, group).await }))
        .await
        .unwrap();
    eb.pull().await.unwrap();
    assert_eq!(b.object_status().await.unwrap()[0].status, "review");
    assert_eq!(eb.flush().await.unwrap().accepted_operations, 0);
    b.resume_object(Note::SYNC_NAME, id).await.unwrap();
    assert_eq!(eb.flush().await.unwrap().accepted_operations, 1);
    assert_eq!(
        state
            .database()
            .get::<Note>(id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "offline draft"
    );
}

#[tokio::test]
async fn lost_reply_retries_original_identity_after_revoke_without_revealing_receipt() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    let db = database(SchemaRole::Client).await;
    let a = client(db.clone(), "alice").await;
    let t = transport(&router, "alice");
    let ea = a.engine(t.clone());
    ea.pull().await.unwrap();
    let mut n = a.database().get::<Note>(id).await.unwrap().unwrap();
    n.title = "committed secret".into();
    a.save(&n).await.unwrap();
    t.lose.store(true, Ordering::SeqCst);
    assert!(ea.flush().await.is_err());
    for p in [VIEW, CHANGE, DELETE] {
        allow(&state, ids[0], id, p, false).await;
    }
    // Pull must hide the object even with an unknown submitted outcome.
    ea.pull().await.unwrap();
    assert!(a.database().get::<Note>(id).await.unwrap().is_none());
    assert!(a.object_status().await.unwrap()[0].submitted);
    assert!(a.discard_retained_draft(Note::SYNC_NAME, id).await.is_err());
    drop(ea);
    drop(a);
    let a = client(db.clone(), "alice").await;
    let ea = a.engine(t.clone());
    let report = ea.flush().await.unwrap();
    assert_eq!(report.accepted_operations, 1);
    assert!(report.blocked);
    assert!(a.database().get::<Note>(id).await.unwrap().is_none());
    assert!(!a.object_status().await.unwrap()[0].submitted);
    {
        let sent = t.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
    }
    assert!(
        state
            .database()
            .query_sql("SELECT result FROM _sync_receipts", vec![])
            .await
            .unwrap()
            .iter()
            .any(|r| r.get::<String>(0).unwrap().contains("committed secret"))
    );
    assert!(
        ModelLogClient::open_objects(db, registry(SchemaRole::Client), "server-one", "bob")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stale_pages_are_redacted_foreign_cursors_rejected_and_late_grants_are_delivered() {
    let (state, ids, router) = setup().await;
    let first = create(&router).await;
    let second = create(&router).await;
    for id in [first, second] {
        allow(&state, ids[1], id, VIEW, true).await;
    }
    let b = transport(&router, "bob");
    let c = transport(&router, "carol");
    let initial = b.object_changes(Note::SYNC_NAME, None, 1).await.unwrap();
    assert!(initial.has_more);
    let id = match &initial.events[0].change {
        ObjectChange::State { state, .. } => state.record_id,
        _ => panic!(),
    };
    allow(&state, ids[1], id, VIEW, false).await;
    let replay = b.object_changes(Note::SYNC_NAME, None, 1).await.unwrap();
    assert!(matches!(
        replay.events[0].change,
        ObjectChange::Revoked { .. }
    ));
    assert!(!serde_json::to_string(&replay).unwrap().contains("shared"));
    assert!(
        c.object_changes(Note::SYNC_NAME, Some(initial.cursor.clone()), 1)
            .await
            .is_err()
    );
    let mut cursor = initial.cursor;
    let mut revoked = false;
    loop {
        let p = b
            .object_changes(Note::SYNC_NAME, Some(cursor), 1)
            .await
            .unwrap();
        revoked |= p
            .events
            .iter()
            .any(|e| matches!(e.change,ObjectChange::Revoked{record_id} if record_id==id));
        cursor = p.cursor;
        if !p.has_more {
            break;
        }
    }
    assert!(revoked);
    allow(&state, ids[1], id, VIEW, true).await;
    let grant = b
        .object_changes(Note::SYNC_NAME, Some(cursor), 10)
        .await
        .unwrap();
    assert!(
        matches!(&grant.events[0].change,ObjectChange::State{state,..} if state.record_id==id && state.revision==1)
    );
    let (v2, _) = call(
        &router,
        "bob",
        "GET",
        "/api/notes/sync/changes/",
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(v2, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn explicit_upgrade_keeps_dirty_data_and_rejects_implicit_protocol_switches() {
    let db = database(SchemaRole::Client).await;
    let old = ModelLogClient::open(db.clone(), registry(SchemaRole::Client))
        .await
        .unwrap();
    let id = Uuid::now_v7();
    old.save(&Note {
        id,
        title: "retained v2 draft".into(),
        done: false,
    })
    .await
    .unwrap();
    drop(old);
    assert!(
        ModelLogClient::open_objects(
            db.clone(),
            registry(SchemaRole::Client),
            "server-one",
            "alice"
        )
        .await
        .is_err()
    );
    let upgraded = ModelLogClient::upgrade_to_objects(
        db.clone(),
        registry(SchemaRole::Client),
        "server-one",
        "alice",
    )
    .await
    .unwrap();
    assert_eq!(
        upgraded
            .retained_draft(Note::SYNC_NAME, id)
            .await
            .unwrap()
            .unwrap()["title"],
        che_sync::protocol::Value::Text("retained v2 draft".into())
    );
    assert!(
        ModelLogClient::open(db, registry(SchemaRole::Client))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn change_only_updates_acknowledge_without_content_and_revoking_change_keeps_reading() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    allow(&state, ids[1], id, CHANGE, true).await;
    let headers = [
        ("x-sync-protocol", "3".into()),
        ("x-sync-replica", Uuid::now_v7().to_string()),
        ("idempotency-key", Uuid::now_v7().to_string()),
        ("if-match", "\"1\"".into()),
        ("x-sync-version", "1".into()),
    ];
    let (status, body) = call(
        &router,
        "bob",
        "PATCH",
        &format!("/api/notes/{id}/"),
        json!({"title":"blind update"}),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "acknowledged");
    assert!(!body.to_string().contains("blind update"));
    allow(&state, ids[1], id, VIEW, true).await;
    let b = client(database(SchemaRole::Client).await, "bob").await;
    let eb = b.engine(transport(&router, "bob"));
    eb.pull().await.unwrap();
    let mut note = b.database().get::<Note>(id).await.unwrap().unwrap();
    assert_eq!(note.title, "blind update");
    note.done = true;
    b.save(&note).await.unwrap();
    allow(&state, ids[1], id, CHANGE, false).await;
    assert!(eb.pull().await.unwrap().blocked);
    assert!(b.database().get::<Note>(id).await.unwrap().is_some());
    assert!(b.save(&note).await.is_err());
    assert_eq!(eb.flush().await.unwrap().accepted_operations, 0);
}

#[tokio::test]
async fn deletion_is_distinct_from_revocation_and_discard_waits_for_unknown_operations() {
    let (state, _, router) = setup().await;
    let id = create(&router).await;
    let a = client(database(SchemaRole::Client).await, "alice").await;
    let ea = a.engine(transport(&router, "alice"));
    ea.pull().await.unwrap();
    let trusted = che_sync::SyncServer::open_model_logs(
        state.database().clone(),
        registry(SchemaRole::Server),
        "worker",
        Arc::new(che_sync::AllowAll),
    )
    .await
    .unwrap();
    trusted.delete::<Note>(id).await.unwrap();
    ea.pull().await.unwrap();
    let status = a.object_status().await.unwrap();
    assert_eq!(status[0].status, "deleted");
    assert!(!status[0].has_draft);
    assert!(a.database().get::<Note>(id).await.unwrap().is_none());
    a.discard_retained_draft(Note::SYNC_NAME, id).await.unwrap();
    assert!(
        a.retained_draft(Note::SYNC_NAME, id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn rolled_back_acl_changes_do_not_create_delivery_and_queryset_changes_revoke() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    let b = transport(&router, "bob");
    let empty = b.object_changes(Note::SYNC_NAME, None, 100).await.unwrap();
    let bob = ids[1];
    let result: Result<(), che_rest::AppError> = state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                Access::<Note>::grant(tx, Subject::User(bob), VIEW, id).await?;
                Err(che_rest::AppError::BadRequest("rollback".into()))
            })
        })
        .await;
    assert!(result.is_err());
    assert!(
        b.object_changes(Note::SYNC_NAME, Some(empty.cursor.clone()), 100)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    allow(&state, bob, id, VIEW, true).await;
    let granted = b
        .object_changes(Note::SYNC_NAME, Some(empty.cursor), 100)
        .await
        .unwrap();
    assert_eq!(granted.events.len(), 1);
    let (status, _) = call(
        &router,
        "alice",
        "PATCH",
        &format!("/api/notes/{id}/"),
        json!({"title":"outside"}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let revoked = b
        .object_changes(Note::SYNC_NAME, Some(granted.cursor), 100)
        .await
        .unwrap();
    assert!(matches!(
        revoked.events[0].change,
        ObjectChange::Revoked { .. }
    ));
}

#[tokio::test]
async fn upgrade_rejects_pending_v2_without_changing_ownership_or_payload() {
    let (state, _, router) = setup().await;
    let id = create(&router).await;
    let db = database(SchemaRole::Client).await;
    let old = ModelLogClient::open(db.clone(), registry(SchemaRole::Client))
        .await
        .unwrap();
    old.save(&Note {
        id: Uuid::now_v7(),
        title: "pending".into(),
        done: false,
    })
    .await
    .unwrap();
    // Protocol-2 prepares a durable operation before this adapter refuses delivery.
    let model = che_sync::SyncServer::open_model_logs(
        state.database().clone(),
        registry(SchemaRole::Server),
        "worker",
        Arc::new(che_sync::AllowAll),
    )
    .await
    .unwrap();
    old.apply(model.changes::<Note>(0, 100).await.unwrap())
        .await
        .unwrap();
    assert!(
        old.engine(transport(&router, "alice"))
            .flush()
            .await
            .is_err()
    );
    let before = db
        .query_sql("SELECT payload FROM _sync_outbox", vec![])
        .await
        .unwrap()[0]
        .get::<String>(0)
        .unwrap();
    assert!(
        ModelLogClient::upgrade_to_objects(
            db.clone(),
            registry(SchemaRole::Client),
            "server-one",
            "alice"
        )
        .await
        .is_err()
    );
    let after = db
        .query_sql("SELECT payload FROM _sync_outbox", vec![])
        .await
        .unwrap()[0]
        .get::<String>(0)
        .unwrap();
    assert_eq!(before, after);
    assert!(
        db.query_sql(
            "SELECT key FROM _sync_meta WHERE key='object_owner'",
            vec![]
        )
        .await
        .unwrap()
        .is_empty()
    );
    assert!(old.database().get::<Note>(id).await.unwrap().is_some());
}

#[tokio::test]
async fn native_creation_refreshes_rights_and_acknowledged_delete_cannot_be_recreated() {
    let (_, _, router) = setup().await;
    let a = client(database(SchemaRole::Client).await, "alice").await;
    let engine = a.engine(transport(&router, "alice"));
    engine.pull().await.unwrap();
    let note = Note {
        id: Uuid::now_v7(),
        title: "offline create".into(),
        done: false,
    };
    a.save(&note).await.unwrap();
    let report = engine.flush().await.unwrap();
    assert_eq!(report.accepted_operations, 1);
    assert!(report.has_more);
    assert!(a.database().get::<Note>(note.id).await.unwrap().is_some());
    // The reply proves reading; only a delivery page proves current write rights.
    assert!(!a.object_status().await.unwrap()[0].rights.change);
    engine.pull().await.unwrap();
    assert!(a.object_status().await.unwrap()[0].rights.change);
    a.delete::<Note>(note.id).await.unwrap();
    assert_eq!(engine.flush().await.unwrap().accepted_operations, 1);
    assert_eq!(a.object_status().await.unwrap()[0].status, "deleted");
    assert!(a.save(&note).await.is_err());
}

#[tokio::test]
async fn new_read_grant_does_not_disclose_historical_receipt() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    allow(&state, ids[1], id, CHANGE, true).await;
    let headers = vec![
        ("x-sync-protocol", "3".into()),
        ("x-sync-replica", Uuid::now_v7().to_string()),
        ("idempotency-key", Uuid::now_v7().to_string()),
        ("if-match", "\"1\"".into()),
        ("x-sync-version", "1".into()),
        ("x-sync-entity", id.to_string()),
    ];
    let path = format!("/api/notes/{id}/");
    let (status, first) = call(
        &router,
        "bob",
        "PATCH",
        &path,
        json!({"title":"blind"}),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["kind"], "acknowledged", "{first}");
    let (status, body) = call(&router, "alice", "PATCH", &path, json!({"done":true}), &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    allow(&state, ids[1], id, VIEW, true).await;
    let (_, replay) = call(
        &router,
        "bob",
        "PATCH",
        &path,
        json!({"title":"blind"}),
        &headers,
    )
    .await;
    assert_eq!(replay["kind"], "acknowledged", "{replay}");
    assert!(!replay.to_string().contains("done"));
    let (_, current) = call(&router, "bob", "GET", &path, Value::Null, &[]).await;
    assert_eq!(current["done"], true);
}

#[tokio::test]
async fn delete_only_draft_can_resume_after_regrant() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    for p in [VIEW, DELETE] {
        allow(&state, ids[1], id, p, true).await;
    }
    let b = client(database(SchemaRole::Client).await, "bob").await;
    let eb = b.engine(transport(&router, "bob"));
    eb.pull().await.unwrap();
    b.delete::<Note>(id).await.unwrap();
    allow(&state, ids[1], id, VIEW, false).await;
    eb.pull().await.unwrap();
    allow(&state, ids[1], id, VIEW, true).await;
    eb.pull().await.unwrap();
    let status = b.object_status().await.unwrap().remove(0);
    assert_eq!(status.status, "review");
    assert!(status.rights.delete && !status.rights.change);
    b.resume_object(Note::SYNC_NAME, id).await.unwrap();
    assert_eq!(eb.flush().await.unwrap().accepted_operations, 1);
    let (status, _) = call(
        &router,
        "alice",
        "GET",
        &format!("/api/notes/{id}/"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pending_operation_does_not_block_later_revocations() {
    let (state, ids, router) = setup().await;
    let first = create(&router).await;
    let second = create(&router).await;
    for id in [first, second] {
        for p in [VIEW, CHANGE] {
            allow(&state, ids[1], id, p, true).await;
        }
    }
    let db = database(SchemaRole::Client).await;
    let b = client(db.clone(), "bob").await;
    let t = transport(&router, "bob");
    let eb = b.engine(t.clone());
    eb.pull().await.unwrap();
    let mut note = b.database().get::<Note>(first).await.unwrap().unwrap();
    note.title = "pending".into();
    b.save(&note).await.unwrap();
    t.lose.store(true, Ordering::SeqCst);
    assert!(eb.flush().await.is_err());
    allow(&state, ids[1], second, VIEW, false).await;
    let eb = b.engine(t.clone()).with_page_size(1).unwrap();
    let first_page = eb.pull().await.unwrap();
    assert!(first_page.has_more);
    assert!(
        b.object_status()
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == first && s.submitted)
    );
    assert!(b.database().get::<Note>(second).await.unwrap().is_some());
    // Reopen with the first object's canonical state deferred durably.
    drop(eb);
    drop(b);
    let b = client(db, "bob").await;
    let eb = b.engine(t.clone()).with_page_size(1).unwrap();
    eb.pull().await.unwrap();
    assert!(b.database().get::<Note>(second).await.unwrap().is_none());
    assert!(
        b.object_status()
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == first && s.submitted)
    );
    assert_eq!(eb.flush().await.unwrap().accepted_operations, 1);
    assert!(!b.object_status().await.unwrap().iter().any(|s| s.submitted));
    assert_eq!(
        b.database()
            .get::<Note>(first)
            .await
            .unwrap()
            .unwrap()
            .title,
        "pending"
    );
    let sent = t.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
}

#[tokio::test]
async fn stale_receipt_ack_refreshes_current_projection_after_pending_pull() {
    let (_, _, router) = setup().await;
    let id = create(&router).await;
    let db = database(SchemaRole::Client).await;
    let a = client(db.clone(), "alice").await;
    let t = transport(&router, "alice");
    let ea = a.engine(t.clone());
    ea.pull().await.unwrap();
    let mut note = a.database().get::<Note>(id).await.unwrap().unwrap();
    note.title = "sent title".into();
    a.save(&note).await.unwrap();
    t.lose.store(true, Ordering::SeqCst);
    assert!(ea.flush().await.is_err());
    let (status, body) = call(
        &router,
        "alice",
        "PATCH",
        &format!("/api/notes/{id}/"),
        json!({"done":true}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    ea.pull().await.unwrap();
    assert!(!a.database().get::<Note>(id).await.unwrap().unwrap().done);
    drop(ea);
    drop(a);
    let a = client(db, "alice").await;
    let ea = a.engine(t.clone());
    assert_eq!(ea.flush().await.unwrap().accepted_operations, 1);
    assert!(!a.object_status().await.unwrap()[0].submitted);
    ea.pull().await.unwrap();
    let current = a.database().get::<Note>(id).await.unwrap().unwrap();
    assert!(current.done);
    assert_eq!(current.title, "sent title");
    assert!(!a.object_status().await.unwrap()[0].has_draft);
    let sent = t.sent.lock().unwrap();
    assert_eq!(sent[0], sent[1]);
}

#[tokio::test]
async fn deferred_state_cannot_restore_revoked_access_after_settlement() {
    let (state, ids, router) = setup().await;
    let id = create(&router).await;
    let a = client(database(SchemaRole::Client).await, "alice").await;
    let t = transport(&router, "alice");
    let ea = a.engine(t.clone());
    ea.pull().await.unwrap();
    let mut note = a.database().get::<Note>(id).await.unwrap().unwrap();
    note.title = "pending".into();
    a.save(&note).await.unwrap();
    t.lose.store(true, Ordering::SeqCst);
    assert!(ea.flush().await.is_err());
    ea.pull().await.unwrap();
    allow(&state, ids[0], id, VIEW, false).await;
    ea.pull().await.unwrap();
    assert!(a.database().get::<Note>(id).await.unwrap().is_none());
    assert_eq!(ea.flush().await.unwrap().accepted_operations, 1);
    ea.pull().await.unwrap();
    assert!(a.database().get::<Note>(id).await.unwrap().is_none());
    assert!(a.object_status().await.unwrap()[0].has_draft);
}

#[tokio::test]
async fn opaque_accepted_intent_rebases_after_restart_and_preserves_only_later_edits() {
    // No later edit, an edit to a different field, and a real same-field conflict.
    for (creating, later) in (0..2).flat_map(|mode| (0..3).map(move |later| (mode == 1, later))) {
        let (_, _, router) = setup().await;
        let id = if creating {
            Uuid::now_v7()
        } else {
            create(&router).await
        };
        let db = database(SchemaRole::Client).await;
        let a = client(db.clone(), "alice").await;
        let t = transport(&router, "alice");
        let ea = a.engine(t.clone());
        ea.pull().await.unwrap();
        let mut note = if creating {
            Note {
                id,
                title: "new note".into(),
                done: false,
            }
        } else {
            a.database().get::<Note>(id).await.unwrap().unwrap()
        };
        note.title = "accepted edit".into();
        a.save(&note).await.unwrap();
        t.lose.store(true, Ordering::SeqCst);
        assert!(ea.flush().await.is_err());
        if later > 0 {
            if later == 1 {
                note.done = true;
            } else {
                note.title = "subsequent draft".into();
            }
            a.save(&note).await.unwrap();
        }
        let (status, body) = call(
            &router,
            "alice",
            "PATCH",
            &format!("/api/notes/{id}/"),
            json!({"title":"later server edit"}),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(ea.flush().await.unwrap().accepted_operations, 1);
        // Both the accepted merge base and pinned cursor survive a restart.
        drop(ea);
        drop(a);
        let a = client(db, "alice").await;
        let ea = a.engine(t.clone());
        ea.pull().await.unwrap();
        let current = a.database().get::<Note>(id).await.unwrap().unwrap();
        let status = a.object_status().await.unwrap().remove(0);
        if later == 2 {
            assert_eq!(a.conflicts().await.unwrap().len(), 1);
            assert_eq!(current.title, "subsequent draft");
        } else {
            assert!(a.conflicts().await.unwrap().is_empty());
            assert_eq!(current.title, "later server edit");
            assert_eq!(current.done, later == 1);
            assert_eq!(status.has_draft, later == 1);
            if later == 0 {
                assert_eq!(status.status, "active");
                let mut next = current;
                next.done = true;
                a.save(&next).await.unwrap();
            } else {
                assert_eq!(status.status, "review");
                a.resume_object(Note::SYNC_NAME, id).await.unwrap();
                assert_eq!(ea.flush().await.unwrap().accepted_operations, 1);
            }
        }
        let sent = t.sent.lock().unwrap();
        assert_eq!(sent[0], sent[1]);
    }
}

#[tokio::test]
async fn opaque_settlement_rewinds_delivery_without_unpinning_server_history() {
    let (state, ids, router) = setup().await;
    let first = create(&router).await;
    let second = create(&router).await;
    let db = database(SchemaRole::Client).await;
    let a = client(db.clone(), "alice").await;
    let t = transport(&router, "alice");
    let ea = a.engine(t.clone());
    ea.pull().await.unwrap();
    let (_, _, replacement) = setup().await;
    assert!(
        a.engine(transport(&replacement, "alice"))
            .pull()
            .await
            .is_err()
    );
    let mut note = a.database().get::<Note>(first).await.unwrap().unwrap();
    note.title = "sent".into();
    a.save(&note).await.unwrap();
    t.lose.store(true, Ordering::SeqCst);
    assert!(ea.flush().await.is_err());
    allow(&state, ids[0], first, VIEW, false).await;
    assert_eq!(ea.flush().await.unwrap().accepted_operations, 1);
    drop(ea);
    drop(a);
    let a = client(db, "alice").await;
    assert!(
        a.engine(transport(&replacement, "alice"))
            .pull()
            .await
            .is_err()
    );
    // A replay against the original feed still works and retains other objects.
    a.engine(t).pull().await.unwrap();
    assert!(a.database().get::<Note>(first).await.unwrap().is_none());
    assert!(a.database().get::<Note>(second).await.unwrap().is_some());
}
