//! Portable device registration, durable outbox and FCM delivery.
//! Applications supply committed domain events through `PushSource`.
pub mod delivery;
pub mod fcm;
pub mod migrations;
pub mod models;
pub mod views;
use crate::{AppModule, ModuleContext};
use che_orm::{OrmError, Transaction};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::Arc,
};
use time::OffsetDateTime;
pub type PushFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, OrmError>> + Send + 'a>>;
#[derive(Clone, Default)]
pub struct PushConfig {
    pub enabled: bool,
    pub package_name: Option<String>,
    pub project_id: Option<String>,
}
/// Application-owned event. Keys must remain stable across retries/restarts.
pub struct PushEvent {
    pub key: String,
    pub users: BTreeSet<i64>,
    pub target_type: String,
    pub target_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub data: BTreeMap<String, String>,
    pub created_at: OffsetDateTime,
}
/// Cursor writes in capture and queue insertion commit in the same transaction.
/// Domain reads/writes must retain the supplied configured transaction.
pub trait PushSource: Send + Sync + 'static {
    /// Initialize missing checkpoints at current heads to exclude old events.
    fn initialize<'a>(&'a self, tx: &'a Transaction) -> PushFuture<'a, ()>;
    fn capture<'a>(&'a self, tx: &'a Transaction) -> PushFuture<'a, Vec<PushEvent>>;
    /// Recheck current recipients/deletions before claiming a delivery.
    fn relevant<'a>(
        &'a self,
        tx: &'a Transaction,
        job: &'a models::PushDelivery,
    ) -> PushFuture<'a, bool>;
}
#[derive(Clone)]
pub struct PushRuntime {
    pub config: PushConfig,
    pub source: Arc<dyn PushSource>,
}
pub fn module(config: PushConfig, source: Arc<dyn PushSource>) -> PushModule {
    PushModule(PushRuntime { config, source })
}
pub struct PushModule(PushRuntime);
impl AppModule for PushModule {
    fn name(&self) -> &'static str {
        "notifications"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new()
            .model::<models::PushDevice>()
            .model::<models::PushCursor>()
            .model::<models::PushDelivery>()
    }
    fn migrations(&self) -> Vec<che_orm::Migration> {
        migrations::all()
    }
    fn init(&self, context: &mut ModuleContext) {
        context.route(views::routes(self.0.clone()));
    }
}
/// Explicit startup validates configuration and schema, never creates tables.
pub async fn start(
    db: che_orm::Database,
    config: PushConfig,
    source: Arc<dyn PushSource>,
) -> Result<Option<tokio::task::JoinHandle<()>>, Box<dyn std::error::Error>> {
    if !config.enabled {
        return Ok(None);
    }
    db.query::<models::PushCursor>()
        .first(&db)
        .await
        .map_err(|_| "Apply notifications migrations before enabling push")?;
    let sender = fcm::FcmSender::from_env(&config)?;
    eprintln!("Push enabled for Firebase project {}", sender.project);
    Ok(Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if delivery::capture(&db, source.clone()).await.is_err() {
                eprintln!("Push scan failed; cursors and deliveries retained");
                continue;
            }
            if delivery::drain(&db, &sender, source.clone()).await.is_err() {
                eprintln!("Push delivery failed; queue retained");
            }
        }
    })))
}

#[cfg(test)]
mod tests;
