//! FCM HTTP v1. Credentials are server-only and never enter journals or responses.
use super::models::{PushDelivery, PushDevice};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum SendResult {
    Accepted,
    Retry(&'static str, Option<i64>),
    Unregistered,
    Permanent(&'static str),
}
pub trait PushSender: Send + Sync {
    fn send<'a>(
        &'a self,
        device: &'a PushDevice,
        job: &'a PushDelivery,
    ) -> Pin<Box<dyn Future<Output = SendResult> + Send + 'a>>;
}
#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
}
#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'static str,
    aud: &'static str,
    iat: i64,
    exp: i64,
}
pub struct FcmSender {
    client: reqwest::Client,
    pub(crate) project: String,
    email: String,
    key: EncodingKey,
    cached: Mutex<Option<(String, i64)>>,
    endpoint: String,
    package_name: Option<String>,
}
impl FcmSender {
    pub fn from_env(config: &super::PushConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .map_err(|_| "Push needs GOOGLE_APPLICATION_CREDENTIALS")?;
        let bytes = std::fs::read(path).map_err(|_| "Cannot read push service-account file")?;
        let credentials: ServiceAccount =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid push service-account JSON")?;
        let project = config.project_id.clone().unwrap_or(credentials.project_id);
        if project.is_empty()
            || project.len() > 100
            || !project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err("Invalid Firebase project ID".into());
        }
        let key = EncodingKey::from_rsa_pem(credentials.private_key.as_bytes())
            .map_err(|_| "Invalid push service-account RSA key")?;
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            endpoint: format!("https://fcm.googleapis.com/v1/projects/{project}/messages:send"),
            project,
            package_name: config.package_name.clone(),
            email: credentials.client_email,
            key,
            cached: Mutex::new(None),
        })
    }
    async fn access_token(&self) -> Result<String, ()> {
        let mut cached = self.cached.lock().await;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if let Some((token, expiry)) = &*cached
            && *expiry > now + 60
        {
            return Ok(token.clone());
        }
        let assertion = jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &Claims {
                iss: &self.email,
                scope: "https://www.googleapis.com/auth/firebase.messaging",
                aud: "https://oauth2.googleapis.com/token",
                iat: now,
                exp: now + 3600,
            },
            &self.key,
        )
        .map_err(|_| ())?;
        let response = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let value: Value = response.json().await.map_err(|_| ())?;
        let token = value["access_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or(())?
            .to_owned();
        let expires = value["expires_in"].as_i64().unwrap_or(3600).clamp(60, 3600);
        *cached = Some((token.clone(), now + expires));
        Ok(token)
    }
}
pub fn message(device: &PushDevice, job: &PushDelivery, package: Option<&str>) -> Value {
    let mut data: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&job.data).unwrap_or_default();
    for (key, value) in [
        ("event_id", job.id.to_string()),
        ("target_id", job.target_id.clone()),
        ("target_type", job.target_type.clone()),
        ("user_id", job.user_id.to_string()),
        ("server_key", device.server_key.clone()),
        ("kind", job.kind.clone()),
        ("title", job.title.clone()),
        ("body", job.body.clone()),
    ] {
        data.insert(key.into(), value);
    }
    let mut value = json!({"message":{"token":device.token,"data":data,"android":{"priority":"high","ttl":"86400s"}}});
    if let Some(package) = package {
        value["message"]["android"]["restricted_package_name"] = json!(package);
    }
    value
}
impl PushSender for FcmSender {
    fn send<'a>(
        &'a self,
        device: &'a PushDevice,
        job: &'a PushDelivery,
    ) -> Pin<Box<dyn Future<Output = SendResult> + Send + 'a>> {
        Box::pin(async move {
            let Ok(token) = self.access_token().await else {
                return SendResult::Retry("oauth_unavailable", None);
            };
            let response = self
                .client
                .post(&self.endpoint)
                .bearer_auth(token)
                .json(&message(device, job, self.package_name.as_deref()))
                .send()
                .await;
            let Ok(response) = response else {
                return SendResult::Retry("fcm_unavailable", None);
            };
            let status = response.status();
            if status.is_success() {
                return SendResult::Accepted;
            }
            let retry = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .map(|v| v.clamp(1, 86400));
            let value: Value = response.json().await.unwrap_or(Value::Null);
            let result = classify(status, retry, &value);
            if status.as_u16() == 401 {
                *self.cached.lock().await = None;
            }
            result
        })
    }
}
fn classify(status: reqwest::StatusCode, retry: Option<i64>, value: &Value) -> SendResult {
    if value["error"]["details"].as_array().is_some_and(|details| {
        details
            .iter()
            .any(|detail| detail["errorCode"] == "UNREGISTERED")
    }) {
        return SendResult::Unregistered;
    }
    if status.as_u16() == 401 {
        return SendResult::Retry("fcm_unauthorized", retry);
    }
    if status.as_u16() == 429 || status.is_server_error() {
        SendResult::Retry("fcm_retry", retry)
    } else {
        SendResult::Permanent("fcm_rejected")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_unregistered_disables_device_and_transient_errors_retry() {
        use reqwest::StatusCode as S;
        assert_eq!(
            classify(S::NOT_FOUND, None, &Value::Null),
            SendResult::Permanent("fcm_rejected")
        );
        assert_eq!(
            classify(
                S::NOT_FOUND,
                None,
                &json!({"error":{"details":[{"errorCode":"UNREGISTERED"}]}})
            ),
            SendResult::Unregistered
        );
        assert_eq!(
            classify(S::TOO_MANY_REQUESTS, Some(90), &Value::Null),
            SendResult::Retry("fcm_retry", Some(90))
        );
        assert_eq!(
            classify(S::SERVICE_UNAVAILABLE, None, &Value::Null),
            SendResult::Retry("fcm_retry", None)
        );
        assert_eq!(
            classify(S::UNAUTHORIZED, None, &Value::Null),
            SendResult::Retry("fcm_unauthorized", None)
        );
    }
}
