use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Clone, Default)]
pub struct AppChannels {
    inner: Arc<Mutex<std::collections::HashMap<String, broadcast::Sender<Value>>>>,
}

impl AppChannels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, channel: impl Into<String>, payload: Value) {
        let mut channels = self.inner.lock().expect("app channel lock is poisoned");
        let sender = channels
            .entry(channel.into())
            .or_insert_with(|| broadcast::channel(64).0);
        let _ = sender.send(payload);
    }

    pub fn subscribe(&self, channel: &str) -> broadcast::Receiver<Value> {
        let mut channels = self.inner.lock().expect("app channel lock is poisoned");
        channels
            .entry(channel.to_owned())
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe()
    }
}
