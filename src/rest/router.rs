use std::{collections::HashMap, future::Future, marker::PhantomData, pin::Pin};

use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use che_orm2::{
    Database, DatabaseQuery, Loaded, Model, ModelField, ModelSerializer, ModelWriteSerializer,
    PrefetchRelatedQuery, QueryValue, SelectRelatedQuery, ValidatedWrite, WithOne, WithOptionalOne,
    WriteMode,
};
use serde::Serialize;
use serde_json::json;

use crate::{AppError, AppResult, AppState, auth::CurrentUser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAction {
    List,
    Retrieve,
    Create,
    Update,
    Patch,
    Delete,
}

pub trait Permission<M: Model>: Clone + Send + Sync + Default + 'static {
    fn check(
        &self,
        _state: &AppState,
        _user: Option<&CurrentUser>,
        _action: ViewAction,
    ) -> AppResult<()> {
        Ok(())
    }
    fn check_object(
        &self,
        _state: &AppState,
        _user: Option<&CurrentUser>,
        _action: ViewAction,
        _model: &M,
    ) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
pub struct AllowAny;
impl<M: Model> Permission<M> for AllowAny {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Exact,
    Contains,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("unknown filter: {0}")]
    UnknownFilter(String),
    #[error("unknown ordering field: {0}")]
    UnknownOrdering(String),
    #[error("invalid value for {field}, expected {expected}")]
    InvalidValue {
        field: String,
        expected: &'static str,
    },
}

pub trait FilterValue: QueryValue<Self> + Sized {
    const EXPECTED: &'static str;
    fn parse(value: &str) -> Result<Self, FilterError>;
}
impl FilterValue for i64 {
    const EXPECTED: &'static str = "integer";
    fn parse(v: &str) -> Result<Self, FilterError> {
        v.parse().map_err(|_| FilterError::InvalidValue {
            field: String::new(),
            expected: Self::EXPECTED,
        })
    }
}
impl FilterValue for bool {
    const EXPECTED: &'static str = "boolean";
    fn parse(v: &str) -> Result<Self, FilterError> {
        match v {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(FilterError::InvalidValue {
                field: String::new(),
                expected: Self::EXPECTED,
            }),
        }
    }
}
impl FilterValue for String {
    const EXPECTED: &'static str = "string";
    fn parse(v: &str) -> Result<Self, FilterError> {
        Ok(v.to_owned())
    }
}
impl FilterValue for time::OffsetDateTime {
    const EXPECTED: &'static str = "RFC3339 datetime";
    fn parse(v: &str) -> Result<Self, FilterError> {
        time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339).map_err(
            |_| FilterError::InvalidValue {
                field: String::new(),
                expected: Self::EXPECTED,
            },
        )
    }
}

pub trait RestQuerySet: Sized + Send {
    type Model: Model + Send + Sync + 'static;
    type Item: Send + Sync + 'static;

    fn item_model(item: &Self::Item) -> &Self::Model;

    fn filter(self, expr: che_orm2::Expr) -> Self;
    fn order_by(self, order: che_orm2::OrderBy) -> Self;
    fn limit(self, limit: u64) -> Self;
    fn offset(self, offset: u64) -> Self;
    fn all<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Item>, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a;

    fn count<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<usize, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move { Ok(<Self as RestQuerySet>::all(self, database).await?.len()) })
    }

    fn first<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Self::Item>, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            Ok(<Self as RestQuerySet>::all(self.limit(1), database)
                .await?
                .into_iter()
                .next())
        })
    }
}

type Apply = fn(&'static str, &str) -> Result<che_orm2::Expr, FilterError>;
type Order = fn(&'static str, bool) -> che_orm2::OrderBy;

#[derive(Clone, Copy)]
pub struct Filter<M: Model> {
    pub name: &'static str,
    source: &'static str,
    lookup: Lookup,
    apply: Apply,
    order: Order,
    _marker: PhantomData<fn() -> M>,
}

impl<M: Model> Filter<M> {
    pub const fn exact<T: FilterValue>(field: ModelField<M, T>) -> Self {
        Self::typed(field, Lookup::Exact, apply_exact::<M, T>)
    }
    pub const fn exact_enum<T: che_orm2::DbEnum>(field: ModelField<M, T>) -> Self {
        Self::typed_enum(field, apply_enum_exact::<M, T>)
    }
    pub const fn contains(field: ModelField<M, String>) -> Self {
        Self::typed(field, Lookup::Contains, apply_contains::<M>)
    }
    pub const fn gt<T: FilterValue>(field: ModelField<M, T>) -> Self {
        Self::typed(field, Lookup::Gt, apply_gt::<M, T>)
    }
    pub const fn gte<T: FilterValue>(field: ModelField<M, T>) -> Self {
        Self::typed(field, Lookup::Gte, apply_gte::<M, T>)
    }
    pub const fn lt<T: FilterValue>(field: ModelField<M, T>) -> Self {
        Self::typed(field, Lookup::Lt, apply_lt::<M, T>)
    }
    pub const fn lte<T: FilterValue>(field: ModelField<M, T>) -> Self {
        Self::typed(field, Lookup::Lte, apply_lte::<M, T>)
    }
    pub const fn exact_as<T: FilterValue>(name: &'static str, field: ModelField<M, T>) -> Self {
        let mut result = Self::exact(field);
        result.name = name;
        result
    }
    const fn typed<T: FilterValue>(field: ModelField<M, T>, lookup: Lookup, apply: Apply) -> Self {
        Self {
            name: field.column().name,
            source: field.column().name,
            lookup,
            apply,
            order: apply_order::<M, T>,
            _marker: PhantomData,
        }
    }
    const fn typed_enum<T: che_orm2::DbEnum>(field: ModelField<M, T>, apply: Apply) -> Self {
        Self {
            name: field.column().name,
            source: field.column().name,
            lookup: Lookup::Exact,
            apply,
            order: apply_order::<M, T>,
            _marker: PhantomData,
        }
    }
    fn matches(&self, name: &str) -> bool {
        match self.lookup {
            Lookup::Exact => name == self.name,
            Lookup::Contains => name == format!("{}__contains", self.name),
            Lookup::Gt => name == format!("{}__gt", self.name),
            Lookup::Gte => name == format!("{}__gte", self.name),
            Lookup::Lt => name == format!("{}__lt", self.name),
            Lookup::Lte => name == format!("{}__lte", self.name),
        }
    }

    pub const fn source(&self) -> &'static str {
        self.source
    }

    pub const fn lookup(&self) -> Lookup {
        self.lookup
    }
}

pub trait FilterSetSpec: Clone + Send + Sync + 'static {
    type Model: Model;
    fn filters(&self) -> &'static [Filter<Self::Model>];
    fn apply<Q>(&self, query: Q, params: &HashMap<String, String>) -> Result<Q, FilterError>
    where
        Q: RestQuerySet<Model = Self::Model>,
    {
        FilterSet::new(self.filters()).apply(query, params)
    }
}
pub struct FilterSet<M: Model + 'static> {
    filters: &'static [Filter<M>],
    _marker: PhantomData<fn() -> M>,
}
impl<M: Model + 'static> Clone for FilterSet<M> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M: Model + 'static> Copy for FilterSet<M> {}
impl<M: Model + 'static> FilterSet<M> {
    pub const fn new(filters: &'static [Filter<M>]) -> Self {
        Self {
            filters,
            _marker: PhantomData,
        }
    }
    pub fn apply<Q>(&self, mut query: Q, params: &HashMap<String, String>) -> Result<Q, FilterError>
    where
        Q: RestQuerySet<Model = M>,
    {
        for (name, value) in params {
            if matches!(name.as_str(), "limit" | "offset" | "ordering") {
                continue;
            }
            let f = self
                .filters
                .iter()
                .find(|f| f.matches(name))
                .ok_or_else(|| FilterError::UnknownFilter(name.clone()))?;
            query = query.filter((f.apply)(f.source, value).map_err(|e| match e {
                FilterError::InvalidValue { expected, .. } => FilterError::InvalidValue {
                    field: name.clone(),
                    expected,
                },
                other => other,
            })?);
        }
        if let Some(ordering) = params.get("ordering") {
            let descending = ordering.starts_with('-');
            let name = ordering.strip_prefix('-').unwrap_or(ordering);
            let f = self
                .filters
                .iter()
                .find(|f| f.name == name)
                .ok_or_else(|| FilterError::UnknownOrdering(name.to_owned()))?;
            query = query.order_by((f.order)(f.source, descending));
        }
        Ok(query)
    }
}
impl<M: Model + 'static> Default for FilterSet<M> {
    fn default() -> Self {
        Self::new(&[])
    }
}
impl<M: Model + 'static> FilterSetSpec for FilterSet<M> {
    type Model = M;
    fn filters(&self) -> &'static [Filter<M>] {
        self.filters
    }
}

fn apply_exact<M: Model, T: FilterValue>(
    field: &'static str,
    value: &str,
) -> Result<che_orm2::Expr, FilterError> {
    Ok(ModelField::<M, T>::new(M::table_name(), field).eq(T::parse(value)?))
}
fn apply_enum_exact<M: Model, T: che_orm2::DbEnum>(
    field: &'static str,
    value: &str,
) -> Result<che_orm2::Expr, FilterError> {
    let parsed = T::from_str(value).ok_or(FilterError::InvalidValue {
        field: String::new(),
        expected: "enum value",
    })?;
    Ok(ModelField::<M, T>::new(M::table_name(), field).eq(parsed))
}
fn apply_contains<M: Model>(
    field: &'static str,
    value: &str,
) -> Result<che_orm2::Expr, FilterError> {
    Ok(ModelField::<M, String>::new(M::table_name(), field).contains(value))
}
macro_rules! range {
    ($fn:ident, $method:ident) => {
        fn $fn<M: Model, T: FilterValue>(
            field: &'static str,
            value: &str,
        ) -> Result<che_orm2::Expr, FilterError> {
            Ok(ModelField::<M, T>::new(M::table_name(), field).$method(T::parse(value)?))
        }
    };
}
range!(apply_gt, gt);
range!(apply_gte, gte);
range!(apply_lt, lt);
range!(apply_lte, lte);
fn apply_order<M: Model, T>(field: &'static str, desc: bool) -> che_orm2::OrderBy {
    let f = ModelField::<M, T>::new(M::table_name(), field);
    if desc { f.desc() } else { f.asc() }
}

impl<M> RestQuerySet for DatabaseQuery<M>
where
    M: Model + Send + Sync + 'static,
{
    type Model = M;
    type Item = M;

    fn item_model(item: &Self::Item) -> &Self::Model {
        item
    }

    fn filter(self, expr: che_orm2::Expr) -> Self {
        DatabaseQuery::filter(self, expr)
    }

    fn order_by(self, order: che_orm2::OrderBy) -> Self {
        DatabaseQuery::order_by(self, order)
    }

    fn limit(self, limit: u64) -> Self {
        DatabaseQuery::limit(self, limit)
    }

    fn offset(self, offset: u64) -> Self {
        DatabaseQuery::offset(self, offset)
    }

    fn all<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Item>, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(DatabaseQuery::all(self, database))
    }

    fn count<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<usize, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(DatabaseQuery::count(self, database))
    }
}

impl<M, R, Relation> RestQuerySet for SelectRelatedQuery<M, R, Relation, i64>
where
    M: Model + Send + Sync + 'static,
    R: Model + Send + Sync + 'static,
    Relation: Send + Sync + 'static,
{
    type Model = M;
    type Item = WithOne<M, R, Relation>;

    fn item_model(item: &Self::Item) -> &Self::Model {
        &item.model
    }

    fn filter(self, expr: che_orm2::Expr) -> Self {
        SelectRelatedQuery::<M, R, Relation, i64>::filter(self, expr)
    }

    fn order_by(self, order: che_orm2::OrderBy) -> Self {
        SelectRelatedQuery::<M, R, Relation, i64>::order_by(self, order)
    }

    fn limit(self, limit: u64) -> Self {
        SelectRelatedQuery::<M, R, Relation, i64>::limit(self, limit)
    }

    fn offset(self, offset: u64) -> Self {
        SelectRelatedQuery::<M, R, Relation, i64>::offset(self, offset)
    }

    fn all<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Item>, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(SelectRelatedQuery::<M, R, Relation, i64>::all(
            self, database,
        ))
    }

    fn count<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<usize, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(SelectRelatedQuery::<M, R, Relation, i64>::count(
            self, database,
        ))
    }
}

impl<M, R, Relation> RestQuerySet for SelectRelatedQuery<M, R, Relation, Option<i64>>
where
    M: Model + Send + Sync + 'static,
    R: Model + Send + Sync + 'static,
    Relation: Send + Sync + 'static,
{
    type Model = M;
    type Item = WithOptionalOne<M, R, Relation>;

    fn item_model(item: &Self::Item) -> &Self::Model {
        &item.model
    }

    fn filter(self, expr: che_orm2::Expr) -> Self {
        SelectRelatedQuery::<M, R, Relation, Option<i64>>::filter(self, expr)
    }

    fn order_by(self, order: che_orm2::OrderBy) -> Self {
        SelectRelatedQuery::<M, R, Relation, Option<i64>>::order_by(self, order)
    }

    fn limit(self, limit: u64) -> Self {
        SelectRelatedQuery::<M, R, Relation, Option<i64>>::limit(self, limit)
    }

    fn offset(self, offset: u64) -> Self {
        SelectRelatedQuery::<M, R, Relation, Option<i64>>::offset(self, offset)
    }

    fn all<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Item>, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(SelectRelatedQuery::<M, R, Relation, Option<i64>>::all(
            self, database,
        ))
    }

    fn count<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<usize, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(SelectRelatedQuery::<M, R, Relation, Option<i64>>::count(
            self, database,
        ))
    }
}

impl<M, R, Relation> RestQuerySet for PrefetchRelatedQuery<M, R, Relation, i64>
where
    M: Model + Send + Sync + 'static,
    R: Model + Send + Sync + 'static,
    Relation: Send + Sync + 'static,
{
    type Model = M;
    type Item = Loaded<M, (che_orm2::LoadedMany<R, Relation>,)>;

    fn item_model(item: &Self::Item) -> &Self::Model {
        &item.model
    }

    fn filter(self, expr: che_orm2::Expr) -> Self {
        PrefetchRelatedQuery::filter(self, expr)
    }

    fn order_by(self, order: che_orm2::OrderBy) -> Self {
        PrefetchRelatedQuery::order_by(self, order)
    }

    fn limit(self, limit: u64) -> Self {
        PrefetchRelatedQuery::limit(self, limit)
    }

    fn offset(self, offset: u64) -> Self {
        PrefetchRelatedQuery::offset(self, offset)
    }

    fn all<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Item>, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(PrefetchRelatedQuery::all(self, database))
    }

    fn count<'a>(
        self,
        database: &'a Database,
    ) -> Pin<Box<dyn Future<Output = Result<usize, che_orm2::OrmError>> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(PrefetchRelatedQuery::count(self, database))
    }
}

pub trait ViewSet: Clone + Send + Sync + 'static {
    type Model: Model + Send + Sync + 'static;
    type Serializer: ModelSerializer<Model = Self::Model>
        + ModelWriteSerializer<Model = Self::Model>
        + Serialize
        + Send
        + Sync
        + 'static;
    type QuerySet: RestQuerySet<Model = Self::Model, Item = <Self::Serializer as ModelSerializer>::Input>
        + Send
        + Sync;
    type FilterSet: FilterSetSpec<Model = Self::Model> + Default;
    type Permission: Permission<Self::Model>;
    fn path(&self) -> &'static str;
    fn filterset(&self) -> Self::FilterSet {
        Default::default()
    }
    fn get_queryset(&self) -> Self::QuerySet;
    fn prepare_create(
        &self,
        _state: &AppState,
        _user: Option<&CurrentUser>,
        write: ValidatedWrite<Self::Model>,
    ) -> AppResult<ValidatedWrite<Self::Model>> {
        Ok(write)
    }
    fn prepare_update(
        &self,
        _state: &AppState,
        _user: Option<&CurrentUser>,
        _current: &Self::Model,
        write: ValidatedWrite<Self::Model>,
    ) -> AppResult<ValidatedWrite<Self::Model>> {
        Ok(write)
    }
    fn prepare_patch(
        &self,
        _state: &AppState,
        _user: Option<&CurrentUser>,
        _current: &Self::Model,
        write: ValidatedWrite<Self::Model>,
    ) -> AppResult<ValidatedWrite<Self::Model>> {
        Ok(write)
    }
    fn actions(&self) -> Router {
        Router::new()
    }
    fn openapi_actions(&self) -> serde_json::Value {
        json!({})
    }
    fn signal_name(&self, action: ViewAction) -> Option<String> {
        let resource = self.path().trim_matches('/').replace('/', ".");
        match action {
            ViewAction::Create => Some(format!("{resource}.created")),
            ViewAction::Update | ViewAction::Patch => Some(format!("{resource}.updated")),
            ViewAction::Delete => Some(format!("{resource}.deleted")),
            ViewAction::List | ViewAction::Retrieve => None,
        }
    }
    fn signal_access(&self, _action: ViewAction) -> Option<crate::SignalAccess> {
        None
    }
}

pub struct CrudViewSet<M, S> {
    path: &'static str,
    _marker: PhantomData<fn() -> (M, S)>,
}
impl<M, S> Clone for CrudViewSet<M, S> {
    fn clone(&self) -> Self {
        Self {
            path: self.path,
            _marker: PhantomData,
        }
    }
}
impl<M, S> CrudViewSet<M, S> {
    pub const fn new(path: &'static str) -> Self {
        Self {
            path,
            _marker: PhantomData,
        }
    }
}
impl<M, S> ViewSet for CrudViewSet<M, S>
where
    M: Model + Send + Sync + 'static,
    S: ModelSerializer<Model = M, Input = M>
        + ModelWriteSerializer<Model = M>
        + Serialize
        + Send
        + Sync
        + 'static,
{
    type Model = M;
    type Serializer = S;
    type QuerySet = DatabaseQuery<M>;
    type FilterSet = FilterSet<M>;
    type Permission = AllowAny;
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(M::query())
    }
    fn path(&self) -> &'static str {
        self.path
    }
}

pub fn router<V: ViewSet>(state: AppState, viewset: V) -> Router
where
    V::Serializer: Serialize,
{
    let path = viewset.path().trim_end_matches('/');
    Router::new()
        .route(&format!("{path}/"), get(list::<V>).post(create::<V>))
        .route(
            &format!("{path}/{{id}}/"),
            get(retrieve::<V>)
                .put(update::<V>)
                .patch(patch::<V>)
                .delete(destroy::<V>),
        )
        .merge(viewset.actions())
        .layer(Extension(state))
        .layer(Extension(viewset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_signal_names_normalize_nested_paths() {
        let viewset =
            CrudViewSet::<crate::auth::User, crate::auth::AdminUserSerializer>::new("/auth/users");

        assert_eq!(
            viewset.signal_name(ViewAction::Create).as_deref(),
            Some("auth.users.created")
        );
        assert_eq!(
            viewset.signal_name(ViewAction::Update).as_deref(),
            Some("auth.users.updated")
        );
        assert_eq!(
            viewset.signal_name(ViewAction::Delete).as_deref(),
            Some("auth.users.deleted")
        );
    }
}

pub fn openapi_json_for<M, S>(path: &str) -> serde_json::Value
where
    M: Model,
    S: ModelSerializer<Model = M> + ModelWriteSerializer<Model = M> + Serialize,
{
    let serializer_name = std::any::type_name::<S>()
        .rsplit("::")
        .next()
        .unwrap_or("Serializer");
    let response = format!("{serializer_name}Response");
    let create = format!("{serializer_name}Create");
    let update = format!("{serializer_name}Update");
    let patch = format!("{serializer_name}Patch");
    let list = format!("{serializer_name}List");
    let schema = M::schema();
    let mut response_properties = serde_json::Map::new();
    let mut create_properties = serde_json::Map::new();
    let mut update_properties = serde_json::Map::new();
    let mut patch_properties = serde_json::Map::new();
    let mut response_required = Vec::new();
    let mut create_required = Vec::new();
    let mut update_required = Vec::new();
    for field in S::fields() {
        let column = schema
            .columns
            .iter()
            .find(|column| column.name == field.source);
        let property = column
            .map(openapi_column_schema)
            .unwrap_or_else(|| json!({"type": "object"}));
        let property = if field.related_model.is_some() {
            json!({"type": "object"})
        } else {
            property
        };
        if !field.write_only {
            response_properties.insert(field.name.to_owned(), property.clone());
            if column.is_some_and(|column| !column.nullable) {
                response_required.push(field.name.to_owned());
            }
        }
        if !field.read_only && field.related_model.is_none() {
            create_properties.insert(field.name.to_owned(), property.clone());
            update_properties.insert(field.name.to_owned(), property.clone());
            patch_properties.insert(field.name.to_owned(), property);
            if column.is_some_and(|column| {
                !column.nullable
                    && column.default.is_none()
                    && !column.auto_now
                    && !column.auto_now_add
            }) {
                create_required.push(field.name.to_owned());
                update_required.push(field.name.to_owned());
            }
        }
    }
    let response_schema = object_schema(response_properties, response_required);
    let create_schema = object_schema(create_properties, create_required);
    let update_schema = object_schema(update_properties, update_required);
    let patch_schema = object_schema(patch_properties, Vec::new());
    let error = schema_ref("Error");
    let collection = format!("{}/", path.trim_end_matches('/'));
    let detail = format!("{collection}{{id}}/");
    json!({
        "openapi": "3.0.3",
        "info": {"title": "che-rest API", "version": "0.1.0"},
        "paths": {
            collection: {
                "get": {"responses": {"200": {"description": "OK", "content": {"application/json": {"schema": schema_ref(&list)}}}}},
                "post": {"security": [{"TokenAuth": []}, {"SessionCookie": [], "CsrfToken": []}], "requestBody": {"required": true, "content": {"application/json": {"schema": schema_ref(&create)}}}, "responses": {"201": response_with_ref(&response), "400": response_with_ref_value(&error), "401": response_with_ref_value(&error), "403": response_with_ref_value(&error)}}
            },
            detail: {
                "get": {"responses": {"200": response_with_ref(&response), "401": response_with_ref_value(&error), "403": response_with_ref_value(&error), "404": response_with_ref_value(&error)}},
                "put": {"security": [{"TokenAuth": []}, {"SessionCookie": [], "CsrfToken": []}], "requestBody": {"required": true, "content": {"application/json": {"schema": schema_ref(&update)}}}, "responses": {"200": response_with_ref(&response), "400": response_with_ref_value(&error), "401": response_with_ref_value(&error), "403": response_with_ref_value(&error), "404": response_with_ref_value(&error)}},
                "patch": {"security": [{"TokenAuth": []}, {"SessionCookie": [], "CsrfToken": []}], "requestBody": {"required": true, "content": {"application/json": {"schema": schema_ref(&patch)}}}, "responses": {"200": response_with_ref(&response), "400": response_with_ref_value(&error), "401": response_with_ref_value(&error), "403": response_with_ref_value(&error), "404": response_with_ref_value(&error)}},
                "delete": {"security": [{"TokenAuth": []}, {"SessionCookie": [], "CsrfToken": []}], "responses": {"204": {"description": "No Content"}, "401": response_with_ref_value(&error), "403": response_with_ref_value(&error), "404": response_with_ref_value(&error)}}
            }
        },
        "components": {"schemas": {
            response.clone(): response_schema,
            create: create_schema,
            update: update_schema,
            patch: patch_schema,
            list: {"type": "object", "required": ["count", "results"], "properties": {"count": {"type": "integer"}, "results": {"type": "array", "items": schema_ref(&response)}}},
            "Error": {"type": "object", "required": ["detail"], "properties": {"detail": {"type": "string"}}}
        }}
    })
}

fn response_with_ref(name: &str) -> serde_json::Value {
    json!({"description": "OK", "content": {"application/json": {"schema": schema_ref(name)}}})
}

fn response_with_ref_value(reference: &serde_json::Value) -> serde_json::Value {
    json!({"description": "Error", "content": {"application/json": {"schema": reference}}})
}

fn object_schema(
    properties: serde_json::Map<String, serde_json::Value>,
    required: Vec<String>,
) -> serde_json::Value {
    let mut schema = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn schema_ref(name: &str) -> serde_json::Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

pub fn openapi_column_schema(column: &che_orm2::ColumnSchema) -> serde_json::Value {
    let mut schema = match column.column_type {
        che_orm2::ColumnType::Integer => json!({"type": "integer", "format": "int64"}),
        che_orm2::ColumnType::Text => json!({"type": "string"}),
        che_orm2::ColumnType::Boolean => json!({"type": "boolean"}),
        che_orm2::ColumnType::DateTime => json!({"type": "string", "format": "date-time"}),
    };
    if let Some(choices) = &column.choices {
        schema["enum"] = json!(choices);
    }
    if column.nullable {
        schema["nullable"] = json!(true);
    }
    schema
}

fn user(ext: &Option<Extension<CurrentUser>>) -> Option<&CurrentUser> {
    ext.as_ref().map(|e| &e.0)
}
fn error_filter(e: FilterError) -> AppError {
    AppError::BadRequest(e.to_string())
}
fn error_validation(e: che_orm2::ValidationErrors) -> AppError {
    AppError::BadRequest(e.detail)
}
fn error_write(e: che_orm2::OrmError) -> AppError {
    match e {
        che_orm2::OrmError::QueryBuild(error) => AppError::BadRequest(format!("{error:?}")),
        other => AppError::Orm(other),
    }
}
fn page(params: &HashMap<String, String>, name: &str) -> AppResult<u64> {
    params
        .get(name)
        .map(|v| {
            v.parse()
                .map_err(|_| AppError::BadRequest(format!("{name} must be an integer")))
        })
        .transpose()
        .map(|v| v.unwrap_or(0))
}

async fn list<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::List)?;
    let filter = viewset.filterset();
    let count = filter
        .apply(viewset.get_queryset(), &params)
        .map_err(error_filter)?
        .count(state.database())
        .await?;
    let offset = page(&params, "offset")?;
    let limit =
        page(&params, "limit").map(|v| if params.contains_key("limit") { v } else { 20 })?;
    if limit > 100 {
        return Err(AppError::BadRequest("limit must not exceed 100".into()));
    }
    let rows = filter
        .apply(viewset.get_queryset(), &params)
        .map_err(error_filter)?
        .limit(limit)
        .offset(offset)
        .all(state.database())
        .await?;
    let results = rows
        .into_iter()
        .map(V::Serializer::to_json)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(json!({"count": count, "results": results})))
}

async fn retrieve<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Retrieve)?;
    let item = viewset
        .get_queryset()
        .filter(V::Model::primary_key().eq(id))
        .first(state.database())
        .await?
        .ok_or(AppError::NotFound)?;
    V::Permission::default().check_object(
        &state,
        u,
        ViewAction::Retrieve,
        V::QuerySet::item_model(&item),
    )?;
    Ok(Json(V::Serializer::to_json(item)?))
}

async fn destroy<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
) -> AppResult<impl IntoResponse> {
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Delete)?;
    {
        let item = viewset
            .get_queryset()
            .filter(V::Model::primary_key().eq(id))
            .first(state.database())
            .await?
            .ok_or(AppError::NotFound)?;
        V::Permission::default().check_object(
            &state,
            u,
            ViewAction::Delete,
            V::QuerySet::item_model(&item),
        )?;
    }
    state.database().delete::<V::Model>(id).await?;
    if let Some(signal) = viewset.signal_name(ViewAction::Delete) {
        if viewset.signal_access(ViewAction::Delete).is_some() {
            state.signals().publish(signal, json!({"id": id}));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn create<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Json(data): Json<serde_json::Value>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Create)?;
    let write = V::Serializer::is_valid(data, WriteMode::Create).map_err(error_validation)?;
    let model = viewset
        .prepare_create(&state, u, write)?
        .save(state.database())
        .await
        .map_err(error_write)?
        .ok_or(AppError::BadRequest("create did not return a model".into()))?;
    let item = viewset
        .get_queryset()
        .filter(V::Model::primary_key().eq(model.primary_key_value()))
        .first(state.database())
        .await?
        .ok_or(AppError::NotFound)?;
    let payload = V::Serializer::to_json(item)?;
    if let Some(signal) = viewset.signal_name(ViewAction::Create) {
        if viewset.signal_access(ViewAction::Create).is_some() {
            state
                .signals()
                .publish(signal, json!({"id": model.primary_key_value()}));
        }
    }
    Ok((StatusCode::CREATED, Json(payload)))
}

async fn patch<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
    Json(data): Json<serde_json::Value>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Patch)?;
    let write = {
        let current = viewset
            .get_queryset()
            .filter(V::Model::primary_key().eq(id))
            .first(state.database())
            .await?
            .ok_or(AppError::NotFound)?;
        V::Permission::default().check_object(
            &state,
            u,
            ViewAction::Patch,
            V::QuerySet::item_model(&current),
        )?;
        let write =
            V::Serializer::is_valid(data, WriteMode::Patch { id }).map_err(error_validation)?;
        viewset.prepare_patch(&state, u, V::QuerySet::item_model(&current), write)?
    };
    let model = write
        .save(state.database())
        .await
        .map_err(error_write)?
        .ok_or(AppError::NotFound)?;
    let item = viewset
        .get_queryset()
        .filter(V::Model::primary_key().eq(model.primary_key_value()))
        .first(state.database())
        .await?
        .ok_or(AppError::NotFound)?;
    let payload = V::Serializer::to_json(item)?;
    if let Some(signal) = viewset.signal_name(ViewAction::Patch) {
        if viewset.signal_access(ViewAction::Patch).is_some() {
            state
                .signals()
                .publish(signal, json!({"id": model.primary_key_value()}));
        }
    }
    Ok(Json(payload))
}

async fn update<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
    Json(data): Json<serde_json::Value>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Update)?;
    let write = {
        let current = viewset
            .get_queryset()
            .filter(V::Model::primary_key().eq(id))
            .first(state.database())
            .await?
            .ok_or(AppError::NotFound)?;
        V::Permission::default().check_object(
            &state,
            u,
            ViewAction::Update,
            V::QuerySet::item_model(&current),
        )?;
        let write =
            V::Serializer::is_valid(data, WriteMode::Update { id }).map_err(error_validation)?;
        viewset.prepare_update(&state, u, V::QuerySet::item_model(&current), write)?
    };
    let model = write
        .save(state.database())
        .await
        .map_err(error_write)?
        .ok_or(AppError::NotFound)?;
    let item = viewset
        .get_queryset()
        .filter(V::Model::primary_key().eq(model.primary_key_value()))
        .first(state.database())
        .await?
        .ok_or(AppError::NotFound)?;
    let payload = V::Serializer::to_json(item)?;
    if let Some(signal) = viewset.signal_name(ViewAction::Update) {
        if viewset.signal_access(ViewAction::Update).is_some() {
            state
                .signals()
                .publish(signal, json!({"id": model.primary_key_value()}));
        }
    }
    Ok(Json(payload))
}
