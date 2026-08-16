use std::sync::Arc;

use axum::{Json, Router, routing::get};
use che_orm2::SchemaSet;
use serde_json::{Map, Value, json};

use crate::{
    AppError, AppResult, AppState, CrudViewSet, Model, ModelSerializer, RestState, ViewSet,
    openapi_json_for, router,
};

pub trait AppModule: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn schema(&self) -> SchemaSet;
    fn init(&self, context: &mut ModuleContext);
    fn subscribe(&self, _state: &AppState) {}
    fn start(&self, _state: &AppState) {}
}

#[derive(Default)]
pub struct InstalledApps {
    modules: Vec<Arc<dyn AppModule>>,
}

impl InstalledApps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<M: AppModule>(mut self, module: M) -> Self {
        self.modules.push(Arc::new(module));
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn AppModule> {
        self.modules.iter().map(Arc::as_ref)
    }

    pub fn find(&self, name: &str) -> Option<&dyn AppModule> {
        self.iter().find(|module| module.name() == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.iter().map(AppModule::name)
    }
}

#[derive(Default)]
pub struct ModuleContext {
    rest_state: Option<RestState>,
    routers: Vec<Router>,
    schemas: Vec<SchemaSet>,
    openapi_paths: Map<String, Value>,
    openapi_components: Map<String, Value>,
}

impl ModuleContext {
    fn new(rest_state: RestState) -> Self {
        Self {
            rest_state: Some(rest_state),
            ..Self::default()
        }
    }

    pub fn route(&mut self, router: Router) {
        self.routers.push(router);
    }

    pub fn model<M: Model>(&mut self) {
        self.schemas.push(SchemaSet::new().model::<M>());
    }

    pub fn viewset<M, S>(&mut self, path: &'static str, viewset: CrudViewSet<M, S>)
    where
        M: Model + Send + 'static,
        S: ModelSerializer<Model = M, Input = M>
            + che_orm2::ModelWriteSerializer<Model = M>
            + serde::Serialize
            + Send
            + Sync
            + 'static,
    {
        self.viewset_with(path, viewset);
    }

    pub fn viewset_with<V>(&mut self, path: &'static str, viewset: V)
    where
        V: ViewSet,
        V::Serializer: serde::Serialize,
    {
        let state = self
            .rest_state
            .as_ref()
            .expect("module context is not initialized");
        let document = openapi_json_for::<V::Model, V::Serializer>(path, Default::default());
        if let Some(paths) = document["paths"].as_object() {
            self.openapi_paths.extend(
                paths
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        if let Some(schemas) = document["components"]["schemas"].as_object() {
            self.openapi_components.extend(
                schemas
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        self.routers.push(router(state.clone(), viewset));
    }

    fn schema(&self) -> SchemaSet {
        self.schemas
            .iter()
            .cloned()
            .fold(SchemaSet::new(), SchemaSet::merge)
    }
}

pub struct Server {
    state: AppState,
    apps: InstalledApps,
    api_prefix: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct First;
    struct Second;

    impl AppModule for First {
        fn name(&self) -> &'static str {
            "first"
        }

        fn schema(&self) -> SchemaSet {
            SchemaSet::new()
        }

        fn init(&self, _context: &mut ModuleContext) {}
    }

    impl AppModule for Second {
        fn name(&self) -> &'static str {
            "second"
        }

        fn schema(&self) -> SchemaSet {
            SchemaSet::new()
        }

        fn init(&self, _context: &mut ModuleContext) {}
    }

    #[test]
    fn installed_apps_preserve_registration_order() {
        let apps = InstalledApps::new().add(First).add(Second);
        assert_eq!(apps.names().collect::<Vec<_>>(), ["first", "second"]);
        assert!(apps.find("first").is_some());
    }
}

impl Server {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            apps: InstalledApps::new(),
            api_prefix: "/api".to_owned(),
        }
    }

    pub fn install(mut self, apps: InstalledApps) -> Self {
        self.apps = apps;
        self
    }

    pub fn api_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.api_prefix = prefix.into();
        self
    }

    pub async fn build(self) -> AppResult<Router> {
        let mut context = ModuleContext::new(self.state.rest_state());
        for module in self.apps.iter() {
            context.schemas.push(module.schema());
            module.init(&mut context);
        }

        let schema = context.schema();
        schema
            .validate()
            .map_err(|error| AppError::BadRequest(format!("invalid app schema: {error:?}")))?;
        let mut api = Router::new();
        for route in context.routers {
            api = api.merge(route);
        }

        let openapi = json!({
            "openapi": "3.0.3",
            "info": { "title": "che-rest API", "version": "0.1.0" },
            "paths": context.openapi_paths,
            "components": { "schemas": context.openapi_components },
        });
        api = api.route(
            "/openapi.json",
            get(move || async move { Json(openapi.clone()) }),
        );

        for module in self.apps.iter() {
            module.subscribe(&self.state);
        }
        for module in self.apps.iter() {
            module.start(&self.state);
        }

        Ok(Router::new().nest(&self.api_prefix, api))
    }
}
