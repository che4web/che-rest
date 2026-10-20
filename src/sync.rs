//! Authenticated synchronization endpoints and journal-aware ViewSet CRUD.
use std::sync::Arc;

use axum::{
    Extension, Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use che_orm::{
    Model, ModelSerializer, ModelWriteSerializer, OrmError, Uuid, ValidatedWrite, WriteMode,
};
use che_sync::{AccessPolicy, Registry, SyncModel, SyncServer, protocol::*};
use serde_json::{Value, json};

use crate::{
    ApiSchema, ApiSchemaDefinition, AppError, AppResult, AppState, CurrentPrincipal,
    ExtensionContext, MutationHook, OperationSpec, Permission, RestQuerySet, ViewAction, ViewSet,
    ViewSetExtension,
    rest::extensions::{MutationBackend, MutationFuture},
};

pub use che_sync;

/// One immutable sync operation encoded as an ordinary serializer JSON write.
pub struct RestSyncRequest {
    pub method: axum::http::Method,
    pub entity_id: Uuid,
    pub headers: HeaderMap,
    pub body: Value,
}

impl RestSyncRequest {
    pub fn new<M, S>(request: PushRequest) -> Result<Self, OrmError>
    where
        M: SyncModel,
        S: ModelSerializer<Model = M>,
    {
        if request.protocol_version != PROTOCOL_VERSION || request.operation.mutations.len() != 1 {
            return Err(OrmError::Configuration(
                "REST sync requires one mutation".into(),
            ));
        }
        let mutation = &request.operation.mutations[0];
        if mutation.model != M::SYNC_NAME || mutation.schema_version != M::SYNC_VERSION {
            return Err(OrmError::Configuration("REST sync model mismatch".into()));
        }
        let schema = M::schema();
        let mappings = M::registered_fields();
        let mut body = serde_json::Map::new();
        if let Some(changes) = &mutation.changes {
            for (name, value) in changes {
                let mapping = mappings
                    .iter()
                    .find(|field| field.name == name && field.writable)
                    .ok_or_else(|| {
                        OrmError::Configuration(format!("unwritable sync field {name}"))
                    })?;
                let column = schema
                    .columns
                    .iter()
                    .find(|column| column.name == mapping.column)
                    .unwrap();
                let field = S::fields()
                    .iter()
                    .find(|field| {
                        !field.read_only
                            && if field.related_model.is_some() {
                                field.source == column.name
                            } else {
                                field.source == column.field_name
                            }
                    })
                    .ok_or_else(|| {
                        OrmError::Configuration(format!(
                            "sync field {name} has no writable serializer field"
                        ))
                    })?;
                let value = match value {
                    che_sync::protocol::Value::Integer(value) => json!(value.to_string()),
                    che_sync::protocol::Value::Real(value) => json!(value),
                    che_sync::protocol::Value::Text(value) => json!(value),
                    che_sync::protocol::Value::Binary(value) => json!(value),
                    che_sync::protocol::Value::Boolean(value) => json!(value),
                    che_sync::protocol::Value::Uuid(value) => json!(value),
                    che_sync::protocol::Value::Null => Value::Null,
                    che_sync::protocol::Value::DateTime(value) => json!(
                        value
                            .format(&che_orm::time::format_description::well_known::Rfc3339)
                            .map_err(|error| OrmError::Configuration(error.to_string()))?
                    ),
                };
                body.insert(field.name.into(), value);
            }
        }
        let method = if mutation.changes.is_none() {
            axum::http::Method::DELETE
        } else if mutation.base_revision == 0 {
            axum::http::Method::POST
        } else {
            axum::http::Method::PATCH
        };
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("x-sync-protocol", request.protocol_version.to_string()),
            ("x-sync-replica", request.replica_id.to_string()),
            ("idempotency-key", request.operation.id.to_string()),
            ("if-match", format!("\"{}\"", mutation.base_revision)),
            ("x-sync-entity", mutation.id.to_string()),
            ("x-sync-version", mutation.schema_version.to_string()),
            ("x-sync-scope", request.scope),
        ] {
            headers.insert(
                axum::http::HeaderName::from_static(name),
                value
                    .parse()
                    .map_err(|_| OrmError::Configuration("invalid REST sync header".into()))?,
            );
        }
        Ok(Self {
            method,
            entity_id: mutation.id,
            headers,
            body: if mutation.changes.is_none() {
                Value::Null
            } else {
                Value::Object(body)
            },
        })
    }
}

pub(crate) trait SyncLifecycle: Send + Sync {
    fn path(&self) -> &'static str;
    fn scope(&self, state: &AppState, principal: &CurrentPrincipal) -> AppResult<String>;
    fn actions(&self) -> Vec<String>;
    fn prepare_delete<'a>(
        &'a self,
        state: &'a AppState,
        entity: &'a Entity,
    ) -> MutationFuture<'a, std::collections::BTreeMap<String, Value>>;
    fn model_logs(&self) -> bool;
    fn committed(
        &self,
        state: &AppState,
        principal: Option<&CurrentPrincipal>,
        old: Option<&Entity>,
        entity: &Entity,
        payload: &EffectPayload,
        action_id: &str,
    ) -> AppResult<()>;
}

struct Lifecycle<V: SyncViewSet>
where
    V::Model: SyncModel,
{
    viewset: V,
    hooks: Vec<Arc<dyn MutationHook<V>>>,
    model_logs: bool,
}

fn materialize<M: SyncModel>(entity: &Entity) -> AppResult<M> {
    let record = entity.record.as_ref().ok_or(AppError::NotFound)?;
    let fields = M::registered_fields();
    let values = M::schema()
        .columns
        .iter()
        .map(|column| {
            let field = fields
                .iter()
                .find(|field| field.column == column.name)
                .ok_or_else(|| AppError::BadRequest("unmapped sync column".into()))?;
            let value = record
                .get(field.name)
                .ok_or_else(|| AppError::BadRequest("incomplete canonical record".into()))?;
            Ok(database_value(value))
        })
        .collect::<AppResult<Vec<_>>>()?;
    M::from_row(&che_orm::Row::new(values)).map_err(|e| AppError::Orm(e.into()))
}

impl<V: SyncViewSet> SyncLifecycle for Lifecycle<V>
where
    V::Model: SyncModel,
{
    fn path(&self) -> &'static str {
        self.viewset.path()
    }
    fn scope(&self, state: &AppState, principal: &CurrentPrincipal) -> AppResult<String> {
        if self.model_logs {
            Ok(che_sync::model_log::MODEL_LOG_SCOPE.into())
        } else {
            self.viewset.sync_scope(state, principal)
        }
    }
    fn actions(&self) -> Vec<String> {
        self.hooks
            .iter()
            .filter(|hook| hook.backend().is_none())
            .map(|hook| hook.id().to_owned())
            .chain(std::iter::once("@signal".into()))
            .collect()
    }
    fn model_logs(&self) -> bool {
        self.model_logs
    }
    fn prepare_delete<'a>(
        &'a self,
        state: &'a AppState,
        entity: &'a Entity,
    ) -> MutationFuture<'a, std::collections::BTreeMap<String, Value>> {
        Box::pin(async move {
            let model = materialize::<V::Model>(entity)?;
            let mut values = std::collections::BTreeMap::new();
            for hook in &self.hooks {
                values.insert(
                    hook.id().into(),
                    hook.prepare_delete_async(state, &model).await?,
                );
            }
            Ok(values)
        })
    }
    fn committed(
        &self,
        state: &AppState,
        principal: Option<&CurrentPrincipal>,
        old: Option<&Entity>,
        entity: &Entity,
        payload: &EffectPayload,
        action_id: &str,
    ) -> AppResult<()> {
        let (payload, write_action) = match payload {
            EffectPayload::Handler {
                version: 2,
                value,
                action,
            } => (value.as_ref(), *action),
            EffectPayload::Handler { .. } => {
                return Err(AppError::BadRequest(
                    "unsupported effect payload version".into(),
                ));
            }
            EffectPayload::Current {
                values: Some(_), ..
            }
            | EffectPayload::Legacy(Some(_)) => {
                return Err(AppError::BadRequest(
                    "legacy positional cleanup requires explicit payload migration".into(),
                ));
            }
            EffectPayload::Current {
                values: None,
                action,
            } => (None, *action),
            EffectPayload::Legacy(None) => (None, None),
        };
        if action_id != "@signal" && !self.hooks.iter().any(|hook| hook.id() == action_id) {
            return Err(AppError::BadRequest(format!(
                "effect handler {action_id} is not installed"
            )));
        }
        let action = if entity.record.is_none() {
            {
                let payload = payload.unwrap_or(&Value::Null);
                for hook in &self.hooks {
                    if hook.id() == action_id {
                        hook.after_delete(state, payload)?;
                    }
                }
            }
            crate::ViewAction::Delete
        } else {
            let model = materialize::<V::Model>(entity)?;
            let action = if let Some(action) = write_action {
                action
            } else if old.is_none_or(|old| old.revision == 0) {
                crate::ViewAction::Create
            } else {
                crate::ViewAction::Patch
            };
            for hook in &self.hooks {
                if hook.id() == action_id {
                    hook.after_commit(state, principal, action, &model)?;
                }
            }
            action
        };
        if action_id == "@signal"
            && self.viewset.signal_access(action).is_some()
            && let Some(signal) = self.viewset.signal_name(action)
        {
            state.signals().publish(signal, json!({"id": entity.id}));
        }
        Ok(())
    }
}

/// Each ViewSet serves its model and automatically includes its FK parents.
/// The registry describes the related models needed for dependency/cascade
/// capture; it does not combine their outgoing queues or cursors.
/// Scope and principal are derived from authenticated context, never JSON.
/// Ordinary reads must use a queryset/permission matching the same access rules.
pub trait SyncViewSet: ViewSet
where
    Self::Model: SyncModel,
{
    fn sync_registry(&self) -> AppResult<Registry>;
    fn sync_scope(&self, _state: &AppState, _principal: &CurrentPrincipal) -> AppResult<String> {
        Ok(che_sync::model_log::MODEL_LOG_SCOPE.into())
    }
    fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
        Arc::new(che_sync::AllowAll)
    }
}

/// Install in `ViewSet::configure`. Also replaces CRUD persistence so changes
/// and the sync journal commit atomically. Sync tables must already be migrated.
#[derive(Clone)]
pub struct SyncEndpoints<V> {
    viewset: V,
    body_limit: usize,
    model_logs: bool,
}

impl<V> SyncEndpoints<V> {
    pub fn new(viewset: V) -> Self {
        Self {
            viewset,
            body_limit: 2 * 1024 * 1024,
            model_logs: false,
        }
    }

    pub fn body_limit(mut self, bytes: usize) -> Self {
        self.body_limit = bytes;
        self
    }
}

/// Whole-model synchronization through ordinary REST writes and a dedicated
/// per-model GET journal. No workspace or scope header is accepted/required.
#[derive(Clone)]
pub struct ModelSyncEndpoints<V>(SyncEndpoints<V>);
impl<V> ModelSyncEndpoints<V> {
    pub fn new(viewset: V) -> Self {
        let mut endpoints = SyncEndpoints::new(viewset);
        endpoints.model_logs = true;
        Self(endpoints)
    }
}
impl<V: SyncViewSet> ViewSetExtension<V> for ModelSyncEndpoints<V>
where
    V::Model: SyncModel,
{
    const ID: &'static str = "model-sync";
    fn install(self, context: &mut ExtensionContext<'_, V>) -> AppResult<()> {
        self.0.install(context)
    }
}

async fn server<V: SyncViewSet>(
    viewset: &V,
    state: &AppState,
    principal: Option<&CurrentPrincipal>,
) -> AppResult<SyncServer>
where
    V::Model: SyncModel,
{
    server_for(viewset, state, principal, false).await
}

async fn server_for<V: SyncViewSet>(
    viewset: &V,
    state: &AppState,
    principal: Option<&CurrentPrincipal>,
    model_logs: bool,
) -> AppResult<SyncServer>
where
    V::Model: SyncModel,
{
    let principal =
        principal.ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
    let scope = if model_logs {
        che_sync::model_log::MODEL_LOG_SCOPE.into()
    } else {
        viewset.sync_scope(state, principal)?
    };
    let identity = principal.auth_user().id.to_string();
    let policy = viewset.sync_policy();
    policy
        .authorize_scope(&identity, &scope)
        .map_err(|error| match error {
            OrmError::Configuration(reason) => AppError::Forbidden(reason),
            other => AppError::Orm(other),
        })?;
    let server = if model_logs {
        SyncServer::open_model_logs(
            state.database().clone(),
            viewset.sync_registry()?,
            identity,
            policy,
        )
        .await
    } else {
        SyncServer::open(
            state.database().clone(),
            viewset.sync_registry()?,
            scope,
            identity,
            policy,
        )
        .await
    };
    server
        .map_err(sync_error)?
        .for_model::<V::Model>()
        .map_err(sync_error)
}

fn sync_error(error: OrmError) -> AppError {
    match error {
        OrmError::Configuration(reason) => AppError::BadRequest(reason),
        OrmError::QueryBuild(error) => AppError::BadRequest(format!("{error:?}")),
        other => AppError::Orm(other),
    }
}

impl<V: SyncViewSet> ViewSetExtension<V> for SyncEndpoints<V>
where
    V::Model: SyncModel,
{
    const ID: &'static str = "sync";

    fn install(self, context: &mut ExtensionContext<'_, V>) -> AppResult<()> {
        if self.body_limit == 0 {
            return Err(AppError::BadRequest(
                "sync body limit must be positive".into(),
            ));
        }
        self.viewset.sync_registry()?.check_model::<V::Model>()?;
        let sync_fields = V::Model::registered_fields();
        for field in V::Serializer::fields()
            .iter()
            .filter(|field| !field.read_only)
        {
            let schema = V::Model::schema();
            let column = schema.columns.iter().find(|column| {
                if field.related_model.is_some() {
                    column.name == field.source
                } else {
                    column.field_name == field.source
                }
            });
            if let Some(column) = column
                && sync_fields
                    .iter()
                    .any(|sync| sync.column == column.name && !sync.writable)
            {
                return Err(AppError::BadRequest(format!(
                    "serializer field {} must be read-only: its sync column is server-owned",
                    field.name
                )));
            }
        }
        let schema = V::Model::schema();
        for sync in sync_fields.iter().filter(|field| field.writable) {
            let column = schema
                .columns
                .iter()
                .find(|column| column.name == sync.column)
                .ok_or_else(|| {
                    AppError::BadRequest(format!("unknown sync column {}", sync.column))
                })?;
            let mappings = V::Serializer::fields()
                .iter()
                .filter(|field| {
                    !field.read_only
                        && if field.related_model.is_some() {
                            field.source == column.name
                        } else {
                            field.source == column.field_name
                        }
                })
                .count();
            if mappings != 1 {
                return Err(AppError::BadRequest(format!(
                    "sync model {} field {} requires exactly one writable serializer mapping (found {mappings})",
                    V::Model::SYNC_NAME,
                    sync.name,
                )));
            }
        }
        let prefix = self.viewset.path().trim_matches('/').replace('/', "_");
        if self.model_logs {
            context.route("sync/changes", |route| {
                route.get(
                    model_changes::<V>,
                    OperationSpec::new(format!("{prefix}_sync_changes"))
                        .authenticated()
                        .response::<che_sync::protocol::model_log::ModelChanges>(StatusCode::OK)
                        .client("sync", "changes"),
                )
            })?;
            return context.mutation_hook(self);
        }
        let spec = |action: &str| {
            OperationSpec::new(format!("{prefix}_sync_{action}"))
                .authenticated()
                .body_limit(self.body_limit)
                .error(StatusCode::BAD_REQUEST)
                .error(StatusCode::UNAUTHORIZED)
                .error(StatusCode::FORBIDDEN)
        };
        context.route("sync/pull", |route| {
            route.post(
                pull::<V>,
                spec("pull")
                    .client("sync", "pull")
                    .request::<PullRequest>()
                    .response::<PullResult>(StatusCode::OK),
            )
        })?;
        context.route("sync/snapshot", |route| {
            route.get(
                snapshot::<V>,
                spec("snapshot")
                    .client("sync", "snapshot")
                    .response::<Snapshot>(StatusCode::OK),
            )
        })?;
        context.mutation_hook(self)
    }
}

async fn commit_rest<V: SyncViewSet>(
    state: &AppState,
    viewset: &V,
    principal: &CurrentPrincipal,
    server: &SyncServer,
    request: PushRequest,
    identity: String,
    action: ViewAction,
) -> AppResult<PushResult>
where
    V::Model: SyncModel,
{
    let registry = viewset.sync_registry()?;
    let before = server.prepare_push(&request).await.map_err(sync_error)?;
    let handlers = state
        .sync_lifecycle
        .read()
        .expect("sync lifecycle lock")
        .clone();
    let mut prepared = Vec::new();
    for entity in registry.deleted_in_operation(&before, &request.operation)? {
        if let Some(lifecycles) = handlers.get(&entity.model) {
            for lifecycle in lifecycles {
                if lifecycle.scope(state, principal)? == request.scope {
                    let values = lifecycle.prepare_delete(state, &entity).await?;
                    prepared.push((entity.model.clone(), entity.id, lifecycle.clone(), values));
                }
            }
        }
    }
    let scope = request.scope.clone();
    let mut effects = Vec::new();
    for entity in &before {
        if let Some(lifecycles) = handlers.get(&entity.model) {
            for lifecycle in lifecycles {
                if lifecycle.scope(state, principal)? != scope {
                    continue;
                }
                let values = prepared
                    .iter()
                    .find(|(model, id, handler, _)| {
                        model == &entity.model
                            && *id == entity.id
                            && Arc::ptr_eq(handler, lifecycle)
                    })
                    .map(|(_, _, _, values)| values);
                for effect_action in lifecycle.actions() {
                    effects.push(che_sync::PreparedEffect {
                        model: entity.model.clone(),
                        entity_id: entity.id,
                        handler: lifecycle.path().into(),
                        action: effect_action.clone(),
                        payload: serde_json::to_string(&EffectPayload::Handler {
                            version: 2,
                            value: values
                                .and_then(|values| values.get(&effect_action))
                                .cloned(),
                            action: (entity.model == V::Model::SYNC_NAME
                                && lifecycle.path() == viewset.path())
                            .then_some(action),
                        })?,
                    });
                }
            }
        }
    }
    let result = server
        .write_rest_prepared(request, before.clone(), effects, identity)
        .await
        .map_err(sync_error)?;
    retry_effects(state, principal, server, &scope).await;
    Ok(result)
}

// Read the legacy payload shape too: pending effects survive upgrades.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum EffectPayload {
    Handler {
        version: u32,
        value: Option<Value>,
        action: Option<ViewAction>,
    },
    Current {
        values: Option<Vec<Value>>,
        action: Option<ViewAction>,
    },
    Legacy(Option<Vec<Value>>),
}

/// Retry all per-model effects from a trusted background worker, including
/// effects whose initiating user no longer sends requests. Handler callbacks
/// receive no live authentication credential; persisted payloads own context.
pub async fn retry_model_effects(state: &AppState) -> AppResult<usize> {
    let _guard = state.sync_effects_lock.lock().await;
    let handlers = state
        .sync_lifecycle
        .read()
        .expect("sync lifecycle lock")
        .clone();
    let rows = state
        .database()
        .query_sql(
            "SELECT id, payload FROM _sync_effects WHERE scope = $1 ORDER BY id",
            vec![che_orm::DatabaseValue::Text(
                che_sync::model_log::MODEL_LOG_SCOPE.into(),
            )],
        )
        .await?;
    let mut completed = 0;
    for row in rows {
        let result = async {
            let effect: che_sync::PostCommitEffect = serde_json::from_str(
                &row.get::<String>(1)
                    .map_err(|error| AppError::Orm(error.into()))?,
            )?;
            let handler = handlers
                .get(&effect.entity.model)
                .and_then(|handlers| {
                    handlers.iter().find(|handler| {
                        handler.model_logs() && handler.path() == effect.prepared.handler
                    })
                })
                .ok_or_else(|| AppError::BadRequest("effect handler is not installed".into()))?;
            let payload = serde_json::from_str(&effect.prepared.payload)?;
            handler.committed(
                state,
                None,
                effect.previous.as_ref(),
                &effect.entity,
                &payload,
                &effect.prepared.action,
            )?;
            state
                .database()
                .execute_sql(
                    "DELETE FROM _sync_effects WHERE id = $1 AND scope = $2",
                    vec![
                        che_orm::DatabaseValue::Text(effect.id.to_string()),
                        che_orm::DatabaseValue::Text(che_sync::model_log::MODEL_LOG_SCOPE.into()),
                    ],
                )
                .await?;
            Ok::<_, AppError>(())
        }
        .await;
        match result {
            Ok(()) => completed += 1,
            Err(error) => eprintln!("model effect deferred: {error}"),
        }
    }
    Ok(completed)
}

async fn retry_effects(
    state: &AppState,
    principal: &CurrentPrincipal,
    server: &SyncServer,
    scope: &str,
) {
    if let Err(error) = flush_effects(state, principal, server, scope).await {
        eprintln!("sync effects deferred: {error}");
    }
}

async fn flush_effects(
    state: &AppState,
    principal: &CurrentPrincipal,
    server: &SyncServer,
    scope: &str,
) -> AppResult<()> {
    let handlers = state
        .sync_lifecycle
        .read()
        .expect("sync lifecycle lock")
        .clone();
    let _effects_guard = state.sync_effects_lock.lock().await;
    for effect in server.pending_effects().await? {
        if let Some(lifecycle) = handlers.get(&effect.entity.model).and_then(|handlers| {
            handlers
                .iter()
                .find(|handler| handler.path() == effect.prepared.handler)
        }) {
            let result = async {
                if lifecycle.scope(state, principal)? != scope {
                    return Ok(());
                }
                let payload = serde_json::from_str::<EffectPayload>(&effect.prepared.payload)?;
                lifecycle.committed(
                    state,
                    Some(principal),
                    effect.previous.as_ref(),
                    &effect.entity,
                    &payload,
                    &effect.prepared.action,
                )?;
                server.acknowledge_effect(effect.id).await?;
                Ok::<_, AppError>(())
            }
            .await;
            if let Err(error) = result {
                eprintln!("sync effect {} deferred: {error}", effect.id);
            }
        }
    }
    Ok(())
}

async fn pull<V: SyncViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentPrincipal>>,
    Json(request): Json<PullRequest>,
) -> AppResult<Json<PullResult>>
where
    V::Model: SyncModel,
{
    let server = server(&viewset, &state, who.as_ref().map(|p| &p.0)).await?;
    Ok(Json(server.pull(request).await.map_err(sync_error)?))
}

async fn snapshot<V: SyncViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentPrincipal>>,
) -> AppResult<Json<Snapshot>>
where
    V::Model: SyncModel,
{
    let server = server(&viewset, &state, who.as_ref().map(|p| &p.0)).await?;
    Ok(Json(server.snapshot().await.map_err(sync_error)?))
}

async fn model_changes<V: SyncViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentPrincipal>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> AppResult<Json<che_sync::protocol::model_log::ModelChanges>>
where
    V::Model: SyncModel,
{
    let principal = who.as_ref().map(|who| &who.0);
    V::Permission::default().check(&state, principal, ViewAction::List)?;
    let after = params
        .get("after")
        .map_or("0", String::as_str)
        .parse()
        .map_err(|_| AppError::BadRequest("invalid model cursor".into()))?;
    let limit = params
        .get("limit")
        .map_or("100", String::as_str)
        .parse()
        .map_err(|_| AppError::BadRequest("invalid page limit".into()))?;
    let server = server_for(&viewset, &state, principal, true).await?;
    Ok(Json(
        server
            .changes::<V::Model>(after, limit)
            .await
            .map_err(sync_error)?,
    ))
}

fn write_response<V: SyncViewSet>(
    viewset: &V,
    result: PushResult,
    action: ViewAction,
    model_logs: bool,
) -> AppResult<Response>
where
    V::Model: SyncModel,
{
    if !model_logs {
        return Ok(rest_response(result, action, V::Model::SYNC_NAME));
    }
    let status = match &result {
        PushResult::Accepted(_) if action == ViewAction::Create => StatusCode::CREATED,
        PushResult::Accepted(_) => StatusCode::OK,
        PushResult::Conflict(_) => StatusCode::PRECONDITION_FAILED,
        PushResult::Rejected { .. } => StatusCode::BAD_REQUEST,
    };
    let revision = match &result {
        PushResult::Accepted(entities) => entities
            .iter()
            .find(|entity| entity.model == V::Model::SYNC_NAME)
            .map(|entity| entity.revision),
        _ => None,
    };
    let result = viewset
        .sync_registry()?
        .model_result(result)
        .map_err(sync_error)?;
    let mut response = (status, Json(result)).into_response();
    response.headers_mut().insert(
        "content-type",
        "application/vnd.che-sync.v2+json".parse().unwrap(),
    );
    if let Some(revision) = revision {
        response
            .headers_mut()
            .insert("etag", format!("\"{revision}\"").parse().unwrap());
    }
    Ok(response)
}

impl<V: SyncViewSet> MutationHook<V> for SyncEndpoints<V>
where
    V::Model: SyncModel,
{
    fn id(&self) -> &'static str {
        "sync"
    }
    fn installed(&self, state: &AppState, viewset: &V, hooks: &[Arc<dyn MutationHook<V>>]) {
        let mut handlers = state.sync_lifecycle.write().expect("sync lifecycle lock");
        let handlers = handlers.entry(V::Model::SYNC_NAME.into()).or_default();
        handlers.retain(|handler| handler.path() != viewset.path());
        handlers.push(Arc::new(Lifecycle {
            viewset: viewset.clone(),
            hooks: hooks.to_vec(),
            model_logs: self.model_logs,
        }));
    }
    fn backend(&self) -> Option<&dyn MutationBackend<V>> {
        Some(self)
    }
}

fn accepted(result: PushResult) -> AppResult<Vec<Entity>> {
    match result {
        PushResult::Accepted(entities) => Ok(entities),
        PushResult::Conflict(_) => Err(AppError::Conflict(
            "concurrent sync write; retry with fresh state".into(),
        )),
        PushResult::Rejected { reason } => Err(AppError::BadRequest(reason)),
    }
}

impl<V: SyncViewSet> MutationBackend<V> for SyncEndpoints<V>
where
    V::Model: SyncModel,
{
    fn filter_read(&self, queryset: V::QuerySet) -> V::QuerySet {
        if self.model_logs {
            queryset.filter(
                che_orm::ModelField::<V::Model, Option<che_orm::time::OffsetDateTime>>::new(
                    V::Model::table_name(),
                    V::Model::DELETED_AT_COLUMN,
                )
                .is_null(),
            )
        } else {
            queryset
        }
    }
    fn rest<'a>(
        &'a self,
        state: &'a AppState,
        principal: Option<&'a CurrentPrincipal>,
        action: ViewAction,
        path_id: Option<Uuid>,
        headers: &'a HeaderMap,
        mut data: Value,
    ) -> MutationFuture<'a, Option<Response>> {
        Box::pin(async move {
            let conditional = [
                "x-sync-replica",
                "idempotency-key",
                "if-match",
                "x-sync-protocol",
                "x-sync-version",
                "x-sync-scope",
                "x-sync-entity",
            ]
            .iter()
            .any(|name| headers.contains_key(*name));
            let header = |name: &str| -> AppResult<&str> {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| AppError::BadRequest(format!("missing or invalid {name}")))
            };
            let replica: Uuid = if conditional {
                header("x-sync-replica")?
                    .parse()
                    .map_err(|_| AppError::BadRequest("invalid replica UUID".into()))?
            } else {
                Uuid::nil()
            };
            let operation: Uuid = if conditional {
                header("idempotency-key")?
                    .parse()
                    .map_err(|_| AppError::BadRequest("invalid operation UUID".into()))?
            } else {
                Uuid::now_v7()
            };
            let version: u32 = if conditional {
                header("x-sync-version")?
                    .parse()
                    .map_err(|_| AppError::BadRequest("invalid sync version".into()))?
            } else {
                V::Model::SYNC_VERSION
            };
            let expected_protocol = if self.model_logs {
                che_sync::protocol::model_log::MODEL_LOG_PROTOCOL
            } else {
                PROTOCOL_VERSION
            };
            if conditional && header("x-sync-protocol")? != expected_protocol.to_string() {
                return Err(AppError::BadRequest("incompatible sync protocol".into()));
            }
            if version != V::Model::SYNC_VERSION {
                return Err(AppError::BadRequest("incompatible model version".into()));
            }
            let revision: i64 = if conditional {
                header("if-match")?
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v >= 0)
                    .ok_or_else(|| {
                        AppError::BadRequest(
                            "If-Match must be a quoted nonnegative revision".into(),
                        )
                    })?
            } else {
                0
            };
            let mut id = match path_id {
                Some(id) => id,
                None if conditional => header("x-sync-entity")?
                    .parse()
                    .map_err(|_| AppError::BadRequest("invalid offline entity UUID".into()))?,
                None => Uuid::now_v7(),
            };
            if action == ViewAction::Create && revision != 0 {
                return Err(AppError::BadRequest("create requires revision zero".into()));
            }
            let principal = principal
                .ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
            V::Permission::default().check(state, Some(principal), action)?;
            let scope = if self.model_logs {
                che_sync::model_log::MODEL_LOG_SCOPE.into()
            } else {
                self.viewset.sync_scope(state, principal)?
            };
            if conditional && !self.model_logs && header("x-sync-scope")? != scope {
                return Err(AppError::BadRequest("sync scope mismatch".into()));
            }
            let server = server_for(&self.viewset, state, Some(principal), self.model_logs).await?;
            let json_null_fields: Vec<String> =
                if self.model_logs {
                    match headers.get("x-sync-json-null-fields") {
                        Some(value) => serde_json::from_str(value.to_str().map_err(|_| {
                            AppError::BadRequest("invalid JSON null fields".into())
                        })?)
                        .map_err(|_| AppError::BadRequest("invalid JSON null fields".into()))?,
                        None => vec![],
                    }
                } else {
                    vec![]
                };
            let identity_body = if self.model_logs {
                json!({"body": &data, "json_null_fields": &json_null_fields})
            } else {
                data.clone()
            };
            let identity = serde_json::to_string(&(
                V::Model::SYNC_NAME,
                format!("{action:?}"),
                id,
                revision,
                version,
                &identity_body,
            ))?;
            if let Some(result) = server
                .rest_receipt(replica, operation, &identity)
                .await
                .map_err(sync_error)?
            {
                retry_effects(state, principal, &server, &scope).await;
                return Ok(Some(write_response(
                    &self.viewset,
                    result,
                    action,
                    self.model_logs,
                )?));
            }
            let outcome = async {
                let mut request = PushRequest {
                    protocol_version: PROTOCOL_VERSION,
                    scope: scope.clone(),
                    replica_id: replica,
                    operation: Operation {
                        id: operation,
                        mutations: vec![Mutation {
                            model: V::Model::SYNC_NAME.into(),
                            id,
                            schema_version: version,
                            base_revision: revision,
                            changes: if action == ViewAction::Delete {
                                None
                            } else {
                                Some(Record::new())
                            },
                        }],
                    },
                };
                let before = server.prepare_push(&request).await.map_err(sync_error)?;
                let current = before
                    .iter()
                    .find(|entity| entity.model == V::Model::SYNC_NAME && entity.id == id)
                    .expect("prepared root");
                let revision = if conditional {
                    revision
                } else if action == ViewAction::Create {
                    0
                } else {
                    current.revision
                };
                request.operation.mutations[0].base_revision = revision;
                let item = if action != ViewAction::Create && current.record.is_some() {
                    let item = self
                        .viewset
                        .get_queryset()
                        .filter(V::Model::primary_key().eq(id))
                        .first(state.database())
                        .await?
                        .ok_or(AppError::NotFound)?;
                    V::Permission::default().check_object(
                        state,
                        Some(principal),
                        action,
                        V::QuerySet::item_model(&item),
                    )?;
                    Some(materialize::<V::Model>(current)?)
                } else {
                    None
                };
                if current.revision != revision
                    || (action != ViewAction::Create && current.record.is_none())
                {
                    if !conditional {
                        return Err(AppError::NotFound);
                    }
                    return Ok(Some(write_response(
                        &self.viewset,
                        PushResult::Conflict(vec![current.clone()]),
                        action,
                        self.model_logs,
                    )?));
                }
                if action != ViewAction::Delete {
                    let mut json_null_mappings = Vec::new();
                    for name in &json_null_fields {
                        let field = V::Serializer::fields()
                            .iter()
                            .find(|field| field.name == name && !field.read_only)
                            .ok_or_else(|| {
                                AppError::BadRequest("unknown writable JSON null field".into())
                            })?;
                        let schema = V::Model::schema();
                        let column = schema
                            .columns
                            .iter()
                            .find(|column| column.field_name == field.source)
                            .filter(|column| V::Model::json_columns().contains(&column.name))
                            .ok_or_else(|| {
                                AppError::BadRequest(
                                    "JSON null marker requires a JSON field".into(),
                                )
                            })?;
                        if data.get(name).is_none_or(|value| !value.is_null())
                            || json_null_mappings
                                .iter()
                                .any(|(existing, _)| existing == name)
                        {
                            return Err(AppError::BadRequest(
                                "JSON null marker must name an explicit null exactly once".into(),
                            ));
                        }
                        let wire = V::Model::registered_fields()
                            .into_iter()
                            .find(|mapping| mapping.column == column.name && mapping.writable)
                            .ok_or_else(|| {
                                AppError::BadRequest("JSON null field is server-owned".into())
                            })?;
                        json_null_mappings.push((name.clone(), wire.name));
                    }
                    crate::rest::router::normalize_datetime_fields(
                        &mut data,
                        V::Serializer::fields(),
                    );
                    // Sync JSON uses decimal strings for i64; turn them into exact
                    // serde_json integers before invoking the ordinary serializer.
                    if conditional && let Some(values) = data.as_object_mut() {
                        let schema = V::Model::schema();
                        for field in V::Serializer::fields() {
                            let column = schema.columns.iter().find(|column| {
                                if field.related_model.is_some() {
                                    column.name == field.source
                                } else {
                                    column.field_name == field.source
                                }
                            });
                            if column.is_some_and(|column| {
                                column.column_type == che_orm::ColumnType::Integer
                            }) && let Some(Value::String(decimal)) = values.get(field.name)
                            {
                                let integer = decimal.parse::<i64>().map_err(|_| {
                                    AppError::BadRequest(format!("invalid i64 for {}", field.name))
                                })?;
                                values.insert(field.name.into(), json!(integer));
                            }
                        }
                    }
                    let mode = match action {
                        ViewAction::Create => WriteMode::Create,
                        ViewAction::Update => WriteMode::Update { id },
                        _ => WriteMode::Patch { id },
                    };
                    let write = V::Serializer::is_valid(data, mode)
                        .map_err(crate::rest::router::error_validation)?;
                    let write = match action {
                        ViewAction::Create => {
                            match self.viewset.prepare_create(state, Some(principal), write)? {
                                ValidatedWrite::Create(query) => {
                                    ValidatedWrite::Create(if conditional {
                                        query.set(V::Model::primary_key(), id)
                                    } else {
                                        query
                                    })
                                }
                                _ => {
                                    return Err(AppError::BadRequest(
                                        "create preparation changed write kind".into(),
                                    ));
                                }
                            }
                        }
                        ViewAction::Update => self.viewset.prepare_update(
                            state,
                            Some(principal),
                            item.as_ref().expect("current item"),
                            write,
                        )?,
                        _ => self.viewset.prepare_patch(
                            state,
                            Some(principal),
                            item.as_ref().expect("current item"),
                            write,
                        )?,
                    };
                    let mut prepared = if !conditional && action == ViewAction::Create {
                        server.prepare_write(write).await
                    } else {
                        server.prepare_rest_write(write, revision).await
                    }
                    .map_err(sync_error)?;
                    for (_, wire) in json_null_mappings {
                        prepared.operation.mutations[0]
                            .changes
                            .as_mut()
                            .ok_or_else(|| AppError::BadRequest("missing JSON write".into()))?
                            .insert(wire.into(), che_sync::protocol::Value::Text("null".into()));
                    }
                    if !conditional && action == ViewAction::Create {
                        id = prepared.operation.mutations[0].id;
                        request.operation.mutations[0].id = id;
                    } else if prepared.operation.mutations[0].id != id {
                        return Err(AppError::BadRequest(
                            "preparation changed entity identity".into(),
                        ));
                    }
                    request.operation.mutations[0].changes =
                        prepared.operation.mutations[0].changes.clone();
                }
                let result = commit_rest(
                    state,
                    &self.viewset,
                    principal,
                    &server,
                    request,
                    identity.clone(),
                    action,
                )
                .await?;
                if conditional {
                    return Ok(Some(write_response(
                        &self.viewset,
                        result,
                        action,
                        self.model_logs,
                    )?));
                }
                accepted(result)?;
                if action == ViewAction::Delete {
                    return Ok(Some(StatusCode::NO_CONTENT.into_response()));
                }
                let item = self
                    .viewset
                    .get_queryset()
                    .filter(V::Model::primary_key().eq(id))
                    .first(state.database())
                    .await?
                    .ok_or(AppError::NotFound)?;
                let payload = V::Serializer::to_json(item)?;
                Ok(Some(
                    (
                        if action == ViewAction::Create {
                            StatusCode::CREATED
                        } else {
                            StatusCode::OK
                        },
                        Json(payload),
                    )
                        .into_response(),
                ))
            }
            .await;
            // A concurrent request may have committed while validation, object
            // loading or preparation was running. Resolve that receipt before
            // returning an early error/conflict instead of the canonical replay.
            if conditional
                && (outcome.is_err()
                    || outcome
                        .as_ref()
                        .ok()
                        .and_then(|response| response.as_ref())
                        .is_some_and(|response| {
                            response.status() == StatusCode::PRECONDITION_FAILED
                        }))
                && let Some(result) = server
                    .rest_receipt(replica, operation, &identity)
                    .await
                    .map_err(sync_error)?
            {
                retry_effects(state, principal, &server, &scope).await;
                return Ok(Some(write_response(
                    &self.viewset,
                    result,
                    action,
                    self.model_logs,
                )?));
            }
            if conditional
                && self.model_logs
                && let Err(AppError::BadRequest(reason)) = &outcome
            {
                let request = PushRequest {
                    protocol_version: PROTOCOL_VERSION,
                    scope,
                    replica_id: replica,
                    operation: Operation {
                        id: operation,
                        mutations: vec![Mutation {
                            model: V::Model::SYNC_NAME.into(),
                            id,
                            schema_version: version,
                            base_revision: revision,
                            changes: Some(Record::new()),
                        }],
                    },
                };
                let result = server
                    .reject_rest(request, identity, reason.clone())
                    .await
                    .map_err(sync_error)?;
                return Ok(Some(write_response(&self.viewset, result, action, true)?));
            }
            outcome
        })
    }
    fn save<'a>(
        &'a self,
        state: &'a AppState,
        principal: Option<&'a CurrentPrincipal>,
        write: crate::ValidatedWrite<V::Model>,
    ) -> MutationFuture<'a, Option<V::Model>> {
        Box::pin(async move {
            let server = server(&self.viewset, state, principal).await?;
            let entities = accepted(server.write(write).await.map_err(sync_error)?)?;
            let entity = entities
                .iter()
                .find(|e| e.model == <V::Model as SyncModel>::SYNC_NAME)
                .ok_or_else(|| AppError::BadRequest("missing canonical write result".into()))?;
            // Decode the canonical receipt, without racing a later database write.
            Ok(Some(materialize::<V::Model>(entity)?))
        })
    }

    fn delete<'a>(
        &'a self,
        state: &'a AppState,
        principal: Option<&'a CurrentPrincipal>,
        id: Uuid,
    ) -> MutationFuture<'a, ()> {
        Box::pin(async move {
            let server = server(&self.viewset, state, principal).await?;
            accepted(server.delete::<V::Model>(id).await.map_err(sync_error)?)?;
            Ok(())
        })
    }
}

fn rest_response(result: PushResult, action: ViewAction, model: &str) -> Response {
    let status = match &result {
        PushResult::Accepted(_) if action == ViewAction::Create => StatusCode::CREATED,
        PushResult::Accepted(_) => StatusCode::OK,
        PushResult::Conflict(_) => StatusCode::PRECONDITION_FAILED,
        PushResult::Rejected { .. } => StatusCode::BAD_REQUEST,
    };
    let revision = match &result {
        PushResult::Accepted(entities) => entities
            .iter()
            .find(|e| e.model == model)
            .map(|e| e.revision),
        _ => None,
    };
    let mut response = (status, Json(result)).into_response();
    response.headers_mut().insert(
        "content-type",
        "application/vnd.che-sync+json".parse().unwrap(),
    );
    if let Some(revision) = revision {
        response
            .headers_mut()
            .insert("etag", format!("\"{revision}\"").parse().unwrap());
    }
    response
}

pub(crate) fn add_rest_openapi(document: &mut Value) {
    if let Some(paths) = document["paths"].as_object_mut() {
        for item in paths.values_mut() {
            for method in ["post", "patch", "put", "delete"] {
                let Some(operation) = item.get_mut(method) else {
                    continue;
                };
                let parameters = operation
                    .as_object_mut()
                    .unwrap()
                    .entry("parameters")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap();
                for name in [
                    "X-Sync-Protocol",
                    "X-Sync-Replica",
                    "X-Sync-Scope",
                    "X-Sync-Version",
                    "If-Match",
                    "Idempotency-Key",
                ] {
                    parameters.push(json!({"in":"header","name":name,"required":false,"description":"Required together for a conditional, idempotent sync write; body uses the ordinary serializer.","schema":{"type":"string"}}));
                }
                if method == "post" {
                    parameters.push(json!({"in":"header","name":"X-Sync-Entity","required":false,"description":"Offline UUID for conditional sync creation.","schema":{"type":"string","format":"uuid"}}));
                }
                let responses = operation["responses"].as_object_mut().unwrap();
                for status in [if method == "post" { "201" } else { "200" }, "412", "400"] {
                    let response = responses.entry(status).or_insert_with(
                        || json!({"description":"Conditional synchronization result"}),
                    );
                    response
                        .as_object_mut()
                        .unwrap()
                        .entry("content")
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .unwrap()
                        .insert(
                            "application/vnd.che-sync+json".into(),
                            json!({"schema":PushResult::api_schema().openapi}),
                        );
                }
            }
        }
    }
}

pub(crate) fn add_model_rest_openapi(document: &mut Value) {
    add_rest_openapi(document);
    if let Some(paths) = document["paths"].as_object_mut() {
        for item in paths.values_mut() {
            for method in ["post", "put", "patch", "delete"] {
                let Some(operation) = item.get_mut(method) else {
                    continue;
                };
                if let Some(parameters) = operation["parameters"].as_array_mut() {
                    parameters.retain(|parameter| parameter["name"] != "X-Sync-Scope");
                    for parameter in parameters {
                        if parameter["name"] == "X-Sync-Protocol" {
                            parameter["schema"]["enum"] = json!(["2"]);
                        }
                    }
                }
                if let Some(responses) = operation["responses"].as_object_mut() {
                    for response in responses.values_mut() {
                        if let Some(content) = response["content"].as_object_mut() {
                            content.remove("application/vnd.che-sync+json");
                            content.insert(
                                "application/vnd.che-sync.v2+json".into(),
                                json!({"schema": model_write_schema()}),
                            );
                        }
                    }
                }
            }
        }
    }
}

fn model_write_schema() -> Value {
    json!({"oneOf":[
        {"type":"object","required":["status","data"],"properties":{"status":{"type":"string","enum":["accepted","conflict"]},"data":{"type":"array","items":{"type":"object","required":["model","record_id","revision","schema_version","values"],"properties":{"model":{"type":"string"},"record_id":{"type":"string","format":"uuid"},"revision":{"type":"string"},"schema_version":{"type":"integer"},"values":{"type":"object","nullable":true,"additionalProperties":true}}}}}},
        {"type":"object","required":["status","data"],"properties":{"status":{"type":"string","enum":["rejected"]},"data":{"type":"object","required":["code","reason"],"properties":{"code":{"type":"string"},"reason":{"type":"string"}}}}}
    ]})
}

impl ApiSchema for che_sync::protocol::model_log::ModelChanges {
    fn api_schema() -> ApiSchemaDefinition {
        ApiSchemaDefinition {
            name: "ModelChanges",
            openapi: json!({"type":"object","required":["model","history","operations","next_cursor","has_more"],"properties":{
                "model":{"type":"string"},"history":{"type":"string","format":"uuid"},"next_cursor":{"type":"string"},"has_more":{"type":"boolean"},
                "operations":{"type":"array","items":{"type":"object","required":["id","record_id","revision","kind","diff","created_at","schema_version"],"properties":{
                    "id":{"type":"string"},"record_id":{"type":"string","format":"uuid"},"revision":{"type":"string"},"kind":{"type":"string","enum":["create","update","delete"]},"created_at":{"type":"string","format":"date-time"},"schema_version":{"type":"integer"},
                    "diff":{"type":"array","items":{"type":"object","required":["field"],"properties":{"field":{"type":"string"},"old_value":{},"new_value":{},"old_sql_null":{"type":"boolean"},"new_sql_null":{"type":"boolean"}}}}
                }}}
            }}),
            typescript: concat!(
                "export interface ModelFieldDiff { field: string; old_value?: JsonValue; new_value?: JsonValue; old_sql_null?: boolean; new_sql_null?: boolean; }\n",
                "export interface ModelLogOperation { id: string; record_id: string; revision: string; kind: 'create' | 'update' | 'delete'; diff: ModelFieldDiff[]; created_at: string; schema_version: number; }\n",
                "export interface ModelChanges { model: string; history: string; operations: ModelLogOperation[]; next_cursor: string; has_more: boolean; }"
            ),
        }
    }
}

fn database_value(value: &che_sync::protocol::Value) -> che_orm::DatabaseValue {
    use che_orm::DatabaseValue as D;
    use che_sync::protocol::Value as V;
    match value {
        V::Integer(v) => D::Integer(*v),
        V::Real(v) => D::Real(*v),
        V::Text(v) => D::Text(v.clone()),
        V::Binary(v) => D::Binary(v.clone()),
        V::Boolean(v) => D::Boolean(*v),
        V::DateTime(v) => D::DateTime(*v),
        V::Uuid(v) => D::Uuid(*v),
        V::Null => D::Null,
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object", "properties":properties, "required":required})
}
fn cursor_schema() -> Value {
    object(
        json!({"scope":{"type":"string"},"model":{"type":"string"},"epoch":{"type":"string","format":"uuid"},
        "sequence":{"type":"string","pattern":"^[0-9]+$"}}),
        &["scope", "epoch", "sequence"],
    )
}
fn record_schema() -> Value {
    let variant = |tag: &str, value: Value| {
        object(
            json!({"type":{"type":"string","enum":[tag]},"value":value}),
            &["type", "value"],
        )
    };
    json!({"type":"object","additionalProperties":{"oneOf":[
        variant("integer",json!({"type":"string","pattern":"^-?[0-9]+$"})),
        variant("real",json!({"type":"number"})), variant("text",json!({"type":"string"})),
        variant("binary",json!({"type":"array","items":{"type":"integer","minimum":0,"maximum":255}})),
        variant("boolean",json!({"type":"boolean"})),
        variant("date_time",json!({"type":"string"})),
        variant("uuid",json!({"type":"string","format":"uuid"})),
        object(json!({"type":{"type":"string","enum":["null"]}}), &["type"])
    ]}})
}
fn entity_schema() -> Value {
    let mut record = record_schema();
    record["nullable"] = json!(true);
    object(
        json!({"model":{"type":"string"},"id":{"type":"string","format":"uuid"},
        "schema_version":{"type":"integer","minimum":1},"revision":{"type":"string","pattern":"^[0-9]+$"},
        "epoch":{"type":"string","format":"uuid","nullable":true},
        "updated_at":{"type":"string","format":"date-time","nullable":true},
        "deleted_at":{"type":"string","format":"date-time","nullable":true},
        "record":record}),
        &["model", "id", "schema_version", "revision", "record"],
    )
}
fn entities_schema() -> Value {
    json!({"type":"array","items":entity_schema()})
}

impl ApiSchema for PushRequest {
    fn api_schema() -> ApiSchemaDefinition {
        let mut changes = record_schema();
        changes["nullable"] = json!(true);
        let mutation = object(
            json!({"model":{"type":"string"},"id":{"type":"string","format":"uuid"},
            "schema_version":{"type":"integer","minimum":1},"base_revision":{"type":"string","pattern":"^[0-9]+$"},
            "changes":changes}),
            &["model", "id", "schema_version", "base_revision", "changes"],
        );
        ApiSchemaDefinition {
            name: "SyncPushRequest",
            openapi: object(
                json!({
                    "protocol_version":{"type":"integer","enum":[PROTOCOL_VERSION]},"scope":{"type":"string"},
                    "replica_id":{"type":"string","format":"uuid"},"operation":object(json!({"id":{"type":"string","format":"uuid"},
                        "mutations":{"type":"array","minItems":1,"items":mutation}}), &["id","mutations"])
                }),
                &["protocol_version", "scope", "replica_id", "operation"],
            ),
            typescript: concat!(
                "export type SyncValue = { type: 'integer'; value: string } | { type: 'real'; value: number } | { type: 'text' | 'uuid' | 'date_time'; value: string } | { type: 'binary'; value: number[] } | { type: 'boolean'; value: boolean } | { type: 'null' };\n",
                "export type SyncRecord = Record<string, SyncValue>;\n",
                "export interface SyncEntity { model: string; id: string; schema_version: number; revision: string; epoch?: string | null; updated_at?: string | null; deleted_at?: string | null; record: SyncRecord | null; }\n",
                "export interface SyncCursor { scope: string; model?: string; epoch: string; sequence: string; }\n",
                "export interface SyncChangeGroup { sequence: string; entities: SyncEntity[]; }\n",
                "export interface SyncPushRequest { protocol_version: number; scope: string; replica_id: string; operation: { id: string; mutations: { model: string; id: string; schema_version: number; base_revision: string; changes: SyncRecord | null; }[] }; }"
            ),
        }
    }
}
impl ApiSchema for PushResult {
    fn api_schema() -> ApiSchemaDefinition {
        ApiSchemaDefinition {
            name: "SyncPushResult",
            openapi: json!({"oneOf":[
                object(json!({"status":{"type":"string","enum":["accepted","conflict"]},"data":entities_schema()}), &["status","data"]),
                object(json!({"status":{"type":"string","enum":["rejected"]},"data":object(json!({"reason":{"type":"string"}}), &["reason"])}), &["status","data"])
            ]}),
            typescript: "export type SyncPushResult = { status: 'accepted' | 'conflict'; data: SyncEntity[] } | { status: 'rejected'; data: { reason: string } };",
        }
    }
}
impl ApiSchema for PullRequest {
    fn api_schema() -> ApiSchemaDefinition {
        let mut cursor = cursor_schema();
        cursor["nullable"] = json!(true);
        ApiSchemaDefinition {
            name: "SyncPullRequest",
            openapi: object(
                json!({"protocol_version":{"type":"integer","enum":[PROTOCOL_VERSION]},
            "cursor":cursor,"limit":{"type":"integer","minimum":1,"maximum":1000}}),
                &["protocol_version", "cursor", "limit"],
            ),
            typescript: concat!(
                "export type SyncValue = { type: 'integer'; value: string } | { type: 'real'; value: number } | { type: 'text' | 'uuid' | 'date_time'; value: string } | { type: 'binary'; value: number[] } | { type: 'boolean'; value: boolean } | { type: 'null' };\n",
                "export type SyncRecord = Record<string, SyncValue>;\n",
                "export interface SyncEntity { model: string; id: string; schema_version: number; revision: string; epoch?: string | null; updated_at?: string | null; deleted_at?: string | null; record: SyncRecord | null; }\n",
                "export interface SyncCursor { scope: string; model?: string; epoch: string; sequence: string; }\n",
                "export interface SyncChangeGroup { sequence: string; entities: SyncEntity[]; }\n",
                "export type SyncRestResult = { status: 'accepted' | 'conflict'; data: SyncEntity[] } | { status: 'rejected'; data: { reason: string } };\n",
                "export interface SyncPullRequest { protocol_version: number; cursor: SyncCursor | null; limit: number; }"
            ),
        }
    }
}
impl ApiSchema for PullResult {
    fn api_schema() -> ApiSchemaDefinition {
        ApiSchemaDefinition {
            name: "SyncPullResult",
            openapi: json!({"oneOf":[
                object(json!({"status":{"type":"string","enum":["full"]},"data":object(json!({"entities":entities_schema(),"cursor":cursor_schema()}), &["entities","cursor"])}), &["status","data"]),
                object(json!({"status":{"type":"string","enum":["changes"]},"data":object(json!({
                    "groups":{"type":"array","items":object(json!({"sequence":{"type":"string","pattern":"^[0-9]+$"},"entities":entities_schema()}), &["sequence","entities"])},
                    "cursor":cursor_schema(),"has_more":{"type":"boolean"}}), &["groups","cursor","has_more"])}), &["status","data"]),
                object(json!({"status":{"type":"string","enum":["reset_required"]}}), &["status"])
            ]}),
            typescript: "export type SyncPullResult = { status: 'full'; data: { entities: SyncEntity[]; cursor: SyncCursor } } | { status: 'changes'; data: { groups: SyncChangeGroup[]; cursor: SyncCursor; has_more: boolean } } | { status: 'reset_required' };",
        }
    }
}
impl ApiSchema for Snapshot {
    fn api_schema() -> ApiSchemaDefinition {
        ApiSchemaDefinition {
            name: "SyncSnapshot",
            openapi: object(
                json!({"entities":entities_schema(),"cursor":cursor_schema()}),
                &["entities", "cursor"],
            ),
            typescript: "export interface SyncSnapshot { entities: SyncEntity[]; cursor: SyncCursor; }",
        }
    }
}
