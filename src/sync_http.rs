//! Reusable ordinary REST transport for a Tauri/native ModelLogClient.
use crate::RestModelWrite;
use che_orm::{ModelSerializer, OrmError};
use che_sync::{ModelLogTransport, SyncModel, TransportFuture, model_log::*};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct RestResource {
    model: String,
    resource: String,
    fields: BTreeMap<String, String>,
}
impl RestResource {
    pub fn new<M: SyncModel, S: ModelSerializer<Model = M>>(
        resource: &str,
    ) -> Result<Self, OrmError> {
        let resource = resource.trim_matches('/');
        if resource.is_empty()
            || resource
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || resource
                .chars()
                .any(|ch| !ch.is_ascii_alphanumeric() && !matches!(ch, '/' | '_' | '-'))
        {
            return Err(OrmError::Configuration("invalid REST resource path".into()));
        }
        let schema = M::schema();
        let mut fields = BTreeMap::new();
        for sync in M::registered_fields().iter().filter(|field| field.writable) {
            let column = schema
                .columns
                .iter()
                .find(|column| column.name == sync.column)
                .ok_or_else(|| OrmError::Configuration("unknown sync column".into()))?;
            let mappings = S::fields()
                .iter()
                .filter(|field| {
                    !field.read_only
                        && if field.related_model.is_some() {
                            field.source == column.name
                        } else {
                            field.source == column.field_name
                        }
                })
                .collect::<Vec<_>>();
            if mappings.len() != 1 {
                return Err(OrmError::Configuration(format!(
                    "sync field {} needs one writable serializer mapping",
                    sync.name
                )));
            }
            fields.insert(sync.name.into(), mappings[0].name.into());
        }
        Ok(Self {
            model: M::SYNC_NAME.into(),
            resource: resource.into(),
            fields,
        })
    }
}

pub struct RestModelTransport {
    base: String,
    client: reqwest::Client,
    resources: BTreeMap<String, RestResource>,
}
impl RestModelTransport {
    /// Configure bearer/token authentication and timeouts on `client`. The base
    /// URL includes the API prefix, e.g. https://host/api. Credentials are never
    /// persisted inside the immutable request body.
    pub fn new(
        base: &str,
        client: reqwest::Client,
        resources: Vec<RestResource>,
    ) -> Result<Self, OrmError> {
        let url = reqwest::Url::parse(base)
            .map_err(|error| OrmError::Configuration(error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(OrmError::Configuration("invalid API base URL".into()));
        }
        let mut mappings = BTreeMap::new();
        for resource in resources {
            if mappings.insert(resource.model.clone(), resource).is_some() {
                return Err(OrmError::Configuration(
                    "duplicate REST model resource".into(),
                ));
            }
        }
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            client,
            resources: mappings,
        })
    }
}
fn network(error: reqwest::Error) -> OrmError {
    OrmError::Interaction(error.to_string())
}
impl ModelLogTransport for RestModelTransport {
    fn id(&self) -> &'static str {
        "che-rest.model.v2"
    }
    fn legacy_payload_key(&self, operation: che_orm::Uuid) -> Option<String> {
        Some(format!("rest-request:{operation}"))
    }
    fn prepare_write(&self, write: &ModelWrite) -> Result<serde_json::Value, OrmError> {
        let resource = self.resources.get(&write.model).ok_or_else(|| {
            OrmError::Configuration(format!("no REST resource for {}", write.model))
        })?;
        let field_name =
            |name: &str| {
                resource.fields.get(name).cloned().ok_or_else(|| {
                    OrmError::Configuration(format!("unknown writable field {name}"))
                })
            };
        let body = write
            .changes
            .as_ref()
            .map(|changes| {
                changes
                    .iter()
                    .map(|(name, value)| Ok((field_name(name)?, value.clone())))
                    .collect::<Result<serde_json::Map<_, _>, OrmError>>()
            })
            .transpose()?
            .map(serde_json::Value::Object)
            .unwrap_or(serde_json::Value::Null);
        let json_null_fields = write
            .changes
            .as_ref()
            .map(|changes| {
                changes
                    .iter()
                    .filter(|(name, value)| {
                        value.is_null() && !write.sql_null_fields.contains(name)
                    })
                    .map(|(name, _)| field_name(name))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        serde_json::to_value(RestModelWrite {
            model: write.model.clone(),
            resource: resource.resource.clone(),
            record_id: write.record_id,
            operation_id: write.operation_id,
            replica_id: write.replica_id,
            base_revision: write.base_revision,
            schema_version: write.schema_version,
            kind: write.kind,
            body,
            json_null_fields,
        })
        .map_err(|error| OrmError::Configuration(error.to_string()))
    }
    fn write<'a>(
        &'a self,
        mutation: &'a ModelWrite,
        payload: &'a serde_json::Value,
    ) -> TransportFuture<'a, ModelWriteResult> {
        Box::pin(async move {
            let response = self
                .send_write(mutation, payload, MODEL_LOG_PROTOCOL)
                .await?;
            let status = response.status();
            if response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .map(str::trim)
                != Some("application/vnd.che-sync.v2+json")
            {
                return Err(OrmError::Interaction(format!(
                    "HTTP {status}; Retry-After: {}",
                    response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("unspecified")
                )));
            }
            let result: ModelWriteResult = response.json().await.map_err(network)?;
            let expected = match &result {
                ModelWriteResult::Accepted(_) if mutation.kind == ChangeKind::Create => 201,
                ModelWriteResult::Accepted(_) => 200,
                ModelWriteResult::Conflict(_) => 412,
                ModelWriteResult::Rejected { .. } => 400,
            };
            if status.as_u16() != expected {
                return Err(OrmError::Interaction(
                    "HTTP status does not match canonical result".into(),
                ));
            }
            Ok(result)
        })
    }
    fn changes<'a>(
        &'a self,
        model: &'a str,
        after: i64,
        limit: u32,
    ) -> TransportFuture<'a, ModelChanges> {
        Box::pin(async move {
            let resource = self
                .resources
                .get(model)
                .ok_or_else(|| OrmError::Configuration(format!("no REST resource for {model}")))?;
            self.client
                .get(format!("{}/{}/sync/changes/", self.base, resource.resource))
                .query(&[("after", after.to_string()), ("limit", limit.to_string())])
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?
                .json()
                .await
                .map_err(network)
        })
    }
}

impl RestModelTransport {
    async fn send_write(
        &self,
        mutation: &ModelWrite,
        payload: &serde_json::Value,
        protocol: u32,
    ) -> Result<reqwest::Response, OrmError> {
        let write: RestModelWrite = serde_json::from_value(payload.clone())
            .map_err(|error| OrmError::Configuration(error.to_string()))?;
        if write.model != mutation.model
            || write.record_id != mutation.record_id
            || write.operation_id != mutation.operation_id
            || write.replica_id != mutation.replica_id
            || write.base_revision != mutation.base_revision
            || write.schema_version != mutation.schema_version
            || write.kind != mutation.kind
        {
            return Err(OrmError::Configuration(
                "REST payload changed operation identity".into(),
            ));
        }
        let method = match write.kind {
            ChangeKind::Create => reqwest::Method::POST,
            ChangeKind::Update => reqwest::Method::PATCH,
            ChangeKind::Delete => reqwest::Method::DELETE,
        };
        let url = if write.kind == ChangeKind::Create {
            format!("{}/{}/", self.base, write.resource)
        } else {
            format!("{}/{}/{}/", self.base, write.resource, write.record_id)
        };
        let response = self
            .client
            .request(method, url)
            .header("x-sync-protocol", protocol)
            .header("x-sync-replica", write.replica_id.to_string())
            .header("idempotency-key", write.operation_id.to_string())
            .header("if-match", format!("\"{}\"", write.base_revision))
            .header("x-sync-version", write.schema_version)
            .header("x-sync-entity", write.record_id.to_string())
            .header(
                "x-sync-json-null-fields",
                serde_json::to_string(&write.json_null_fields)
                    .map_err(|error| OrmError::Configuration(error.to_string()))?,
            )
            .json(&write.body)
            .send()
            .await
            .map_err(network)?;
        Ok(response)
    }
}

/// Explicit protocol-3 adapter. Its immutable payload namespace is separate
/// from protocol 2; submitted v2 requests must settle before upgrading a client.
pub struct RestObjectTransport(RestModelTransport);
impl RestObjectTransport {
    pub fn new(
        base: &str,
        client: reqwest::Client,
        resources: Vec<RestResource>,
    ) -> Result<Self, OrmError> {
        Ok(Self(RestModelTransport::new(base, client, resources)?))
    }
}
impl ModelLogTransport for RestObjectTransport {
    fn id(&self) -> &'static str {
        "che-rest.object.v3"
    }
    fn prepare_write(&self, write: &ModelWrite) -> Result<serde_json::Value, OrmError> {
        self.0.prepare_write(write)
    }
    fn write<'a>(
        &'a self,
        _: &'a ModelWrite,
        _: &'a serde_json::Value,
    ) -> TransportFuture<'a, ModelWriteResult> {
        Box::pin(async {
            Err(OrmError::Configuration(
                "object transport requires a protocol-3 client".into(),
            ))
        })
    }
    fn changes<'a>(&'a self, _: &'a str, _: i64, _: u32) -> TransportFuture<'a, ModelChanges> {
        Box::pin(async {
            Err(OrmError::Configuration(
                "object transport requires a protocol-3 client".into(),
            ))
        })
    }
    fn object_write<'a>(
        &'a self,
        write: &'a ModelWrite,
        payload: &'a serde_json::Value,
    ) -> TransportFuture<'a, che_sync::object_log::ObjectWriteResult> {
        Box::pin(async move {
            use che_sync::object_log::*;
            let response = self
                .0
                .send_write(write, payload, OBJECT_LOG_PROTOCOL)
                .await?;
            let status = response.status();
            if response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .map(str::trim)
                != Some("application/vnd.che-sync.v3+json")
            {
                return Err(OrmError::Interaction(format!(
                    "HTTP {status}; no canonical protocol-3 settlement"
                )));
            }
            let result: ObjectWriteResult = response.json().await.map_err(network)?;
            let outcome = match &result {
                ObjectWriteResult::Acknowledged {
                    operation_id,
                    outcome,
                } => {
                    if *operation_id != write.operation_id {
                        return Err(OrmError::Interaction("wrong settlement operation".into()));
                    }
                    *outcome
                }
                ObjectWriteResult::Canonical(r) => match r {
                    ModelWriteResult::Accepted(_) => WriteOutcome::Accepted,
                    ModelWriteResult::Conflict(_) => WriteOutcome::Conflict,
                    ModelWriteResult::Rejected { .. } => WriteOutcome::Rejected,
                },
            };
            let expected = match outcome {
                WriteOutcome::Accepted if write.kind == ChangeKind::Create => 201,
                WriteOutcome::Accepted => 200,
                WriteOutcome::Conflict => 412,
                WriteOutcome::Rejected => 400,
            };
            if status.as_u16() != expected {
                return Err(OrmError::Interaction(
                    "HTTP status does not match object settlement".into(),
                ));
            }
            Ok(result)
        })
    }
    fn object_changes<'a>(
        &'a self,
        model: &'a str,
        cursor: Option<che_sync::object_log::ObjectCursor>,
        limit: u32,
    ) -> TransportFuture<'a, che_sync::object_log::ObjectChanges> {
        Box::pin(async move {
            let resource = self
                .0
                .resources
                .get(model)
                .ok_or_else(|| OrmError::Configuration("unknown object resource".into()))?;
            self.0
                .client
                .post(format!(
                    "{}/{}/sync/objects/",
                    self.0.base, resource.resource
                ))
                .json(&serde_json::json!({"protocol_version":3,"cursor":cursor,"limit":limit}))
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?
                .json()
                .await
                .map_err(network)
        })
    }
}
