use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::auth::CurrentUser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAccess {
    Public,
    Authenticated,
    Admin,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SignalEvent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub signal: String,
    pub payload: Value,
}

#[derive(Clone, Default)]
pub struct SignalBus {
    inner: Arc<Mutex<SignalState>>,
}

#[derive(Default)]
struct SignalState {
    senders: HashMap<String, broadcast::Sender<Value>>,
    access: HashMap<String, SignalAccess>,
}

impl SignalBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(&self, signal: impl Into<String>, access: SignalAccess) {
        let signal = signal.into();
        let mut state = self.inner.lock().expect("signal bus lock is poisoned");
        if let Some(existing) = state.access.get(&signal) {
            assert_eq!(
                *existing, access,
                "conflicting access declaration for signal `{signal}`"
            );
        }
        state.access.insert(signal.clone(), access);
        state
            .senders
            .entry(signal)
            .or_insert_with(|| broadcast::channel(64).0);
    }

    pub fn publish(&self, signal: impl Into<String>, payload: Value) {
        let mut state = self.inner.lock().expect("signal bus lock is poisoned");
        let sender = state
            .senders
            .entry(signal.into())
            .or_insert_with(|| broadcast::channel(64).0);
        let _ = sender.send(payload);
    }

    pub fn publish_user(&self, user_id: i64, signal: impl Into<String>, payload: Value) {
        self.publish(format!("user:{user_id}:{}", signal.into()), payload);
    }

    pub fn subscribe(&self, signal: &str) -> broadcast::Receiver<Value> {
        let mut state = self.inner.lock().expect("signal bus lock is poisoned");
        state
            .senders
            .entry(signal.to_owned())
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe()
    }

    pub fn check_access(
        &self,
        signal: &str,
        user: Option<&CurrentUser>,
    ) -> Result<(), SignalError> {
        if let Some(owner_id) = private_user_id(signal) {
            let known = self
                .inner
                .lock()
                .expect("signal bus lock is poisoned")
                .senders
                .contains_key(signal);
            if !known {
                return Err(SignalError::Unknown);
            }
            return match user {
                Some(user) if user.id == owner_id => Ok(()),
                Some(_) => Err(SignalError::Forbidden),
                None => Err(SignalError::Unauthorized),
            };
        }

        let access = self
            .inner
            .lock()
            .expect("signal bus lock is poisoned")
            .access
            .get(signal)
            .copied()
            .ok_or(SignalError::Unknown)?;
        match access {
            SignalAccess::Public => Ok(()),
            SignalAccess::Authenticated => user.map(|_| ()).ok_or(SignalError::Unauthorized),
            SignalAccess::Admin => match user {
                Some(user) if user.is_admin || user.is_superuser => Ok(()),
                Some(_) => Err(SignalError::Forbidden),
                None => Err(SignalError::Unauthorized),
            },
        }
    }

    pub fn public_signals(&self) -> Vec<(String, SignalAccess)> {
        let mut signals = self
            .inner
            .lock()
            .expect("signal bus lock is poisoned")
            .access
            .iter()
            .map(|(name, access)| (name.clone(), *access))
            .collect::<Vec<_>>();
        signals.sort_by(|left, right| left.0.cmp(&right.0));
        signals
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalError {
    Unknown,
    Unauthorized,
    Forbidden,
}

impl SignalError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unknown => "unknown_signal",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Unknown => "unknown signal",
            Self::Unauthorized => "authentication required",
            Self::Forbidden => "signal subscription is forbidden",
        }
    }
}

pub fn signal_event(signal: impl Into<String>, payload: Value) -> SignalEvent {
    SignalEvent {
        kind: "signal",
        signal: signal.into(),
        payload,
    }
}

pub fn error_event(code: &str, detail: &str) -> Value {
    json!({"type": "error", "code": code, "detail": detail})
}

pub fn valid_signal_name(signal: &str) -> bool {
    !signal.is_empty()
        && signal
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '-' | '_' | '.'))
}

fn private_user_id(signal: &str) -> Option<i64> {
    let rest = signal.strip_prefix("user:")?;
    let (id, _) = rest.split_once(':')?;
    id.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64) -> CurrentUser {
        CurrentUser {
            id,
            username: format!("user{id}"),
            is_staff: false,
            is_admin: false,
            is_superuser: false,
        }
    }

    #[test]
    fn private_user_signal_must_exist_before_subscription() {
        let bus = SignalBus::new();
        assert_eq!(
            bus.check_access("user:1:notifications", Some(&user(1))),
            Err(SignalError::Unknown)
        );

        bus.publish_user(1, "notifications", json!({"message": "ready"}));

        assert_eq!(
            bus.check_access("user:1:notifications", Some(&user(1))),
            Ok(())
        );
        assert_eq!(
            bus.check_access("user:1:notifications", Some(&user(2))),
            Err(SignalError::Forbidden)
        );
    }

    #[test]
    #[should_panic(expected = "conflicting access declaration")]
    fn duplicate_signal_declarations_must_keep_same_access() {
        let bus = SignalBus::new();
        bus.declare("tasks.created", SignalAccess::Authenticated);
        bus.declare("tasks.created", SignalAccess::Public);
    }
}
