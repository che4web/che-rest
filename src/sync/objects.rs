//! HTTP/principal bridge for explicit object-delivery protocol 3.
use super::*;
use crate::auth::access::{ADD, Access, AccessModel, CHANGE, DELETE, VIEW};
use crate::rest::permissions::PermissionContext;
use che_orm::{Expr, Transaction, TransactionFuture};
use che_sync::object_log::*;
use std::marker::PhantomData;

pub(super) type ObjectPolicyFactory = Arc<
    dyn Fn(
            AppState,
            CurrentPrincipal,
        ) -> AppResult<(Arc<dyn AccessPolicy>, Arc<dyn ObjectAccessPolicy>)>
        + Send
        + Sync,
>;

/// Opt-in protocol 3. ACLs and business writes share a transaction; GET journal
/// v2 is deliberately absent. Apply explicit object-delivery follow-up migrations.
#[derive(Clone)]
pub struct ObjectSyncEndpoints<V>(SyncEndpoints<V>);
impl<V: SyncViewSet> ObjectSyncEndpoints<V>
where
    V::Model: SyncModel + AccessModel,
{
    pub fn new(viewset: V) -> Self {
        let mut inner = SyncEndpoints::new(viewset.clone());
        inner.model_logs = true;
        inner.object_access = Some(Arc::new(move |state, principal| {
            let filter = viewset.get_queryset().object_sync_filter()?;
            let policy = Arc::new(ObjectPolicy::<V::Model> {
                state,
                principal,
                filter,
                namespace: viewset.path().into(),
                model: PhantomData,
            });
            Ok((policy.clone(), policy))
        }));
        Self(inner)
    }
}
impl<V: SyncViewSet> ViewSetExtension<V> for ObjectSyncEndpoints<V>
where
    V::Model: SyncModel + AccessModel,
{
    const ID: &'static str = "object-sync";
    fn install(self, context: &mut ExtensionContext<'_, V>) -> AppResult<()> {
        if std::any::TypeId::of::<V::Permission>() != std::any::TypeId::of::<Access<V::Model>>() {
            return Err(AppError::BadRequest(
                "object sync currently requires Permission = Access<Model>".into(),
            ));
        }
        che_sync::object_log::validate_registry(&self.0.viewset.sync_registry()?)?;
        self.0.viewset.get_queryset().object_sync_filter()?;
        let schema = V::Model::schema();
        for mapping in V::Model::registered_fields() {
            if [
                V::Model::REVISION_COLUMN,
                V::Model::UPDATED_AT_COLUMN,
                V::Model::DELETED_AT_COLUMN,
                V::Model::SCOPE_COLUMN,
            ]
            .contains(&mapping.column)
            {
                continue;
            }
            let column = schema
                .columns
                .iter()
                .find(|c| c.name == mapping.column)
                .expect("registered column");
            if !V::Serializer::fields()
                .iter()
                .any(|f| f.source == column.field_name && f.related_model.is_none())
            {
                return Err(AppError::BadRequest(format!(
                    "object sync discloses full scalar states; serializer must expose {}",
                    mapping.name
                )));
            }
        }

        self.0.install(context)
    }
}

struct ObjectPolicy<M> {
    state: AppState,
    principal: CurrentPrincipal,
    filter: Option<Expr>,
    namespace: String,
    model: PhantomData<fn() -> M>,
}
fn policy_error(error: AppError) -> OrmError {
    match error {
        AppError::Orm(e) => e,
        _ => OrmError::Configuration("object permission required".into()),
    }
}
impl<M: SyncModel + AccessModel> ObjectPolicy<M> {
    async fn in_queryset(&self, tx: &Transaction, id: Uuid) -> Result<bool, OrmError> {
        let mut q = M::query().filter(M::primary_key().eq(id));
        if let Some(f) = &self.filter {
            q = q.filter(f.clone());
        }
        Ok(tx.fetch_one(q).await?.is_some())
    }
    fn context<'a>(&'a self, tx: &'a Transaction) -> PermissionContext<'a> {
        PermissionContext::in_transaction(&self.state, tx)
    }
}
impl<M: SyncModel + AccessModel> ObjectAccessPolicy for ObjectPolicy<M> {
    fn feed_namespace(&self) -> &str {
        &self.namespace
    }
    fn rights<'a>(
        &'a self,
        tx: &'a Transaction,
        model: &'a str,
        id: Uuid,
    ) -> TransactionFuture<'a, ObjectRights> {
        Box::pin(async move {
            if model != M::SYNC_NAME {
                return Err(OrmError::Configuration("wrong object model".into()));
            }
            if !self.in_queryset(tx, id).await? {
                return Ok(ObjectRights::default());
            }
            let context = self.context(tx);
            Ok(ObjectRights {
                read: Access::<M>::can(context, &self.principal, VIEW, id)
                    .await
                    .map_err(policy_error)?,
                change: Access::<M>::can(context, &self.principal, CHANGE, id)
                    .await
                    .map_err(policy_error)?,
                delete: Access::<M>::can(context, &self.principal, DELETE, id)
                    .await
                    .map_err(policy_error)?,
            })
        })
    }
    fn can_create<'a>(
        &'a self,
        tx: &'a Transaction,
        model: &'a str,
    ) -> TransactionFuture<'a, bool> {
        Box::pin(async move {
            if model != M::SYNC_NAME {
                return Err(OrmError::Configuration("wrong object model".into()));
            }
            Ok(
                (M::SUPERUSER_BYPASS && self.principal.auth_user().is_superuser)
                    || Access::<M>::has_model_permission(
                        self.context(tx),
                        self.principal.auth_user().id,
                        ADD,
                    )
                    .await
                    .map_err(policy_error)?,
            )
        })
    }
}
impl<M: SyncModel + AccessModel> AccessPolicy for ObjectPolicy<M> {
    fn isolation(&self) -> che_orm::IsolationLevel {
        che_orm::IsolationLevel::Serializable
    }
    fn authorize_scope(&self, principal: &str, scope: &str) -> Result<(), OrmError> {
        if principal != self.principal.auth_user().id.to_string()
            || scope != che_sync::model_log::MODEL_LOG_SCOPE
        {
            return Err(OrmError::Configuration("object principal mismatch".into()));
        }
        Ok(())
    }
    fn validate(&self, _: &str, _: &str, _: &Entity, _: &Entity) -> Result<(), OrmError> {
        Ok(())
    }
    fn authorize_write<'a>(
        &'a self,
        tx: &'a Transaction,
        current: &'a Entity,
        mutation: &'a Mutation,
    ) -> TransactionFuture<'a, ()> {
        Box::pin(async move {
            if mutation.model != M::SYNC_NAME {
                return Err(OrmError::Configuration("wrong object model".into()));
            }
            let create = mutation.base_revision == 0 && mutation.changes.is_some();
            if create {
                if !self.can_create(tx, M::SYNC_NAME).await? {
                    return Err(OrmError::Configuration("add permission required".into()));
                }
            } else {
                if !self.in_queryset(tx, current.id).await? {
                    return Err(OrmError::Configuration("object permission required".into()));
                }
                Access::<M>::require(
                    self.context(tx),
                    &self.principal,
                    if mutation.changes.is_none() {
                        DELETE
                    } else {
                        CHANGE
                    },
                    current.id,
                )
                .await
                .map_err(policy_error)?;
            }
            Ok(())
        })
    }
    fn after_write<'a>(
        &'a self,
        tx: &'a Transaction,
        current: &'a Entity,
        next: &'a Entity,
    ) -> TransactionFuture<'a, ()> {
        Box::pin(async move {
            if current.revision == 0 && next.record.is_some() {
                for permission in M::CREATOR_PERMISSIONS {
                    Access::<M>::grant(
                        tx,
                        crate::auth::access::Subject::User(self.principal.auth_user().id),
                        permission,
                        next.id,
                    )
                    .await
                    .map_err(policy_error)?;
                }
            }
            Ok(())
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectPullRequest {
    protocol_version: u32,
    cursor: Option<ObjectCursor>,
    limit: u32,
}
fn object_cursor_schema() -> Value {
    object(
        json!({"model":{"type":"string"},"history":{"type":"string","format":"uuid"},"feed":{"type":"string","format":"uuid"},"sequence":{"type":"string","pattern":"^[0-9]+$"}}),
        &["model", "history", "feed", "sequence"],
    )
}
impl ApiSchema for ObjectPullRequest {
    fn api_schema() -> ApiSchemaDefinition {
        let mut cursor = object_cursor_schema();
        cursor["nullable"] = json!(true);
        ApiSchemaDefinition {
            name: "ObjectPullRequest",
            openapi: object(
                json!({"protocol_version":{"type":"integer","enum":[3]},"cursor":cursor,"limit":{"type":"integer","minimum":1,"maximum":1000}}),
                &["protocol_version", "cursor", "limit"],
            ),
            typescript: concat!(
                "export interface ObjectCursor { model:string; history:string; feed:string; sequence:string; }\n",
                "export interface ObjectPullRequest { protocol_version:3; cursor:ObjectCursor | null; limit:number; }"
            ),
        }
    }
}
impl ApiSchema for ObjectChanges {
    fn api_schema() -> ApiSchemaDefinition {
        let rights = object(
            json!({"read":{"type":"boolean"},"change":{"type":"boolean"},"delete":{"type":"boolean"}}),
            &["read", "change", "delete"],
        );
        let state = object(
            json!({"model":{"type":"string"},"record_id":{"type":"string","format":"uuid"},"revision":{"type":"string"},"schema_version":{"type":"integer"},"values":{"type":"object","additionalProperties":true},"sql_null_fields":{"type":"array","items":{"type":"string"}},"updated_at":{"type":"string","format":"date-time","nullable":true},"deleted_at":{"type":"string","format":"date-time","nullable":true}}),
            &["model", "record_id", "revision", "schema_version", "values"],
        );
        let changes = json!({"oneOf":[
            object(json!({"kind":{"type":"string","enum":["state"]},"state":state,"rights":rights}),&["kind","state","rights"]),
            object(json!({"kind":{"type":"string","enum":["revoked"]},"record_id":{"type":"string","format":"uuid"}}),&["kind","record_id"]),
            object(json!({"kind":{"type":"string","enum":["deleted"]},"record_id":{"type":"string","format":"uuid"},"revision":{"type":"string"}}),&["kind","record_id","revision"])
        ]});
        ApiSchemaDefinition {
            name: "ObjectChanges",
            openapi: object(
                json!({"protocol_version":{"type":"integer","enum":[3]},"cursor":object_cursor_schema(),"after":{"type":"string"},"events":{"type":"array","items":object(json!({"sequence":{"type":"string"},"change":changes}),&["sequence","change"])},"can_create":{"type":"boolean"},"has_more":{"type":"boolean"}}),
                &[
                    "protocol_version",
                    "cursor",
                    "after",
                    "events",
                    "can_create",
                    "has_more",
                ],
            ),
            typescript: concat!(
                "export interface ObjectRights { read:boolean; change:boolean; delete:boolean; }\n",
                "export interface ObjectModelState { model:string; record_id:string; revision:string; schema_version:number; values:Record<string,unknown> | null; sql_null_fields:string[]; updated_at:string | null; deleted_at:string | null; }\n",
                "export type ObjectChange = {kind:'state'; state:ObjectModelState; rights:ObjectRights} | {kind:'revoked'; record_id:string} | {kind:'deleted'; record_id:string; revision:string};\n",
                "export interface ObjectChanges { protocol_version:3; cursor:ObjectCursor; after:string; events:{sequence:string;change:ObjectChange}[]; can_create:boolean; has_more:boolean; }"
            ),
        }
    }
}
pub(super) fn install_route<V: SyncViewSet>(
    endpoints: &SyncEndpoints<V>,
    context: &mut ExtensionContext<'_, V>,
) -> AppResult<()>
where
    V::Model: SyncModel,
{
    let factory = endpoints.object_access.clone().expect("object factory");
    let prefix = endpoints.viewset.path().trim_matches('/').replace('/', "_");
    context.route("sync/objects", move |route| {
        route.post(
            move |Extension(state): Extension<AppState>,
                  Extension(viewset): Extension<V>,
                  who: Option<Extension<CurrentPrincipal>>,
                  Json(request): Json<ObjectPullRequest>| {
                let factory = factory.clone();
                async move {
                    if request.protocol_version != OBJECT_LOG_PROTOCOL {
                        return Err(AppError::BadRequest("incompatible object protocol".into()));
                    }
                    let principal = who
                        .ok_or_else(|| AppError::Unauthorized("authentication required".into()))?
                        .0;
                    let server = server_with_objects(
                        &viewset,
                        &state,
                        Some(&principal),
                        true,
                        Some(&factory),
                    )
                    .await?;
                    let policy = factory(state.clone(), principal)?.1;
                    Ok::<_, AppError>(Json(
                        server
                            .object_changes::<V::Model>(request.cursor, request.limit, policy)
                            .await
                            .map_err(sync_error)?,
                    ))
                }
            },
            OperationSpec::new(format!("{prefix}_sync_objects"))
                .authenticated()
                .request::<ObjectPullRequest>()
                .response::<ObjectChanges>(StatusCode::OK)
                .client("sync", "objects"),
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn response<V: SyncViewSet>(
    viewset: &V,
    result: PushResult,
    action: ViewAction,
    model_logs: bool,
    factory: Option<&ObjectPolicyFactory>,
    state: &AppState,
    principal: &CurrentPrincipal,
    server: &SyncServer,
    operation: Uuid,
) -> AppResult<Response>
where
    V::Model: SyncModel,
{
    let Some(factory) = factory else {
        return write_response(viewset, result, action, model_logs);
    };
    let result = server
        .object_write_result(
            operation,
            result,
            factory(state.clone(), principal.clone())?.1,
        )
        .await?;
    let outcome = match &result {
        ObjectWriteResult::Acknowledged { outcome, .. } => *outcome,
        ObjectWriteResult::Canonical(r) => match r {
            che_sync::model_log::ModelWriteResult::Accepted(_) => WriteOutcome::Accepted,
            che_sync::model_log::ModelWriteResult::Conflict(_) => WriteOutcome::Conflict,
            che_sync::model_log::ModelWriteResult::Rejected { .. } => WriteOutcome::Rejected,
        },
    };
    let status = match outcome {
        WriteOutcome::Accepted if action == ViewAction::Create => StatusCode::CREATED,
        WriteOutcome::Accepted => StatusCode::OK,
        WriteOutcome::Conflict => StatusCode::PRECONDITION_FAILED,
        WriteOutcome::Rejected => StatusCode::BAD_REQUEST,
    };
    let mut response = (status, Json(result)).into_response();
    response.headers_mut().insert(
        "content-type",
        "application/vnd.che-sync.v3+json".parse().unwrap(),
    );
    Ok(response)
}

/// Schema-only infrastructure module. Freeze the operations returned by
/// `che_sync::object_log::migration_operations(Server)` in a follow-up history.
pub fn object_schema_module() -> impl crate::AppModule {
    ObjectSchemaModule
}
struct ObjectSchemaModule;
impl crate::AppModule for ObjectSchemaModule {
    fn name(&self) -> &'static str {
        "che_object_sync"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        che_sync::object_log::schema(che_sync::SchemaRole::Server)
    }
    fn init(&self, _: &mut crate::ModuleContext) {}
}

pub(super) fn add_openapi(document: &mut Value) {
    add_model_rest_openapi(document);
    if let Some(paths) = document["paths"].as_object_mut() {
        for item in paths.values_mut() {
            for method in ["post", "put", "patch", "delete"] {
                let Some(operation) = item.get_mut(method) else {
                    continue;
                };
                if let Some(parameters) = operation["parameters"].as_array_mut() {
                    for parameter in parameters {
                        if parameter["name"] == "X-Sync-Protocol" {
                            parameter["schema"]["enum"] = json!(["3"]);
                        }
                    }
                }
                if let Some(responses) = operation["responses"].as_object_mut() {
                    for response in responses.values_mut() {
                        if let Some(content) = response["content"].as_object_mut() {
                            content.remove("application/vnd.che-sync.v2+json");
                            content.insert("application/vnd.che-sync.v3+json".into(),json!({"schema":{"oneOf":[object(json!({"kind":{"type":"string","enum":["canonical"]},"data":model_write_schema()}),&["kind","data"]),object(json!({"kind":{"type":"string","enum":["acknowledged"]},"data":object(json!({"operation_id":{"type":"string","format":"uuid"},"outcome":{"type":"string","enum":["accepted","conflict","rejected"]}}),&["operation_id","outcome"])}),&["kind","data"])]}}));
                        }
                    }
                }
            }
        }
    }
}

pub(super) async fn ordinary_response<V: SyncViewSet>(
    viewset: &V,
    state: &AppState,
    principal: &CurrentPrincipal,
    factory: &ObjectPolicyFactory,
    id: Uuid,
    action: ViewAction,
) -> AppResult<Response>
where
    V::Model: SyncModel,
{
    if action == ViewAction::Delete {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let policy = factory(state.clone(), principal.clone())?.1;
    let viewset = viewset.clone();
    // Authorization and response materialization must see the same snapshot;
    // a later write after a concurrent revoke cannot leak through a reload.
    let payload = state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                if !policy.rights(tx, V::Model::SYNC_NAME, id).await?.read {
                    return Ok::<_, AppError>(None);
                }
                let item = viewset
                    .get_queryset()
                    .filter(V::Model::primary_key().eq(id))
                    .first_in(tx)
                    .await?
                    .ok_or(AppError::NotFound)?;
                Ok(Some(V::Serializer::to_json(item)?))
            })
        })
        .await?;
    let status = if action == ViewAction::Create {
        StatusCode::CREATED
    } else if payload.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    };
    Ok(match payload {
        Some(payload) => (status, Json(payload)).into_response(),
        None => status.into_response(),
    })
}
