use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Default)]
pub struct AppChannels {
    senders: Arc<Mutex<std::collections::HashMap<String, broadcast::Sender<Value>>>>,
}

pub struct AppChannelReceiver {
    name: String,
    receiver: broadcast::Receiver<Value>,
    channels: AppChannels,
}

impl AppChannels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, name: impl Into<String>, payload: Value) {
        let name = name.into();
        let sender = self
            .senders
            .lock()
            .expect("application channel registry lock is poisoned")
            .get(&name)
            .cloned();
        if let Some(sender) = sender {
            let _ = sender.send(payload);
        }
    }

    pub fn subscribe(&self, name: impl Into<String>) -> AppChannelReceiver {
        let name = name.into();
        let mut senders = self
            .senders
            .lock()
            .expect("application channel registry lock is poisoned");
        senders
            .entry(name.clone())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        let receiver = senders
            .get(&name)
            .expect("application channel sender was just inserted")
            .subscribe();
        AppChannelReceiver {
            name,
            receiver,
            channels: self.clone(),
        }
    }

    fn cleanup(&self, name: &str) {
        let mut senders = self
            .senders
            .lock()
            .expect("application channel registry lock is poisoned");
        if senders
            .get(name)
            // Drop runs before the receiver field is destroyed, so the current
            // receiver is still included in this count.
            .is_some_and(|sender| sender.receiver_count() <= 1)
        {
            senders.remove(name);
        }
    }
}

impl AppChannelReceiver {
    pub async fn recv(&mut self) -> Result<Value, broadcast::error::RecvError> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Result<Value, broadcast::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for AppChannelReceiver {
    fn drop(&mut self) {
        self.channels.cleanup(&self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_internal_events() {
        let channels = AppChannels::new();
        let mut receiver = channels.subscribe("chat.message.created");
        channels.publish("chat.message.created", serde_json::json!({ "id": 42 }));

        assert_eq!(
            receiver.recv().await.unwrap(),
            serde_json::json!({ "id": 42 })
        );
    }

    #[test]
    fn drops_empty_channel_after_last_receiver() {
        let channels = AppChannels::new();
        let receiver = channels.subscribe("temporary");
        assert_eq!(channels.senders.lock().unwrap().len(), 1);
        drop(receiver);
        assert!(channels.senders.lock().unwrap().is_empty());
    }
}
