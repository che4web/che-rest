use std::collections::HashMap;

use axum::{
    Extension, Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    AppModule, AppState, ModuleContext,
    auth::CurrentUser,
    signals::{SignalError, error_event, signal_event, valid_signal_name},
};

pub type Channels = crate::signals::SignalBus;
const MAX_SUBSCRIPTIONS_PER_SOCKET: usize = 32;

pub fn module() -> ChannelsModule {
    ChannelsModule
}

#[derive(Clone, Copy)]
pub struct ChannelsModule;

impl AppModule for ChannelsModule {
    fn name(&self) -> &'static str {
        "channels"
    }

    fn schema(&self) -> che_orm2::SchemaSet {
        che_orm2::SchemaSet::new()
    }

    fn init(&self, context: &mut ModuleContext) {
        context.route(Router::new().route("/ws/", get(websocket)));
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ClientFrame {
    Subscribe {
        signal: Option<String>,
        channel: Option<String>,
    },
    Unsubscribe {
        signal: Option<String>,
        channel: Option<String>,
    },
}

async fn websocket(
    Extension(state): Extension<AppState>,
    user: Option<Extension<CurrentUser>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(state, user.map(|user| user.0), socket))
}

async fn handle_socket(state: AppState, user: Option<CurrentUser>, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let (events_tx, mut events_rx) = mpsc::channel::<Value>(64);
    let mut subscriptions: HashMap<String, JoinHandle<()>> = HashMap::new();

    loop {
        tokio::select! {
            maybe_event = events_rx.recv() => {
                let Some(event) = maybe_event else { break; };
                if sender.send(Message::Text(event.to_string().into())).await.is_err() {
                    break;
                }
            }
            maybe_message = receiver.next() => {
                let Some(Ok(message)) = maybe_message else { break; };
                match message {
                    Message::Text(text) => handle_text(&state, user.as_ref(), &events_tx, &mut subscriptions, &text).await,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    for task in subscriptions.into_values() {
        task.abort();
    }
}

async fn handle_text(
    state: &AppState,
    user: Option<&CurrentUser>,
    events_tx: &mpsc::Sender<Value>,
    subscriptions: &mut HashMap<String, JoinHandle<()>>,
    text: &str,
) {
    let frame = match serde_json::from_str::<ClientFrame>(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(
                events_tx,
                "invalid_frame",
                &format!("invalid JSON frame: {error}"),
            )
            .await;
            return;
        }
    };

    match frame {
        ClientFrame::Subscribe { signal, channel } => {
            let Some(signal) = signal.or(channel) else {
                send_error(events_tx, "invalid_frame", "subscribe requires signal").await;
                return;
            };
            subscribe(state, user, events_tx, subscriptions, signal).await;
        }
        ClientFrame::Unsubscribe { signal, channel } => {
            let Some(signal) = signal.or(channel) else {
                send_error(events_tx, "invalid_frame", "unsubscribe requires signal").await;
                return;
            };
            unsubscribe(events_tx, subscriptions, &signal).await;
        }
    }
}

async fn subscribe(
    state: &AppState,
    user: Option<&CurrentUser>,
    events_tx: &mpsc::Sender<Value>,
    subscriptions: &mut HashMap<String, JoinHandle<()>>,
    signal: String,
) {
    if !valid_signal_name(&signal) {
        send_error(events_tx, "invalid_signal", "invalid signal name").await;
        return;
    }
    if let Err(error) = state.signals().check_access(&signal, user) {
        send_signal_error(events_tx, error).await;
        return;
    }
    if subscriptions.contains_key(&signal) {
        let _ = events_tx
            .send(json!({"type": "subscribed", "signal": signal}))
            .await;
        return;
    }
    if subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_SOCKET {
        send_error(
            events_tx,
            "too_many_subscriptions",
            "too many subscriptions",
        )
        .await;
        return;
    }

    let mut receiver = state.signals().subscribe(&signal);
    let tx = events_tx.clone();
    let relay_signal = signal.clone();
    let task = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(payload) => {
                    if tx
                        .send(json!(signal_event(relay_signal.clone(), payload)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    let _ = tx
                        .send(json!({"type": "error", "code": "lagged", "detail": format!("dropped {count} signal event(s)"), "signal": relay_signal}))
                        .await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    subscriptions.insert(signal.clone(), task);
    let _ = events_tx
        .send(json!({"type": "subscribed", "signal": signal}))
        .await;
}

async fn unsubscribe(
    events_tx: &mpsc::Sender<Value>,
    subscriptions: &mut HashMap<String, JoinHandle<()>>,
    signal: &str,
) {
    if let Some(task) = subscriptions.remove(signal) {
        task.abort();
    }
    let _ = events_tx
        .send(json!({"type": "unsubscribed", "signal": signal}))
        .await;
}

async fn send_signal_error(events_tx: &mpsc::Sender<Value>, error: SignalError) {
    send_error(events_tx, error.code(), error.detail()).await;
}

async fn send_error(events_tx: &mpsc::Sender<Value>, code: &str, detail: &str) {
    let _ = events_tx.send(error_event(code, detail)).await;
}
