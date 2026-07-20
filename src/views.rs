use std::marker::PhantomData;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use che_orm::SqliteModel;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::{error::AppResult, filters::FilterSet, serializer::ModelSerializer, state::AppState};

pub struct ModelViewSet<M> {
    _marker: PhantomData<M>,
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
        let detail_path = format!("{base_path}/{{id}}");

        Router::new()
            .route(base_path, get(Self::list).post(Self::create))
            .route(
                &detail_path,
                get(Self::retrieve)
                    .patch(Self::update)
                    .delete(Self::destroy),
            )
            .layer(Extension(serializer))
            .layer(Extension(filterset))
    }

    async fn list(
        Extension(state): Extension<AppState>,
        Extension(serializer): Extension<ModelSerializer<M>>,
        Extension(filterset): Extension<FilterSet<M>>,
        Query(params): Query<HashMap<String, String>>,
    ) -> AppResult<Response> {
        let query = filterset.apply(M::objects(state.db()).query(), &params)?;
        let models = query.all().await?;
        let results = models
            .iter()
            .map(|model| serializer.to_json(model))
            .collect::<Vec<_>>();

        Ok(json_response(json!({
            "count": results.len(),
            "results": results,
        })))
    }

    async fn create(
        Extension(state): Extension<AppState>,
        Extension(serializer): Extension<ModelSerializer<M>>,
        Json(payload): Json<Value>,
    ) -> AppResult<Response> {
        let mut create = M::objects(state.db()).create();

        for (field, value) in serializer.create_values(payload)? {
            create = create.set(field, value);
        }

        let model = create.execute().await?;
        Ok((StatusCode::CREATED, Json(serializer.to_json(&model))).into_response())
    }

    async fn retrieve(
        Extension(state): Extension<AppState>,
        Extension(serializer): Extension<ModelSerializer<M>>,
        Path(id): Path<i64>,
    ) -> AppResult<Response> {
        let model = M::objects(state.db()).get(id).await?;
        Ok(json_response(serializer.to_json(&model)))
    }

    async fn update(
        Extension(state): Extension<AppState>,
        Extension(serializer): Extension<ModelSerializer<M>>,
        Path(id): Path<i64>,
        Json(payload): Json<Value>,
    ) -> AppResult<Response> {
        let mut update = M::objects(state.db()).update_fields(id);

        for (field, value) in serializer.update_values(payload)? {
            update = update.set(field, value);
        }

        let model = update.execute().await?;
        Ok(json_response(serializer.to_json(&model)))
    }

    async fn destroy(
        Extension(state): Extension<AppState>,
        Path(id): Path<i64>,
    ) -> AppResult<Response> {
        M::objects(state.db()).get(id).await?;
        M::objects(state.db()).delete(id).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

fn json_response(value: Value) -> Response {
    Json(value).into_response()
}
