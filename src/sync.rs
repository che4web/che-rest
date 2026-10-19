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
    ) -> MutationFuture<'a, Vec<Value>>;
    fn committed(
        &self,
        state: &AppState,
        principal: Option<&CurrentPrincipal>,
        old: Option<&Entity>,
        entity: &Entity,
        payload: Option<&[Value]>,
        action_id: &str,
    ) -> AppResult<()>;
}

struct Lifecycle<V: SyncViewSet>
where
    V::Model: SyncModel,
{
    viewset: V,
    hooks: Vec<Arc<dyn MutationHook<V>>>,
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
        self.viewset.sync_scope(state, principal)
    }
    fn actions(&self) -> Vec<String> {
        self.hooks
            .iter()
            .filter(|hook| hook.backend().is_none())
            .map(|hook| hook.id().to_owned())
            .chain(std::iter::once("@signal".into()))
            .collect()
    }
    fn prepare_delete<'a>(
        &'a self,
        state: &'a AppState,
        entity: &'a Entity,
    ) -> MutationFuture<'a, Vec<Value>> {
        Box::pin(async move {
            let model = materialize::<V::Model>(entity)?;
            let mut values = Vec::new();
            for hook in &self.hooks {
                values.push(hook.prepare_delete_async(state, &model).await?);
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
        payload: Option<&[Value]>,
        action_id: &str,
    ) -> AppResult<()> {
        let action = if entity.record.is_none() {
            if let Some(payload) = payload {
                for (hook, value) in self.hooks.iter().zip(payload) {
                    if hook.id() == action_id {
                        hook.after_delete(state, value)?;
                    }
                }
            }
            crate::ViewAction::Delete
        } else {
            let model = materialize::<V::Model>(entity)?;
            let action = if old.is_none_or(|old| old.revision == 0) {
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
    fn sync_scope(&self, state: &AppState, principal: &CurrentPrincipal) -> AppResult<String>;
    fn sync_policy(&self) -> Arc<dyn AccessPolicy>;
}

/// Install in `ViewSet::configure`. Also replaces CRUD persistence so changes
/// and the sync journal commit atomically. Sync tables must already be migrated.
#[derive(Clone)]
pub struct SyncEndpoints<V> {
    viewset: V,
    body_limit: usize,
}

impl<V> SyncEndpoints<V> {
    pub fn new(viewset: V) -> Self {
        Self {
            viewset,
            body_limit: 2 * 1024 * 1024,
        }
    }

    pub fn body_limit(mut self, bytes: usize) -> Self {
        self.body_limit = bytes;
        self
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
    let principal =
        principal.ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
    let scope = viewset.sync_scope(state, principal)?;
    let identity = principal.auth_user().id.to_string();
    let policy = viewset.sync_policy();
    policy
        .authorize_scope(&identity, &scope)
        .map_err(|error| match error {
            OrmError::Configuration(reason) => AppError::Forbidden(reason),
            other => AppError::Orm(other),
        })?;
    SyncServer::open(
        state.database().clone(),
        viewset.sync_registry()?,
        scope,
        identity,
        policy,
    )
    .await
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
        let prefix = self.viewset.path().trim_matches('/').replace('/', "_");
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
                for action in lifecycle.actions() {
                    effects.push(che_sync::PreparedEffect {
                        model: entity.model.clone(),
                        entity_id: entity.id,
                        handler: lifecycle.path().into(),
                        action,
                        payload: serde_json::to_string(&values)?,
                    });
                }
            }
        }
    }
    let result = server
        .write_rest_prepared(request, before.clone(), effects, identity)
        .await
        .map_err(sync_error)?;
    flush_effects(state, principal, server, &scope).await?;
    Ok(result)
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
            if lifecycle.scope(state, principal)? != scope {
                continue;
            }
            let values: Option<Vec<Value>> = serde_json::from_str(&effect.prepared.payload)?;
            match lifecycle.committed(
                state,
                Some(principal),
                effect.previous.as_ref(),
                &effect.entity,
                values.as_deref(),
                &effect.prepared.action,
            ) {
                Ok(()) => server.acknowledge_effect(effect.id).await?,
                Err(error) => eprintln!("sync effect {} deferred: {error}", effect.id),
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
            if !headers.contains_key("x-sync-replica")
                && !headers.contains_key("idempotency-key")
                && !headers.contains_key("if-match")
            {
                return Ok(None);
            }
            let header = |name: &str| -> AppResult<&str> {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| AppError::BadRequest(format!("missing or invalid {name}")))
            };
            let replica: Uuid = header("x-sync-replica")?
                .parse()
                .map_err(|_| AppError::BadRequest("invalid replica UUID".into()))?;
            let operation: Uuid = header("idempotency-key")?
                .parse()
                .map_err(|_| AppError::BadRequest("invalid operation UUID".into()))?;
            let version: u32 = header("x-sync-version")?
                .parse()
                .map_err(|_| AppError::BadRequest("invalid sync version".into()))?;
            if header("x-sync-protocol")? != PROTOCOL_VERSION.to_string() {
                return Err(AppError::BadRequest("incompatible sync protocol".into()));
            }
            if version != V::Model::SYNC_VERSION {
                return Err(AppError::BadRequest("incompatible model version".into()));
            }
            let revision = header("if-match")?;
            let revision: i64 = revision
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .and_then(|v| v.parse().ok())
                .filter(|v| *v >= 0)
                .ok_or_else(|| {
                    AppError::BadRequest("If-Match must be a quoted nonnegative revision".into())
                })?;
            let id = match path_id {
                Some(id) => id,
                None => header("x-sync-entity")?
                    .parse()
                    .map_err(|_| AppError::BadRequest("invalid offline entity UUID".into()))?,
            };
            if action == ViewAction::Create && revision != 0 {
                return Err(AppError::BadRequest("create requires revision zero".into()));
            }
            let principal = principal
                .ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
            V::Permission::default().check(state, Some(principal), action)?;
            let scope = self.viewset.sync_scope(state, principal)?;
            if header("x-sync-scope")? != scope {
                return Err(AppError::BadRequest("sync scope mismatch".into()));
            }
            let server = server(&self.viewset, state, Some(principal)).await?;
            let identity = serde_json::to_string(&(
                V::Model::SYNC_NAME,
                format!("{action:?}"),
                id,
                revision,
                version,
                &data,
            ))?;
            if let Some(result) = server
                .rest_receipt(replica, operation, &identity)
                .await
                .map_err(sync_error)?
            {
                flush_effects(state, principal, &server, &scope).await?;
                return Ok(Some(rest_response(result, action, V::Model::SYNC_NAME)));
            }
            let mut request = PushRequest {
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
                return Ok(Some(rest_response(
                    PushResult::Conflict(vec![current.clone()]),
                    action,
                    V::Model::SYNC_NAME,
                )));
            }
            if action != ViewAction::Delete {
                crate::rest::router::normalize_datetime_fields(&mut data, V::Serializer::fields());
                // Sync JSON uses decimal strings for i64; turn them into exact
                // serde_json integers before invoking the ordinary serializer.
                if let Some(values) = data.as_object_mut() {
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
                                ValidatedWrite::Create(query.set(V::Model::primary_key(), id))
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
                let prepared = server.prepare_write(write).await.map_err(sync_error)?;
                if prepared.operation.mutations[0].id != id {
                    return Err(AppError::BadRequest(
                        "preparation changed entity identity".into(),
                    ));
                }
                request.operation.mutations[0].changes =
                    prepared.operation.mutations[0].changes.clone();
            }
            let result =
                commit_rest(state, &self.viewset, principal, &server, request, identity).await?;
            Ok(Some(rest_response(result, action, V::Model::SYNC_NAME)))
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
