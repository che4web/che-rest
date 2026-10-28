use super::{
    PushEvent, PushFuture, PushSource, delivery,
    fcm::{PushSender, SendResult, message},
    models::{PushCursor, PushDelivery, PushDevice},
    views::{RegisterDevice, register_device},
};
use che_orm::{Database, MigrationGraph, Transaction, Uuid};
use std::{collections::BTreeSet, sync::Arc};
struct InvoiceSource;
impl PushSource for InvoiceSource {
    fn initialize<'a>(&'a self, tx: &'a Transaction) -> PushFuture<'a, ()> {
        Box::pin(async move {
            if tx
                .query::<PushCursor>()
                .filter(PushCursor::STREAM.eq("invoices"))
                .first()
                .await?
                .is_none()
            {
                tx.create::<PushCursor>()
                    .set(PushCursor::STREAM, "invoices")
                    .set(PushCursor::POSITION, 0_i64)
                    .set(PushCursor::HISTORY, "example")
                    .execute()
                    .await?;
            }
            Ok(())
        })
    }
    fn capture<'a>(&'a self, tx: &'a Transaction) -> PushFuture<'a, Vec<PushEvent>> {
        Box::pin(async move {
            let Some(mut cursor) = tx
                .query::<PushCursor>()
                .filter(PushCursor::STREAM.eq("invoices"))
                .first()
                .await?
            else {
                return Ok(vec![]);
            };
            if cursor.position > 0 {
                return Ok(vec![]);
            }
            cursor.position = 1;
            tx.save(&cursor).await?;
            Ok(vec![PushEvent {
                key: "invoice-paid:1".into(),
                users: BTreeSet::from([7]),
                target_type: "invoice".into(),
                target_id: "INV-42".into(),
                kind: "invoice_paid".into(),
                title: "Billing".into(),
                body: "Invoice paid".into(),
                data: [("screen".into(), "invoices".into())].into(),
                created_at: time::OffsetDateTime::now_utc(),
            }])
        })
    }
    fn relevant<'a>(&'a self, _: &'a Transaction, job: &'a PushDelivery) -> PushFuture<'a, bool> {
        Box::pin(async move { Ok(job.target_type == "invoice" && job.user_id == 7) })
    }
}
struct Accepted;
impl PushSender for Accepted {
    fn send<'a>(
        &'a self,
        _: &'a PushDevice,
        _: &'a PushDelivery,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SendResult> + Send + 'a>> {
        Box::pin(async { SendResult::Accepted })
    }
}
#[tokio::test]
async fn standalone_module_migrates_and_delivers_non_sync_non_uuid_domain_events() {
    let db = Database::connect_in_memory().await.unwrap();
    db.migrate(MigrationGraph::new(super::migrations::all()).unwrap())
        .await
        .unwrap();
    db.migrate(MigrationGraph::new(super::migrations::all()).unwrap())
        .await
        .unwrap();
    let source = Arc::new(InvoiceSource);
    let id = Uuid::now_v7();
    register_device(
        &db,
        7,
        RegisterDevice {
            id,
            token: "fcm-example-token".into(),
            server_key: "b".repeat(64),
        },
        source.clone(),
    )
    .await
    .unwrap();
    delivery::capture(&db, source.clone()).await.unwrap();
    delivery::capture(&db, source.clone()).await.unwrap();
    let jobs = db.query::<PushDelivery>().all(&db).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].target_id, "INV-42");
    let device = db.get::<PushDevice>(id).await.unwrap().unwrap();
    let payload = message(&device, &jobs[0], Some("org.example.billing"));
    assert_eq!(payload["message"]["data"]["target_type"], "invoice");
    assert!(payload["message"]["data"]["task_id"].is_null());
    assert_eq!(
        payload["message"]["android"]["restricted_package_name"],
        "org.example.billing"
    );
    delivery::drain(&db, &Accepted, source.clone())
        .await
        .unwrap();
    delivery::drain(&db, &Accepted, source).await.unwrap();
    assert_eq!(
        db.query::<PushDelivery>()
            .first(&db)
            .await
            .unwrap()
            .unwrap()
            .attempts,
        1
    );
}
#[test]
fn compiled_schema_matches_library_models_without_project_dependencies() {
    let module = super::module(Default::default(), Arc::new(InvoiceSource));
    let expected = crate::InstalledApps::new()
        .add(module)
        .migration_state()
        .unwrap();
    let state = MigrationGraph::new(super::migrations::all())
        .unwrap()
        .replay(Default::default())
        .unwrap();
    assert!(state.diff(&expected).unwrap().is_empty());
}

struct Oversized;
impl PushSource for Oversized {
    fn initialize<'a>(&'a self, tx: &'a Transaction) -> PushFuture<'a, ()> {
        InvoiceSource.initialize(tx)
    }
    fn capture<'a>(&'a self, tx: &'a Transaction) -> PushFuture<'a, Vec<PushEvent>> {
        Box::pin(async move {
            let mut events = InvoiceSource.capture(tx).await?;
            if let Some(event) = events.first_mut() {
                event.body = "x".repeat(4000);
            }
            Ok(events)
        })
    }
    fn relevant<'a>(&'a self, tx: &'a Transaction, job: &'a PushDelivery) -> PushFuture<'a, bool> {
        InvoiceSource.relevant(tx, job)
    }
}
#[tokio::test]
async fn invalid_event_rolls_back_source_checkpoint_and_queue_together() {
    let db = Database::connect_in_memory().await.unwrap();
    db.migrate(MigrationGraph::new(super::migrations::all()).unwrap())
        .await
        .unwrap();
    let source = Arc::new(Oversized);
    register_device(
        &db,
        7,
        RegisterDevice {
            id: Uuid::now_v7(),
            token: "example-device-token".into(),
            server_key: "b".repeat(64),
        },
        source.clone(),
    )
    .await
    .unwrap();
    assert!(delivery::capture(&db, source).await.is_err());
    assert_eq!(
        db.query::<PushCursor>()
            .first(&db)
            .await
            .unwrap()
            .unwrap()
            .position,
        0
    );
    assert!(
        db.query::<PushDelivery>()
            .all(&db)
            .await
            .unwrap()
            .is_empty()
    );
}
