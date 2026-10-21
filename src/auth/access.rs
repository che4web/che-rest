//! Group and typed per-object grants. All mutators require the caller's transaction.
use super::models::{AccessEvent, GroupMembership, GroupModelPermission, UserModelPermission};
use crate::rest::permissions::PermissionContext;
use crate::{AppError, AppResult, CurrentPrincipal, Permission, ViewAction};
use che_orm::{ColumnRef, Expr, Model, ModelField, QueryAst, QueryValue, SelectQuery, Transaction};
use std::marker::PhantomData;

pub const VIEW: &str = "view";
pub const ADD: &str = "add";
pub const CHANGE: &str = "change";
pub const DELETE: &str = "delete";
pub const MANAGE_ACCESS: &str = "manage_access";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    ObjectOnly,
    ModelOrObject,
}

/// Implement on each application-owned grant model. The schema must declare
/// real cascading FKs and a unique (object, subject, permission) constraint.
pub trait ObjectGrant<M: Model>: Model<PrimaryKey = i64> + Send + Sync + 'static {
    const OBJECT: ModelField<Self, M::PrimaryKey>;
    const SUBJECT: ModelField<Self, i64>;
    const PERMISSION: ModelField<Self, String>;
}

pub trait AccessModel: Model + Send + Sync + 'static {
    /// Stable public identity; independent of Rust paths and table renames.
    const ACCESS_KEY: &'static str;
    type UserGrant: ObjectGrant<Self>;
    type GroupGrant: ObjectGrant<Self>;
    /// Explicit permissions assigned to the creator, atomically with creation.
    const CREATOR_PERMISSIONS: &'static [&'static str] = &[];
    const MODE: PermissionMode = PermissionMode::ObjectOnly;
    const SUPERUSER_BYPASS: bool = false;
    const PERMISSIONS: &'static [&'static str] = &[VIEW, ADD, CHANGE, DELETE, MANAGE_ACCESS];
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Subject {
    User(i64),
    Group(i64),
}
impl Subject {
    fn id(self) -> i64 {
        match self {
            Self::User(id) | Self::Group(id) => id,
        }
    }
}

fn in_query<M: Model>(
    column: ColumnRef,
    query: SelectQuery<M>,
    selected: ColumnRef,
) -> AppResult<Expr> {
    let QueryAst::Select(mut query) = query.into_ast().map_err(che_orm::OrmError::from)? else {
        unreachable!()
    };
    query.columns = vec![selected];
    Ok(Expr::InQuery {
        left: Box::new(Expr::Column(column)),
        query: Box::new(query),
    })
}
fn groups(column: ColumnRef, user: i64) -> AppResult<Expr> {
    in_query(
        column,
        GroupMembership::query().filter(GroupMembership::USER_ID.eq(user)),
        GroupMembership::GROUP_ID.column(),
    )
}
fn principal(current: Option<&CurrentPrincipal>) -> AppResult<&CurrentPrincipal> {
    current.ok_or_else(|| AppError::Unauthorized("authentication required".into()))
}
fn action_permission(action: ViewAction) -> &'static str {
    match action {
        ViewAction::List | ViewAction::Retrieve => VIEW,
        ViewAction::Create => ADD,
        ViewAction::Update | ViewAction::Patch => CHANGE,
        ViewAction::Delete => DELETE,
    }
}
fn validate_permission<M: AccessModel>(permission: &str) -> AppResult<()> {
    if M::PERMISSIONS.contains(&permission) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "unknown permission {permission} for {}",
            M::ACCESS_KEY
        )))
    }
}

pub struct Access<M>(PhantomData<fn() -> M>);
impl<M> Copy for Access<M> {}
impl<M> Clone for Access<M> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M> Default for Access<M> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "source", content = "group_id", rename_all = "snake_case")]
pub enum PermissionSource {
    Direct,
    Group(i64),
    ModelDirect,
    ModelGroup(i64),
    Superuser,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EffectiveGrant {
    pub permission: String,
    pub source: PermissionSource,
}
#[derive(Debug, serde::Serialize)]
pub struct ObjectPermissions<K> {
    pub id: K,
    pub grants: Vec<EffectiveGrant>,
}

fn field_value<G: Model, T: che_orm::FromDatabaseValue>(
    model: &G,
    field: ModelField<G, T>,
) -> AppResult<T> {
    let value = model
        .field_values()
        .into_iter()
        .find(|value| value.column.name == field.column().name)
        .ok_or_else(|| {
            AppError::BadRequest("grant model does not expose its declared field".into())
        })?;
    T::from_database_value(&value.value)
        .map_err(che_orm::OrmError::from)
        .map_err(AppError::from)
}
impl<M: AccessModel> Access<M> {
    /// Batch introspection for an already-authorized page; callers must protect the ACL metadata itself.
    pub async fn effective_permissions(
        context: PermissionContext<'_>,
        current: &CurrentPrincipal,
        ids: &[M::PrimaryKey],
    ) -> AppResult<Vec<ObjectPermissions<M::PrimaryKey>>> {
        if ids.len() > 100 {
            return Err(AppError::BadRequest(
                "permission batch must not exceed 100 objects".into(),
            ));
        }
        let mut result: Vec<_> = ids
            .iter()
            .map(|id| ObjectPermissions {
                id: *id,
                grants: Vec::new(),
            })
            .collect();
        if ids.is_empty() {
            return Ok(result);
        }
        let user = current.auth_user().id;
        let values = || ids.iter().map(|id| (*id).into_query_value()).collect();
        let direct = context
            .fetch_all(
                M::UserGrant::query().filter(
                    Expr::In {
                        left: Box::new(Expr::Column(M::UserGrant::OBJECT.column())),
                        values: values(),
                    }
                    .and(M::UserGrant::SUBJECT.eq(user)),
                ),
            )
            .await?;
        let group = context
            .fetch_all(
                M::GroupGrant::query().filter(
                    Expr::In {
                        left: Box::new(Expr::Column(M::GroupGrant::OBJECT.column())),
                        values: values(),
                    }
                    .and(groups(M::GroupGrant::SUBJECT.column(), user)?),
                ),
            )
            .await?;
        for grant in direct {
            let id = field_value(&grant, M::UserGrant::OBJECT)?;
            let permission = field_value(&grant, M::UserGrant::PERMISSION)?;
            for object in result.iter_mut().filter(|object| object.id == id) {
                object.grants.push(EffectiveGrant {
                    permission: permission.clone(),
                    source: PermissionSource::Direct,
                });
            }
        }
        for grant in group {
            let id = field_value(&grant, M::GroupGrant::OBJECT)?;
            let permission = field_value(&grant, M::GroupGrant::PERMISSION)?;
            let group = field_value(&grant, M::GroupGrant::SUBJECT)?;
            for object in result.iter_mut().filter(|object| object.id == id) {
                object.grants.push(EffectiveGrant {
                    permission: permission.clone(),
                    source: PermissionSource::Group(group),
                });
            }
        }
        let mut model_grants = Vec::new();
        if M::MODE == PermissionMode::ModelOrObject {
            for grant in context
                .fetch_all(
                    UserModelPermission::query().filter(
                        UserModelPermission::USER_ID
                            .eq(user)
                            .and(UserModelPermission::MODEL_KEY.eq(M::ACCESS_KEY)),
                    ),
                )
                .await?
            {
                if grant.permission != ADD {
                    model_grants.push(EffectiveGrant {
                        permission: grant.permission,
                        source: PermissionSource::ModelDirect,
                    });
                }
            }
            for grant in context
                .fetch_all(
                    GroupModelPermission::query().filter(
                        groups(GroupModelPermission::GROUP_ID.column(), user)?
                            .and(GroupModelPermission::MODEL_KEY.eq(M::ACCESS_KEY)),
                    ),
                )
                .await?
            {
                if grant.permission != ADD {
                    model_grants.push(EffectiveGrant {
                        permission: grant.permission,
                        source: PermissionSource::ModelGroup(grant.group_id),
                    });
                }
            }
        }
        if M::SUPERUSER_BYPASS && current.auth_user().is_superuser {
            model_grants.extend(
                M::PERMISSIONS
                    .iter()
                    .filter(|permission| **permission != ADD)
                    .map(|permission| EffectiveGrant {
                        permission: (*permission).into(),
                        source: PermissionSource::Superuser,
                    }),
            );
        }
        for object in &mut result {
            object.grants.extend(model_grants.clone());
        }
        Ok(result)
    }
    pub fn schema() -> che_orm::SchemaSet {
        che_orm::SchemaSet::new()
            .model::<M::UserGrant>()
            .model::<M::GroupGrant>()
    }
    pub async fn has_model_permission(
        context: PermissionContext<'_>,
        user: i64,
        permission: &str,
    ) -> AppResult<bool> {
        if !context
            .fetch_all(
                UserModelPermission::query()
                    .filter(
                        UserModelPermission::USER_ID
                            .eq(user)
                            .and(UserModelPermission::MODEL_KEY.eq(M::ACCESS_KEY))
                            .and(UserModelPermission::PERMISSION.eq(permission)),
                    )
                    .limit(1),
            )
            .await?
            .is_empty()
        {
            return Ok(true);
        }
        Ok(!context
            .fetch_all(
                GroupModelPermission::query()
                    .filter(
                        groups(GroupModelPermission::GROUP_ID.column(), user)?
                            .and(GroupModelPermission::MODEL_KEY.eq(M::ACCESS_KEY))
                            .and(GroupModelPermission::PERMISSION.eq(permission)),
                    )
                    .limit(1),
            )
            .await?
            .is_empty())
    }
    pub async fn filter(
        context: PermissionContext<'_>,
        current: &CurrentPrincipal,
        permission: &str,
    ) -> AppResult<Option<Expr>> {
        validate_permission::<M>(permission)?;
        if (M::SUPERUSER_BYPASS && current.auth_user().is_superuser)
            || (M::MODE == PermissionMode::ModelOrObject
                && Self::has_model_permission(context, current.auth_user().id, permission).await?)
        {
            return Ok(None);
        }
        let user = current.auth_user().id;
        let direct = in_query(
            M::primary_key().column(),
            M::UserGrant::query().filter(
                M::UserGrant::SUBJECT
                    .eq(user)
                    .and(M::UserGrant::PERMISSION.eq(permission)),
            ),
            M::UserGrant::OBJECT.column(),
        )?;
        let group = in_query(
            M::primary_key().column(),
            M::GroupGrant::query().filter(
                groups(M::GroupGrant::SUBJECT.column(), user)?
                    .and(M::GroupGrant::PERMISSION.eq(permission)),
            ),
            M::GroupGrant::OBJECT.column(),
        )?;
        Ok(Some(direct.or(group)))
    }
    pub async fn can(
        context: PermissionContext<'_>,
        current: &CurrentPrincipal,
        permission: &str,
        id: M::PrimaryKey,
    ) -> AppResult<bool> {
        let mut query = M::query().filter(M::primary_key().eq(id));
        if let Some(filter) = Self::filter(context, current, permission).await? {
            query = query.filter(filter);
        }
        Ok(!context.fetch_all(query.limit(1)).await?.is_empty())
    }
    pub async fn require(
        context: PermissionContext<'_>,
        current: &CurrentPrincipal,
        permission: &str,
        id: M::PrimaryKey,
    ) -> AppResult<()> {
        if Self::can(context, current, permission, id).await? {
            Ok(())
        } else {
            Err(AppError::Forbidden(format!(
                "{permission} permission required"
            )))
        }
    }
    /// Trusted server operation. Does not authorize the actor; use change_access for user requests.
    pub async fn grant(
        tx: &Transaction,
        subject: Subject,
        permission: &str,
        id: M::PrimaryKey,
    ) -> AppResult<bool> {
        validate_permission::<M>(permission)?;
        if permission == ADD {
            return Err(AppError::BadRequest("add is a model permission".into()));
        }
        let changed = match subject {
            Subject::User(_) => grant::<M, M::UserGrant>(tx, subject.id(), permission, id).await?,
            Subject::Group(_) => {
                grant::<M, M::GroupGrant>(tx, subject.id(), permission, id).await?
            }
        };
        if changed {
            event(tx, "object_granted", serde_json::json!({"model": M::ACCESS_KEY, "object": key_json(id), "subject": subject, "permission": permission})).await?;
        }
        Ok(changed)
    }
    pub async fn revoke(
        tx: &Transaction,
        subject: Subject,
        permission: &str,
        id: M::PrimaryKey,
    ) -> AppResult<bool> {
        validate_permission::<M>(permission)?;
        let changed = match subject {
            Subject::User(_) => revoke::<M, M::UserGrant>(tx, subject.id(), permission, id).await?,
            Subject::Group(_) => {
                revoke::<M, M::GroupGrant>(tx, subject.id(), permission, id).await?
            }
        };
        if changed {
            event(tx, "object_revoked", serde_json::json!({"model": M::ACCESS_KEY, "object": key_json(id), "subject": subject, "permission": permission})).await?;
        }
        Ok(changed)
    }
    /// Explicit delegation bound supplied by trusted application code, never the request body.
    pub async fn change_access(
        context: PermissionContext<'_>,
        current: &CurrentPrincipal,
        subject: Subject,
        permission: &str,
        id: M::PrimaryKey,
        allow: bool,
        delegable: &[&str],
    ) -> AppResult<bool> {
        let tx = context
            .transaction
            .ok_or_else(|| AppError::BadRequest("access changes require a transaction".into()))?;
        Self::require(context, current, MANAGE_ACCESS, id).await?;
        if !delegable.contains(&permission) {
            return Err(AppError::Forbidden("permission cannot be delegated".into()));
        }
        if allow {
            Self::grant(tx, subject, permission, id).await
        } else {
            Self::revoke(tx, subject, permission, id).await
        }
    }
    pub async fn grant_model(
        tx: &Transaction,
        subject: Subject,
        permission: &str,
    ) -> AppResult<bool> {
        Self::change_model(tx, subject, permission, true).await
    }
    pub async fn revoke_model(
        tx: &Transaction,
        subject: Subject,
        permission: &str,
    ) -> AppResult<bool> {
        Self::change_model(tx, subject, permission, false).await
    }
    async fn change_model(
        tx: &Transaction,
        subject: Subject,
        permission: &str,
        allow: bool,
    ) -> AppResult<bool> {
        validate_permission::<M>(permission)?;
        let changed = match subject {
            Subject::User(user) => {
                let filter = UserModelPermission::USER_ID
                    .eq(user)
                    .and(UserModelPermission::MODEL_KEY.eq(M::ACCESS_KEY))
                    .and(UserModelPermission::PERMISSION.eq(permission));
                let existing = tx
                    .fetch_one(UserModelPermission::query().filter(filter.clone()))
                    .await?;
                if allow && existing.is_none() {
                    tx.create::<UserModelPermission>()
                        .set(UserModelPermission::USER_ID, user)
                        .set(UserModelPermission::MODEL_KEY, M::ACCESS_KEY)
                        .set(UserModelPermission::PERMISSION, permission)
                        .execute()
                        .await?;
                    true
                } else if !allow && existing.is_some() {
                    tx.execute(
                        UserModelPermission::delete()
                            .filter(filter)
                            .into_ast()
                            .map_err(che_orm::OrmError::from)?,
                    )
                    .await?;
                    true
                } else {
                    false
                }
            }
            Subject::Group(group) => {
                let filter = GroupModelPermission::GROUP_ID
                    .eq(group)
                    .and(GroupModelPermission::MODEL_KEY.eq(M::ACCESS_KEY))
                    .and(GroupModelPermission::PERMISSION.eq(permission));
                let existing = tx
                    .fetch_one(GroupModelPermission::query().filter(filter.clone()))
                    .await?;
                if allow && existing.is_none() {
                    tx.create::<GroupModelPermission>()
                        .set(GroupModelPermission::GROUP_ID, group)
                        .set(GroupModelPermission::MODEL_KEY, M::ACCESS_KEY)
                        .set(GroupModelPermission::PERMISSION, permission)
                        .execute()
                        .await?;
                    true
                } else if !allow && existing.is_some() {
                    tx.execute(
                        GroupModelPermission::delete()
                            .filter(filter)
                            .into_ast()
                            .map_err(che_orm::OrmError::from)?,
                    )
                    .await?;
                    true
                } else {
                    false
                }
            }
        };
        if changed {
            event(tx, if allow { "model_granted" } else { "model_revoked" }, serde_json::json!({"model": M::ACCESS_KEY, "subject": subject, "permission": permission})).await?;
        }
        Ok(changed)
    }
}

impl<M: AccessModel> Permission<M> for Access<M> {
    async fn check_request(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
    ) -> AppResult<()> {
        let current = principal(current)?;
        if action == ViewAction::Create
            && !(M::SUPERUSER_BYPASS && current.auth_user().is_superuser)
            && !Self::has_model_permission(context, current.auth_user().id, ADD).await?
        {
            return Err(AppError::Forbidden("add permission required".into()));
        }
        Ok(())
    }
    async fn scope(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
    ) -> AppResult<Option<Expr>> {
        self.check_request(context, current, action).await?;
        Self::filter(context, principal(current)?, action_permission(action)).await
    }
    async fn check_object(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
        model: &M,
    ) -> AppResult<()> {
        Self::require(
            context,
            principal(current)?,
            action_permission(action),
            model.primary_key_value(),
        )
        .await
    }
    async fn after_write(
        &self,
        context: PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
        model: &M,
    ) -> AppResult<()> {
        if action == ViewAction::Create {
            let user = principal(current)?.auth_user().id;
            let tx = context.transaction.ok_or_else(|| {
                AppError::BadRequest("creator grants require a transaction".into())
            })?;
            for permission in M::CREATOR_PERMISSIONS {
                Self::grant(
                    tx,
                    Subject::User(user),
                    permission,
                    model.primary_key_value(),
                )
                .await?;
            }
        }
        Ok(())
    }
    fn validate(&self, fields: &[che_orm::SerializerField]) -> AppResult<()> {
        validate_grant::<M, M::UserGrant>(super::User::table_name())?;
        validate_grant::<M, M::GroupGrant>(super::Group::table_name())?;
        if [VIEW, ADD, CHANGE, DELETE, MANAGE_ACCESS]
            .iter()
            .any(|permission| !M::PERMISSIONS.contains(permission))
            || M::CREATOR_PERMISSIONS
                .iter()
                .any(|permission| *permission == ADD || !M::PERMISSIONS.contains(permission))
        {
            return Err(AppError::BadRequest("ACL policies must declare the base permissions and valid object creator permissions".into()));
        }
        let schema = M::schema();
        if M::ACCESS_KEY.is_empty()
            || fields.iter().any(|field| {
                field.related_model.is_some()
                    || (!field.read_only
                        && schema.columns.iter().any(|column| {
                            (column.field_name == field.source || column.name == field.source)
                                && column.references.is_some()
                        }))
            })
        {
            return Err(AppError::BadRequest("ACL models require a stable ACCESS_KEY and an explicit relation authorization policy; use scalar serializers until that policy is implemented".into()));
        }
        Ok(())
    }
    fn model_access_key(&self) -> Option<&'static str> {
        Some(M::ACCESS_KEY)
    }
    fn object_scoped(&self) -> bool {
        true
    }
}

async fn grant<M: Model, G: ObjectGrant<M>>(
    tx: &Transaction,
    subject: i64,
    permission: &str,
    id: M::PrimaryKey,
) -> AppResult<bool> {
    let filter = G::OBJECT
        .eq(id)
        .and(G::SUBJECT.eq(subject))
        .and(G::PERMISSION.eq(permission));
    if tx.fetch_one(G::query().filter(filter)).await?.is_some() {
        return Ok(false);
    }
    tx.create::<G>()
        .set(G::OBJECT, id)
        .set(G::SUBJECT, subject)
        .set(G::PERMISSION, permission)
        .execute()
        .await?;
    Ok(true)
}
async fn revoke<M: Model, G: ObjectGrant<M>>(
    tx: &Transaction,
    subject: i64,
    permission: &str,
    id: M::PrimaryKey,
) -> AppResult<bool> {
    let filter = G::OBJECT
        .eq(id)
        .and(G::SUBJECT.eq(subject))
        .and(G::PERMISSION.eq(permission));
    if tx
        .fetch_one(G::query().filter(filter.clone()))
        .await?
        .is_none()
    {
        return Ok(false);
    }
    tx.execute(
        G::delete()
            .filter(filter)
            .into_ast()
            .map_err(che_orm::OrmError::from)?,
    )
    .await?;
    Ok(true)
}
fn key_json<K: QueryValue<K>>(key: K) -> serde_json::Value {
    match key.into_query_value() {
        che_orm::DatabaseValue::Integer(value) => serde_json::json!(value),
        che_orm::DatabaseValue::Uuid(value) => serde_json::json!(value.to_string()),
        _ => unreachable!("unsupported primary key"),
    }
}
async fn event(tx: &Transaction, kind: &str, payload: serde_json::Value) -> AppResult<()> {
    tx.create::<AccessEvent>()
        .set(AccessEvent::KIND, kind)
        .set(
            AccessEvent::PAYLOAD,
            serde_json::json!({"version": 1, "data": payload}).to_string(),
        )
        .execute()
        .await?;
    Ok(())
}
/// Trusted membership mutation, atomic with durable access-change work.
pub async fn add_member(tx: &Transaction, user: i64, group: i64) -> AppResult<bool> {
    let filter = GroupMembership::USER_ID
        .eq(user)
        .and(GroupMembership::GROUP_ID.eq(group));
    if tx
        .fetch_one(GroupMembership::query().filter(filter))
        .await?
        .is_some()
    {
        return Ok(false);
    }
    tx.create::<GroupMembership>()
        .set(GroupMembership::USER_ID, user)
        .set(GroupMembership::GROUP_ID, group)
        .execute()
        .await?;
    event(
        tx,
        "member_added",
        serde_json::json!({"user": user, "group": group}),
    )
    .await?;
    Ok(true)
}
pub async fn remove_member(tx: &Transaction, user: i64, group: i64) -> AppResult<bool> {
    let filter = GroupMembership::USER_ID
        .eq(user)
        .and(GroupMembership::GROUP_ID.eq(group));
    if tx
        .fetch_one(GroupMembership::query().filter(filter.clone()))
        .await?
        .is_none()
    {
        return Ok(false);
    }
    tx.execute(
        GroupMembership::delete()
            .filter(filter)
            .into_ast()
            .map_err(che_orm::OrmError::from)?,
    )
    .await?;
    event(
        tx,
        "member_removed",
        serde_json::json!({"user": user, "group": group}),
    )
    .await?;
    Ok(true)
}

fn validate_grant<M: Model, G: ObjectGrant<M>>(subject_table: &str) -> AppResult<()> {
    let schema = G::schema();
    let object = G::OBJECT.column();
    let subject = G::SUBJECT.column();
    let permission = G::PERMISSION.column();
    for (field, target, key) in [
        (object, M::table_name(), M::primary_key().column().name),
        (subject, subject_table, "id"),
    ] {
        let valid = field.table == G::table_name()
            && schema.columns.iter().any(|column| {
                column.name == field.name
                    && !column.nullable
                    && column.references.as_ref().is_some_and(|reference| {
                        reference.target_parts() == Some((target, key))
                            && reference
                                .on_delete()
                                .is_some_and(|action| action.eq_ignore_ascii_case("cascade"))
                    })
            });
        if !valid {
            return Err(AppError::BadRequest(format!(
                "{} must declare a cascading FK for {}",
                G::table_name(),
                field.name
            )));
        }
    }
    let mut expected = vec![object.name, subject.name, permission.name];
    expected.sort();
    if permission.table != G::table_name()
        || !schema.unique_constraints.iter().any(|columns| {
            let mut columns = columns.clone();
            columns.sort();
            columns == expected
        })
    {
        return Err(AppError::BadRequest(format!(
            "{} requires unique(object, subject, permission)",
            G::table_name()
        )));
    }
    Ok(())
}
