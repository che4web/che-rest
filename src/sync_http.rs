//! Reusable ordinary REST transport for a Tauri/native ModelLogClient.
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
    fn encode_write(&self, mut write: RestModelWrite) -> Result<RestModelWrite, OrmError> {
        let resource = self.resources.get(&write.model).ok_or_else(|| {
            OrmError::Configuration(format!("no REST resource for {}", write.model))
        })?;
        write.resource = resource.resource.clone();
        if let Some(body) = write.body.as_object() {
            write.body = serde_json::Value::Object(
                body.iter()
                    .map(|(name, value)| {
                        resource
                            .fields
                            .get(name)
                            .map(|field| (field.clone(), value.clone()))
                            .ok_or_else(|| {
                                OrmError::Configuration(format!("unknown writable field {name}"))
                            })
                    })
                    .collect::<Result<_, _>>()?,
            );
        }
        write.json_null_fields = write
            .json_null_fields
            .iter()
            .map(|name| {
                resource
                    .fields
                    .get(name)
                    .cloned()
                    .ok_or_else(|| OrmError::Configuration("unknown JSON field".into()))
            })
            .collect::<Result<_, _>>()?;
        Ok(write)
    }
    fn write<'a>(&'a self, write: &'a RestModelWrite) -> TransportFuture<'a, ModelWriteResult> {
        Box::pin(async move {
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
                .header("x-sync-protocol", MODEL_LOG_PROTOCOL)
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
                ModelWriteResult::Accepted(_) if write.kind == ChangeKind::Create => 201,
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
