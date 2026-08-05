use std::{collections::HashMap, marker::PhantomData};

use async_trait::async_trait;
use axum::{
    Extension, Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query},
    http::{Extensions, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use che_orm::SqliteModel;
use serde_json::{Map, Value, json};

use crate::{
    error::{AppError, AppResult},
    filters::{FilterSet, FilterSetSpec},
    permissions::{AllowAny, Permission, ViewAction},
    serializer::{ModelSerializer, Serializer},
    state::AppState,
};

const DEFAULT_PAGE_LIMIT: u32 = 20;

pub struct ModelViewSet<M, V = DefaultViewSet<M>> {
    _marker: PhantomData<(M, V)>,
}

#[async_trait]
pub trait ViewSet: Clone + Send + Sync + 'static {
    type Model: SqliteModel<Id = i64>;
    type Serializer: Serializer<Model = Self::Model> + Default;

    fn serializer(&self) -> Self::Serializer {
        Self::Serializer::default()
    }
    type FilterSet: FilterSetSpec<Model = Self::Model> + Default;
    type Permission: Permission<Self::Model>;

    fn filterset(&self) -> Self::FilterSet {
        Self::FilterSet::default()
    }

    fn extra_routes(&self) -> Router {
        Router::new()
    }

    async fn perform_create(
        &self,
        _state: &AppState,
        _extensions: &Extensions,
        payload: Map<String, Value>,
    ) -> AppResult<Map<String, Value>> {
        Ok(payload)
    }

    async fn system_create_values(
        &self,
        _state: &AppState,
        _extensions: &Extensions,
    ) -> AppResult<Map<String, Value>> {
        Ok(Map::new())
    }
}

pub struct DefaultViewSet<M> {
    serializer: ModelSerializer<M>,
    filterset: FilterSet<M>,
}

impl<M> Clone for DefaultViewSet<M> {
    fn clone(&self) -> Self {
        Self {
            serializer: self.serializer,
            filterset: self.filterset,
        }
    }
}

impl<M> DefaultViewSet<M> {
    pub const fn new(serializer: ModelSerializer<M>, filterset: FilterSet<M>) -> Self {
        Self {
            serializer,
            filterset,
        }
    }
}

impl<M> ViewSet for DefaultViewSet<M>
where
    M: SqliteModel<Id = i64>,
{
    type Model = M;
    type Serializer = ModelSerializer<M>;
    type FilterSet = FilterSet<M>;
    type Permission = AllowAny;

    fn serializer(&self) -> ModelSerializer<M> {
        self.serializer
    }

    fn filterset(&self) -> FilterSet<M> {
        self.filterset
    }
}

impl<M> ModelViewSet<M>
where
    M: SqliteModel<Id = i64>,
{
    pub fn router(
        base_path: &'static str,
        serializer: ModelSerializer<M>,
        filterset: FilterSet<M>,
    ) -> Router {
        Self::router_with(base_path, DefaultViewSet::new(serializer, filterset))
    }
}

impl<M, V> ModelViewSet<M, V>
where
    M: SqliteModel<Id = i64>,
    V: ViewSet<Model = M>,
{
    pub fn router_with(base_path: &'static str, viewset: V) -> Router {
        let collection_path = trailing_slash(base_path);
        let detail_path = format!("{}{{id}}/", collection_path);
        let extra_routes = viewset.extra_routes();

        Router::new()
            .route(&collection_path, get(Self::list).post(Self::create))
            .route(
                &detail_path,
                get(Self::retrieve)
                    .patch(Self::update)
                    .delete(Self::destroy),
            )
            .merge(extra_routes)
            .layer(Extension(viewset))
    }

    async fn list(
        Extension(state): Extension<AppState>,
        Extension(viewset): Extension<V>,
        user: Option<Extension<crate::auth::CurrentUser>>,
        Query(params): Query<HashMap<String, String>>,
    ) -> AppResult<Response> {
        let extensions = Extensions::new();
        let permission = V::Permission::default();
        permission
            .has_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::List,
            )
            .await?;
        let serializer = viewset.serializer().model_serializer();
        let filterset = viewset.filterset().filterset();
        let total = filterset
            .apply_for_count(M::objects(state.db()).query(), &params)?
            .count()
            .await?;
        let mut query = filterset.apply(M::objects(state.db()).query(), &params)?;
        if !params.contains_key("limit") {
            query = query.limit(DEFAULT_PAGE_LIMIT);
        }
        let models = query.all().await?;
        let mut results = Vec::new();
        for model in &models {
            results.push(serializer.to_json_async(state.db(), model).await?);
        }

        Ok(json_response(json!({
            "count": total,
            "results": results,
        })))
    }

    async fn create(
        Extension(state): Extension<AppState>,
        Extension(viewset): Extension<V>,
        user: Option<Extension<crate::auth::CurrentUser>>,
        request: axum::extract::Request<Body>,
    ) -> AppResult<Response> {
        let (parts, body) = request.into_parts();
        let permission = V::Permission::default();
        permission
            .has_permission(
                &state,
                &parts.extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Create,
            )
            .await?;
        let bytes = to_bytes(body, usize::MAX)
            .await
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        let payload: Map<String, Value> = serde_json::from_slice(bytes.as_ref())
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        let payload = viewset
            .perform_create(&state, &parts.extensions, payload)
            .await?;

        let serializer = viewset.serializer().model_serializer();
        let mut create = M::objects(state.db()).create();

        for (field, value) in serializer.create_values(Value::Object(payload))? {
            create = create.set(field, value);
        }
        let system_values = viewset
            .system_create_values(&state, &parts.extensions)
            .await?;
        for (field, value) in serializer.system_create_values(Value::Object(system_values))? {
            create = create.set(field, value);
        }

        let model = create.execute().await?;
        let payload = serializer.to_json_async(state.db(), &model).await?;
        Ok((StatusCode::CREATED, Json(payload)).into_response())
    }

    async fn retrieve(
        Extension(state): Extension<AppState>,
        Extension(viewset): Extension<V>,
        user: Option<Extension<crate::auth::CurrentUser>>,
        Path(id): Path<i64>,
    ) -> AppResult<Response> {
        let model = M::objects(state.db()).get(id).await?;
        let extensions = Extensions::new();
        let permission = V::Permission::default();
        permission
            .has_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Retrieve,
            )
            .await?;
        permission
            .has_object_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Retrieve,
                &model,
            )
            .await?;
        let serializer = viewset.serializer().model_serializer();
        Ok(json_response(
            serializer.to_json_async(state.db(), &model).await?,
        ))
    }

    async fn update(
        Extension(state): Extension<AppState>,
        Extension(viewset): Extension<V>,
        user: Option<Extension<crate::auth::CurrentUser>>,
        Path(id): Path<i64>,
        Json(payload): Json<Value>,
    ) -> AppResult<Response> {
        let model = M::objects(state.db()).get(id).await?;
        let extensions = Extensions::new();
        let permission = V::Permission::default();
        permission
            .has_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Update,
            )
            .await?;
        permission
            .has_object_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Update,
                &model,
            )
            .await?;
        let serializer = viewset.serializer().model_serializer();
        let mut update = M::objects(state.db()).update_fields(id);

        for (field, value) in serializer.update_values(payload)? {
            update = update.set(field, value);
        }

        let model = update.execute().await?;
        Ok(json_response(
            serializer.to_json_async(state.db(), &model).await?,
        ))
    }

    async fn destroy(
        Extension(state): Extension<AppState>,
        Extension(_viewset): Extension<V>,
        user: Option<Extension<crate::auth::CurrentUser>>,
        Path(id): Path<i64>,
    ) -> AppResult<Response> {
        let model = M::objects(state.db()).get(id).await?;
        let extensions = Extensions::new();
        let permission = V::Permission::default();
        permission
            .has_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Destroy,
            )
            .await?;
        permission
            .has_object_permission(
                &state,
                &extensions,
                user.as_ref().map(|user| &user.0),
                ViewAction::Destroy,
                &model,
            )
            .await?;
        M::objects(state.db()).delete(id).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

fn json_response(value: Value) -> Response {
    Json(value).into_response()
}

fn trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}
