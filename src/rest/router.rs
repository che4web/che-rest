use std::{collections::HashMap, marker::PhantomData};

use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use che_orm2::{
    DatabaseQuery, Model, ModelField, ModelSerializer, ModelWriteSerializer, QueryValue,
};
use serde::Serialize;
use serde_json::json;

use crate::{AppError, AppResult, AppState, auth::CurrentUser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAction {
    List,
    Retrieve,
    Create,
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

type Apply<M> = for<'a> fn(
    DatabaseQuery<'a, M>,
    &'static str,
    &str,
) -> Result<DatabaseQuery<'a, M>, FilterError>;
type Order<M> = for<'a> fn(DatabaseQuery<'a, M>, &'static str, bool) -> DatabaseQuery<'a, M>;

#[derive(Clone, Copy)]
pub struct Filter<M: Model> {
    pub name: &'static str,
    source: &'static str,
    lookup: Lookup,
    apply: Apply<M>,
    order: Order<M>,
    _marker: PhantomData<fn() -> M>,
}

impl<M: Model> Filter<M> {
    pub const fn exact<T: FilterValue>(field: ModelField<M, T>) -> Self {
        Self::typed(field, Lookup::Exact, apply_exact::<M, T>)
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
    const fn typed<T: FilterValue>(
        field: ModelField<M, T>,
        lookup: Lookup,
        apply: Apply<M>,
    ) -> Self {
        Self {
            name: field.column().name,
            source: field.column().name,
            lookup,
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
}

pub trait FilterSetSpec: Clone + Send + Sync + 'static {
    type Model: Model;
    fn filters(&self) -> &'static [Filter<Self::Model>];
    fn apply<'a>(
        &self,
        query: DatabaseQuery<'a, Self::Model>,
        params: &HashMap<String, String>,
    ) -> Result<DatabaseQuery<'a, Self::Model>, FilterError> {
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
    pub fn apply<'a>(
        &self,
        mut query: DatabaseQuery<'a, M>,
        params: &HashMap<String, String>,
    ) -> Result<DatabaseQuery<'a, M>, FilterError> {
        for (name, value) in params {
            if matches!(name.as_str(), "limit" | "offset" | "ordering") {
                continue;
            }
            let f = self
                .filters
                .iter()
                .find(|f| f.matches(name))
                .ok_or_else(|| FilterError::UnknownFilter(name.clone()))?;
            query = (f.apply)(query, f.source, value).map_err(|e| match e {
                FilterError::InvalidValue { expected, .. } => FilterError::InvalidValue {
                    field: name.clone(),
                    expected,
                },
                other => other,
            })?;
        }
        if let Some(ordering) = params.get("ordering") {
            let descending = ordering.starts_with('-');
            let name = ordering.strip_prefix('-').unwrap_or(ordering);
            let f = self
                .filters
                .iter()
                .find(|f| f.name == name)
                .ok_or_else(|| FilterError::UnknownOrdering(name.to_owned()))?;
            query = (f.order)(query, f.source, descending);
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

fn apply_exact<'a, M: Model, T: FilterValue>(
    q: DatabaseQuery<'a, M>,
    field: &'static str,
    value: &str,
) -> Result<DatabaseQuery<'a, M>, FilterError> {
    Ok(q.filter(ModelField::<M, T>::new(M::table_name(), field).eq(T::parse(value)?)))
}
fn apply_contains<'a, M: Model>(
    q: DatabaseQuery<'a, M>,
    field: &'static str,
    value: &str,
) -> Result<DatabaseQuery<'a, M>, FilterError> {
    Ok(q.filter(ModelField::<M, String>::new(M::table_name(), field).contains(value)))
}
macro_rules! range {
    ($fn:ident, $method:ident) => {
        fn $fn<'a, M: Model, T: FilterValue>(
            q: DatabaseQuery<'a, M>,
            field: &'static str,
            value: &str,
        ) -> Result<DatabaseQuery<'a, M>, FilterError> {
            Ok(q.filter(ModelField::<M, T>::new(M::table_name(), field).$method(T::parse(value)?)))
        }
    };
}
range!(apply_gt, gt);
range!(apply_gte, gte);
range!(apply_lt, lt);
range!(apply_lte, lte);
fn apply_order<'a, M: Model, T>(
    q: DatabaseQuery<'a, M>,
    field: &'static str,
    desc: bool,
) -> DatabaseQuery<'a, M> {
    let f = ModelField::<M, T>::new(M::table_name(), field);
    if desc {
        q.order_by(f.desc())
    } else {
        q.order_by(f.asc())
    }
}

pub trait ViewSet: Clone + Send + Sync + 'static {
    type Model: Model + Send + 'static;
    type Serializer: ModelSerializer<Model = Self::Model, Input = Self::Model>
        + ModelWriteSerializer<Model = Self::Model>
        + Serialize
        + Send
        + Sync
        + 'static;
    type FilterSet: FilterSetSpec<Model = Self::Model> + Default;
    type Permission: Permission<Self::Model>;
    fn path(&self) -> &'static str;
    fn filterset(&self) -> Self::FilterSet {
        Default::default()
    }
    fn list_query<'a>(&self, q: DatabaseQuery<'a, Self::Model>) -> DatabaseQuery<'a, Self::Model> {
        q
    }
    fn retrieve_query<'a>(
        &self,
        q: DatabaseQuery<'a, Self::Model>,
    ) -> DatabaseQuery<'a, Self::Model> {
        q
    }
    fn actions(&self) -> Router {
        Router::new()
    }
    fn openapi_actions(&self) -> serde_json::Value {
        json!({})
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
    M: Model + Send + 'static,
    S: ModelSerializer<Model = M, Input = M>
        + ModelWriteSerializer<Model = M>
        + Serialize
        + Send
        + Sync
        + 'static,
{
    type Model = M;
    type Serializer = S;
    type FilterSet = FilterSet<M>;
    type Permission = AllowAny;
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
            get(retrieve::<V>).patch(patch::<V>).delete(destroy::<V>),
        )
        .merge(viewset.actions())
        .layer(Extension(state))
        .layer(Extension(viewset))
}

pub fn openapi_json_for<M, S>(path: &str) -> serde_json::Value
where
    M: Model,
    S: ModelSerializer<Model = M, Input = M> + ModelWriteSerializer<Model = M> + Serialize,
{
    let response = format!(
        "{}Response",
        std::any::type_name::<S>()
            .rsplit("::")
            .next()
            .unwrap_or("Model")
    );
    let mut properties = serde_json::Map::new();
    for field in S::fields() {
        properties.insert(field.name.to_owned(), json!({"type": "string"}));
    }
    let schema = json!({"type":"object", "properties": properties});
    let path = path.trim_end_matches('/');
    json!({"openapi":"3.0.3", "info":{"title":"che-rest API","version":"0.1.0"}, "paths": {
        format!("{path}/"): {"get":{"responses":{"200":{"description":"OK"}}}, "post":{"responses":{"201":{"description":"Created"}}}},
        format!("{path}/{{id}}/"): {"get":{"responses":{"200":{"description":"OK"}}}, "patch":{"responses":{"200":{"description":"OK"}}}, "delete":{"responses":{"204":{"description":"No Content"}}}}
    }, "components":{"schemas":{response:schema}}})
}

fn user(ext: &Option<Extension<CurrentUser>>) -> Option<&CurrentUser> {
    ext.as_ref().map(|e| &e.0)
}
fn error_filter(e: FilterError) -> AppError {
    AppError::BadRequest(e.to_string())
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
    let count = state
        .database()
        .count_query(
            filter
                .apply(
                    viewset.list_query(state.database().query::<V::Model>()),
                    &params,
                )
                .map_err(error_filter)?
                .into_select_query(),
        )
        .await?;
    let offset = page(&params, "offset")?;
    let limit =
        page(&params, "limit").map(|v| if params.contains_key("limit") { v } else { 20 })?;
    if limit > 100 {
        return Err(AppError::BadRequest("limit must not exceed 100".into()));
    }
    let rows = filter
        .apply(
            viewset.list_query(state.database().query::<V::Model>()),
            &params,
        )
        .map_err(error_filter)?
        .limit(limit)
        .offset(offset)
        .all()
        .await?;
    let results = rows
        .into_iter()
        .map(|m| serde_json::to_value(V::Serializer::from_input(m)))
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
    let model = viewset
        .retrieve_query(state.database().query::<V::Model>())
        .filter(V::Model::primary_key().eq(id))
        .first()
        .await?
        .ok_or(AppError::NotFound)?;
    V::Permission::default().check_object(&state, u, ViewAction::Retrieve, &model)?;
    Ok(Json(serde_json::to_value(V::Serializer::from_input(
        model,
    ))?))
}

async fn destroy<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
) -> AppResult<impl IntoResponse> {
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Delete)?;
    let model = viewset
        .retrieve_query(state.database().query::<V::Model>())
        .filter(V::Model::primary_key().eq(id))
        .first()
        .await?
        .ok_or(AppError::NotFound)?;
    V::Permission::default().check_object(&state, u, ViewAction::Delete, &model)?;
    state.database().delete::<V::Model>(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(_viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Json(input): Json<<V::Serializer as ModelWriteSerializer>::CreateInput>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Create)?;
    let model = V::Serializer::create(state.database(), input)
        .await
        .map_err(error_write)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(V::Serializer::from_input(model))?),
    ))
}

async fn patch<V: ViewSet>(
    Extension(state): Extension<AppState>,
    Extension(viewset): Extension<V>,
    who: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
    Json(input): Json<<V::Serializer as ModelWriteSerializer>::PatchInput>,
) -> AppResult<impl IntoResponse>
where
    V::Serializer: Serialize,
{
    let u = user(&who);
    V::Permission::default().check(&state, u, ViewAction::Patch)?;
    let current = viewset
        .retrieve_query(state.database().query::<V::Model>())
        .filter(V::Model::primary_key().eq(id))
        .first()
        .await?
        .ok_or(AppError::NotFound)?;
    V::Permission::default().check_object(&state, u, ViewAction::Patch, &current)?;
    let model = V::Serializer::patch(state.database(), id, input)
        .await
        .map_err(error_write)?
        .ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::to_value(V::Serializer::from_input(
        model,
    ))?))
}
