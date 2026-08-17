use std::sync::Arc;

use axum::{Json, Router, routing::get};
use che_orm2::SchemaSet;
use serde_json::{Map, Value, json};

use crate::{
    AppError, AppResult, AppState, CrudViewSet, FilterSetSpec, Model, ModelSerializer, ViewSet,
    openapi_json_for, router,
};

pub trait AppModule: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn schema(&self) -> SchemaSet;
    fn init(&self, context: &mut ModuleContext);
    fn subscribe(&self, _state: &AppState) {}
    fn start(&self, _state: &AppState) {}
    fn middleware(&self, router: Router, _state: &AppState) -> Router {
        router
    }
}

#[derive(Debug, Clone)]
pub struct ApiEndpoint {
    pub app_name: &'static str,
    pub model_name: String,
    pub resource: String,
    pub fields: Vec<che_orm2::SerializerField>,
    pub filters: Vec<ApiFilter>,
}

#[derive(Debug, Clone, Copy)]
pub struct ApiFilter {
    pub name: &'static str,
    pub source: &'static str,
    pub lookup: crate::Lookup,
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

    pub fn api_endpoints(&self, state: AppState) -> Vec<ApiEndpoint> {
        let mut context = ModuleContext::new(state);
        for module in self.iter() {
            context.current_app = module.name();
            module.init(&mut context);
        }
        context.api_endpoints
    }
}

#[derive(Default)]
pub struct ModuleContext {
    state: Option<AppState>,
    routers: Vec<Router>,
    root_routers: Vec<Router>,
    schemas: Vec<SchemaSet>,
    openapi_paths: Map<String, Value>,
    openapi_components: Map<String, Value>,
    api_endpoints: Vec<ApiEndpoint>,
    current_app: &'static str,
}

impl ModuleContext {
    fn new(state: AppState) -> Self {
        Self {
            state: Some(state),
            ..Self::default()
        }
    }

    pub fn route(&mut self, router: Router) {
        self.routers.push(router);
    }

    pub fn route_at_root(&mut self, router: Router) {
        self.root_routers.push(router);
    }

    pub fn model<M: Model>(&mut self) {
        self.schemas.push(SchemaSet::new().model::<M>());
    }

    pub fn viewset<M, S>(&mut self, path: &'static str, viewset: CrudViewSet<M, S>)
    where
        M: Model + Send + Sync + 'static,
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
        self.api_endpoints.push(ApiEndpoint {
            app_name: self.current_app,
            model_name: std::any::type_name::<V::Model>()
                .rsplit("::")
                .next()
                .unwrap_or("Model")
                .to_owned(),
            resource: path.trim_matches('/').to_owned(),
            fields: V::Serializer::fields().to_vec(),
            filters: V::FilterSet::default()
                .filters()
                .iter()
                .map(|filter| ApiFilter {
                    name: filter.name,
                    source: filter.source(),
                    lookup: filter.lookup(),
                })
                .collect(),
        });
        let state = self
            .state
            .as_ref()
            .expect("module context is not initialized");
        let document = openapi_json_for::<V::Model, V::Serializer>(path);
        let mut document = document;
        if let Some(action_paths) = viewset
            .openapi_actions()
            .get("paths")
            .and_then(Value::as_object)
        {
            if let Some(paths) = document["paths"].as_object_mut() {
                paths.extend(action_paths.clone());
            }
        }
        if let Some(paths) = document["paths"].as_object() {
            self.openapi_paths.extend(paths.clone());
        }
        if let Some(schemas) = document["components"]["schemas"].as_object() {
            self.openapi_components.extend(schemas.clone());
        }
        self.routers.push(router(state.clone(), viewset));
    }

    fn schema(&self) -> SchemaSet {
        self.schemas
            .iter()
            .cloned()
            .fold(SchemaSet::new(), SchemaSet::merge)
    }

    pub fn api_endpoints(&self) -> &[ApiEndpoint] {
        &self.api_endpoints
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
        let mut context = ModuleContext::new(self.state.clone());
        for module in self.apps.iter() {
            context.schemas.push(module.schema());
            context.current_app = module.name();
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
        let mut root = Router::new();
        for route in context.root_routers {
            root = root.merge(route);
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

        root = root.nest(&self.api_prefix, api);
        for module in self.apps.iter() {
            root = module.middleware(root, &self.state);
        }

        Ok(root.layer(axum::Extension(self.state)))
    }
}
