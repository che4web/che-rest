use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Extension, Router,
    extract::{
        WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};

use crate::{
    AppError, AppModule, AppResult, ModuleContext, auth::CurrentUser, commands::Commands,
    state::AppState,
};

const CHANNEL_CAPACITY: usize = 64;
const OUTBOUND_CAPACITY: usize = 64;
const MAX_SUBSCRIPTIONS: usize = 32;
const MAX_CHANNEL_LENGTH: usize = 128;
const USER_CHANNEL_PREFIX: &str = "user:";

#[derive(Debug, Clone, Default)]
pub struct Channels {
    inner: Arc<Mutex<HashMap<String, broadcast::Sender<Value>>>>,
}

impl Channels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, channel: impl AsRef<str>, payload: Value) {
        let sender = self
            .inner
            .lock()
            .expect("channel registry lock is poisoned")
            .get(channel.as_ref())
            .cloned();
        if let Some(sender) = sender {
            let _ = sender.send(payload);
        }
    }

    pub fn publish_user(&self, user_id: i64, payload: Value) {
        self.publish(user_channel(user_id), payload);
    }

    fn subscribe(&self, channel: &str) -> broadcast::Receiver<Value> {
        let mut channels = self
            .inner
            .lock()
            .expect("channel registry lock is poisoned");
        channels
            .entry(channel.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .subscribe()
    }

    fn cleanup(&self, channel: &str) {
        self.inner
            .lock()
            .expect("channel registry lock is poisoned")
            .retain(|name, sender| name != channel || sender.receiver_count() > 0);
    }
}

#[derive(Clone, Copy)]
pub struct ChannelModule;

pub fn module() -> ChannelModule {
    ChannelModule
}

impl AppModule for ChannelModule {
    fn name(&self) -> &'static str {
        "channels"
    }

    fn init(&self, ctx: &mut ModuleContext) {
        ctx.enable_auth();
        ctx.route(Router::new().route("/ws/", get(websocket)));
    }
}

async fn websocket(
    user: Option<Extension<CurrentUser>>,
    Extension(state): Extension<AppState>,
    Extension(commands): Extension<Commands>,
    upgrade: WebSocketUpgrade,
) -> AppResult<Response> {
    let user = user.ok_or_else(|| {
        AppError::Unauthorized("authentication credentials were not provided".to_string())
    })?;
    Ok(upgrade
        .on_upgrade(move |socket| handle_socket(socket, user.0, state, commands))
        .into_response())
}

async fn handle_socket(socket: WebSocket, user: CurrentUser, state: AppState, commands: Commands) {
    let channels = state.channels().clone();
    let (mut sender, mut receiver) = socket.split();
    let (outbound, mut outbound_receiver) = mpsc::channel(OUTBOUND_CAPACITY);
    let send_task = tokio::spawn(async move {
        while let Some(message) = outbound_receiver.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    let mut subscriptions = HashMap::new();
    add_subscription(
        &mut subscriptions,
        user_channel(user.id),
        channels.clone(),
        outbound.clone(),
    );

    while let Some(Ok(message)) = receiver.next().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            let _ = send_json(
                &outbound,
                json!({ "type": "error", "code": "invalid_message", "detail": "only text JSON messages are supported" }),
            )
            .await;
            continue;
        };

        let command = match serde_json::from_str::<ClientCommand>(&text) {
            Ok(command) => command,
            Err(_) => {
                let _ = send_json(
                    &outbound,
                    json!({ "type": "error", "code": "invalid_command", "detail": "expected JSON with action, channel, event, and payload" }),
                )
                .await;
                continue;
            }
        };

        if let Some(channel) = command.channel.as_deref()
            && !valid_channel(channel)
        {
            let _ = send_json(
                &outbound,
                json!({ "type": "error", "code": "invalid_channel", "detail": "channel name contains unsupported characters" }),
            )
            .await;
            continue;
        }
        if command
            .channel
            .as_deref()
            .is_some_and(|channel| channel.starts_with(USER_CHANNEL_PREFIX))
        {
            let _ = send_json(
                &outbound,
                json!({ "type": "error", "code": "reserved_channel", "detail": "user channels are managed by the server" }),
            )
            .await;
            continue;
        }

        match command.action.as_str() {
            "publish" => {
                let Some(event) = command.event.as_deref() else {
                    let _ = send_json(
                        &outbound,
                        json!({ "type": "error", "code": "missing_event", "detail": "publish requires an event name" }),
                    )
                    .await;
                    continue;
                };
                if let Err(error) = commands
                    .dispatch(
                        &state,
                        event,
                        user.clone(),
                        command.payload.unwrap_or(Value::Null),
                    )
                    .await
                {
                    let _ = send_json(
                        &outbound,
                        json!({ "type": "error", "code": "command_error", "detail": error.to_string() }),
                    )
                    .await;
                } else {
                    let _ =
                        send_json(&outbound, json!({ "type": "published", "event": event })).await;
                }
            }
            "subscribe"
                if command
                    .channel
                    .as_ref()
                    .is_some_and(|channel| subscriptions.contains_key(channel)) =>
            {
                let _ = send_json(
                    &outbound,
                    json!({ "type": "subscribed", "channel": command.channel }),
                )
                .await;
            }
            "subscribe" if subscriptions.len() >= MAX_SUBSCRIPTIONS => {
                let _ = send_json(
                    &outbound,
                    json!({ "type": "error", "code": "subscription_limit", "detail": "too many channel subscriptions" }),
                )
                .await;
            }
            "subscribe" => {
                let Some(channel) = command.channel.clone() else {
                    let _ = send_json(
                        &outbound,
                        json!({ "type": "error", "code": "missing_channel", "detail": "subscribe requires a channel" }),
                    )
                    .await;
                    continue;
                };
                add_subscription(
                    &mut subscriptions,
                    channel.clone(),
                    channels.clone(),
                    outbound.clone(),
                );
                let _ = send_json(
                    &outbound,
                    json!({ "type": "subscribed", "channel": channel }),
                )
                .await;
            }
            "unsubscribe" => {
                let Some(channel) = command.channel.clone() else {
                    let _ = send_json(
                        &outbound,
                        json!({ "type": "error", "code": "missing_channel", "detail": "unsubscribe requires a channel" }),
                    )
                    .await;
                    continue;
                };
                if let Some(task) = subscriptions.remove(&channel) {
                    task.abort();
                    channels.cleanup(&channel);
                }
                let _ = send_json(
                    &outbound,
                    json!({ "type": "unsubscribed", "channel": channel }),
                )
                .await;
            }
            _ => {
                let _ = send_json(
                    &outbound,
                    json!({ "type": "error", "code": "unknown_action", "detail": "action must be subscribe, unsubscribe, or publish" }),
                )
                .await;
            }
        }
    }

    for (channel, task) in subscriptions {
        task.abort();
        channels.cleanup(&channel);
    }
    send_task.abort();
}

fn add_subscription(
    subscriptions: &mut HashMap<String, JoinHandle<()>>,
    channel: String,
    channels: Channels,
    outbound: mpsc::Sender<Message>,
) {
    let receiver = channels.subscribe(&channel);
    let task = tokio::spawn(forward_messages(
        channel.clone(),
        receiver,
        channels,
        outbound,
    ));
    subscriptions.insert(channel, task);
}

async fn forward_messages(
    channel: String,
    mut receiver: broadcast::Receiver<Value>,
    channels: Channels,
    outbound: mpsc::Sender<Message>,
) {
    loop {
        match receiver.recv().await {
            Ok(payload) => {
                if send_json(
                    &outbound,
                    json!({ "type": "message", "channel": channel, "payload": payload }),
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(count)) => {
                if send_json(
                    &outbound,
                    json!({ "type": "error", "code": "lagged", "detail": format!("{count} messages were dropped") }),
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    channels.cleanup(&channel);
}

async fn send_json(outbound: &mpsc::Sender<Message>, payload: Value) -> Result<(), ()> {
    outbound
        .send(Message::Text(payload.to_string().into()))
        .await
        .map_err(|_| ())
}

fn user_channel(user_id: i64) -> String {
    format!("{USER_CHANNEL_PREFIX}{user_id}")
}

#[derive(Deserialize)]
struct ClientCommand {
    action: String,
    channel: Option<String>,
    event: Option<String>,
    payload: Option<Value>,
}

fn valid_channel(channel: &str) -> bool {
    !channel.is_empty()
        && channel.len() <= MAX_CHANNEL_LENGTH
        && channel
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_channel_names() {
        assert!(valid_channel("orders:42"));
        assert!(valid_channel("project-a.events_1"));
        assert!(!valid_channel(""));
        assert!(!valid_channel("contains space"));
    }

    #[tokio::test]
    async fn delivers_messages_to_subscribers() {
        let channels = Channels::new();
        let mut receiver = channels.subscribe("updates");
        channels.publish("updates", json!({ "status": "ready" }));
        assert_eq!(receiver.recv().await.unwrap(), json!({ "status": "ready" }));
    }

    #[tokio::test]
    async fn delivers_messages_to_user_channel() {
        let channels = Channels::new();
        let mut receiver = channels.subscribe(&user_channel(42));
        channels.publish_user(42, json!({ "kind": "notification" }));
        assert_eq!(
            receiver.recv().await.unwrap(),
            json!({ "kind": "notification" })
        );
    }
}
