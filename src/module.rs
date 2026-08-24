use std::sync::Arc;

use axum::{Json, Router, response::Html, routing::get};
use che_orm::SchemaSet;
use serde_json::{Map, Value, json};

use crate::{
    AppError, AppResult, AppState, CrudViewSet, FilterSetSpec, Model, ModelSerializer,
    SignalAccess, ViewAction, ViewSet, ViewSetConfig, openapi_column_schema, openapi_json_for,
    router,
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
    fn openapi(&self, _state: &AppState) -> Value {
        json!({})
    }
}

#[derive(Debug, Clone)]
pub struct ApiEndpoint {
    pub app_name: &'static str,
    pub model_name: String,
    pub resource: String,
    pub fields: Vec<che_orm::SerializerField>,
    pub columns: Vec<ApiColumn>,
    pub filters: Vec<ApiFilter>,
    pub extensions: Vec<crate::ApiExtension>,
}

#[derive(Debug, Clone)]
pub struct ApiSignal {
    pub name: String,
    pub access: SignalAccess,
}

#[derive(Debug, Clone)]
pub struct ApiColumn {
    pub name: &'static str,
    pub nullable: bool,
    pub has_default: bool,
    pub choices: Option<Vec<&'static str>>,
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

    #[allow(clippy::should_implement_trait)]
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

    pub fn api_signals(&self, state: AppState) -> Vec<ApiSignal> {
        let mut context = ModuleContext::new(state.clone());
        for module in self.iter() {
            context.current_app = module.name();
            module.init(&mut context);
        }
        state
            .signals()
            .public_signals()
            .into_iter()
            .map(|(name, access)| ApiSignal { name, access })
            .collect()
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
    registration_errors: Vec<String>,
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

    pub fn signal(&mut self, signal: impl Into<String>, access: SignalAccess) {
        let state = self
            .state
            .as_ref()
            .expect("module context is not initialized");
        state.signals().declare(signal, access);
    }

    pub fn model<M: Model>(&mut self) {
        self.schemas.push(SchemaSet::new().model::<M>());
    }

    pub fn viewset<M, S>(&mut self, viewset: CrudViewSet<M, S>)
    where
        M: Model + Send + Sync + 'static,
        S: ModelSerializer<Model = M, Input = M>
            + che_orm::ModelWriteSerializer<Model = M>
            + serde::Serialize
            + Send
            + Sync
            + 'static,
    {
        self.viewset_with(viewset);
    }

    pub fn viewset_with<V>(&mut self, viewset: V)
    where
        V: ViewSet,
        V::Serializer: serde::Serialize,
    {
        let path = viewset.path();
        let mut config = ViewSetConfig::<V>::new(path);
        if let Err(error) = viewset.configure(&mut config) {
            self.registration_errors.push(error.to_string());
        }
        let (extension_router, extensions, extension_schemas, _hooks) = config.into_parts();
        self.api_endpoints.push(ApiEndpoint {
            app_name: self.current_app,
            model_name: std::any::type_name::<V::Model>()
                .rsplit("::")
                .next()
                .unwrap_or("Model")
                .to_owned(),
            resource: path.trim_matches('/').to_owned(),
            fields: V::Serializer::fields().to_vec(),
            columns: V::Model::schema()
                .columns
                .iter()
                .map(|column| ApiColumn {
                    name: column.name,
                    nullable: column.nullable,
                    has_default: column.default.is_some() || column.auto_now || column.auto_now_add,
                    choices: column.choices.clone(),
                })
                .collect(),
            filters: V::FilterSet::default()
                .filters()
                .iter()
                .map(|filter| ApiFilter {
                    name: filter.name,
                    source: filter.source(),
                    lookup: filter.lookup(),
                })
                .collect(),
            extensions: extensions.clone(),
        });
        if let (Some(signal), Some(access)) = (
            viewset.signal_name(ViewAction::Create),
            viewset.signal_access(ViewAction::Create),
        ) {
            self.signal(signal, access);
        }
        if let (Some(signal), Some(access)) = (
            viewset.signal_name(ViewAction::Update),
            viewset.signal_access(ViewAction::Update),
        ) {
            self.signal(signal, access);
        }
        if let (Some(signal), Some(access)) = (
            viewset.signal_name(ViewAction::Delete),
            viewset.signal_access(ViewAction::Delete),
        ) {
            self.signal(signal, access);
        }
        let state = self
            .state
            .as_ref()
            .expect("module context is not initialized");
        let document = openapi_json_for::<V::Model, V::Serializer>(path);
        let mut document = document;
        add_filter_parameters::<V>(&mut document, &viewset);
        let extension_openapi = crate::rest::extensions::extension_openapi(&extensions);
        if let Some(action_paths) = extension_openapi.get("paths").and_then(Value::as_object) {
            if let Some(paths) = document["paths"].as_object_mut() {
                for (path, item) in action_paths {
                    let target = paths.entry(path.clone()).or_insert_with(|| json!({}));
                    target
                        .as_object_mut()
                        .expect("OpenAPI path items are objects")
                        .extend(
                            item.as_object()
                                .expect("OpenAPI path item is an object")
                                .clone(),
                        );
                }
            }
        }
        if let Some(schemas) = document["components"]["schemas"].as_object_mut() {
            for schema in extension_schemas {
                if let Some(existing) = schemas.get(schema.name) {
                    if existing != &schema.openapi {
                        self.registration_errors
                            .push(format!("conflicting OpenAPI schema `{}`", schema.name));
                    }
                } else {
                    schemas.insert(schema.name.to_owned(), schema.openapi);
                }
            }
        }
        if let Some(paths) = document["paths"].as_object() {
            self.openapi_paths.extend(paths.clone());
        }
        if let Some(schemas) = document["components"]["schemas"].as_object() {
            self.openapi_components.extend(schemas.clone());
        }
        self.routers
            .push(router(state.clone(), viewset, extension_router));
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

    #[test]
    fn lifecycle_signals_are_opt_in() {
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap());
        let mut context = ModuleContext::new(state.clone());

        context.viewset_with(crate::auth::AdminUserViewSet);

        assert!(state.signals().public_signals().is_empty());
    }

    #[test]
    fn viewset_path_drives_metadata_and_openapi() {
        let state = AppState::from_database(che_orm::Database::connect_in_memory().unwrap());
        let mut context = ModuleContext::new(state);

        context.viewset_with(crate::auth::AdminUserViewSet);

        assert_eq!(context.api_endpoints()[0].resource, "auth/users");
        assert!(context.openapi_paths.contains_key("/auth/users/"));
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

        if !context.registration_errors.is_empty() {
            return Err(AppError::BadRequest(context.registration_errors.join("; ")));
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

        let mut openapi_paths = context.openapi_paths;
        let mut openapi_components = Map::new();
        openapi_components.insert(
            "schemas".to_owned(),
            Value::Object(context.openapi_components),
        );
        for module in self.apps.iter() {
            let metadata = module.openapi(&self.state);
            if let Some(paths) = metadata.get("paths").and_then(Value::as_object) {
                openapi_paths.extend(paths.clone());
            }
            if let Some(components) = metadata.get("components").and_then(Value::as_object) {
                for (name, values) in components {
                    if name == "schemas" {
                        if let Some(target) = openapi_components
                            .get_mut("schemas")
                            .and_then(Value::as_object_mut)
                        {
                            if let Some(values) = values.as_object() {
                                target.extend(values.clone());
                            }
                        }
                    } else {
                        openapi_components.insert(name.clone(), values.clone());
                    }
                }
            }
        }
        let openapi = json!({
            "openapi": "3.0.3",
            "info": { "title": "che-rest API", "version": "0.1.0" },
            "servers": [{"url": self.api_prefix.clone()}],
            "paths": openapi_paths,
            "components": openapi_components,
            "x-che-rest-signals": self.state.signals().public_signals().into_iter().map(|(name, access)| json!({"name": name, "access": format!("{:?}", access)})).collect::<Vec<_>>(),
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
        let swagger_html = swagger_html(&self.state.config.auth.session.csrf_cookie_name);
        root = root.route(
            &format!("{}/", self.api_prefix.trim_end_matches('/')),
            get(move || {
                let html = swagger_html.clone();
                async move { Html(html) }
            }),
        );
        for module in self.apps.iter() {
            root = module.middleware(root, &self.state);
        }

        Ok(root.layer(axum::Extension(self.state)))
    }
}

fn swagger_html(csrf_cookie_name: &str) -> String {
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>che-rest API</title>
  <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script>
    window.ui = SwaggerUIBundle({{
      url: "./openapi.json",
      dom_id: "#swagger-ui",
      withCredentials: true,
      requestInterceptor: (request) => {{
        if (["POST", "PUT", "PATCH", "DELETE"].includes(request.method)) {{
          const token = document.cookie.split("; ").find((cookie) => cookie.startsWith("{csrf_cookie_name}="));
          if (token) request.headers["X-CSRF-Token"] = token.split("=").slice(1).join("=");
        }}
        return request;
      }}
    }});
  </script>
</body>
</html>"##
    )
}

fn add_filter_parameters<V: ViewSet>(document: &mut Value, viewset: &V) {
    let Some(parameters) = document
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .and_then(|paths| paths.values_mut().next())
        .and_then(Value::as_object_mut)
        .and_then(|path| path.get_mut("get"))
        .and_then(Value::as_object_mut)
        .map(|get| get.entry("parameters").or_insert_with(|| json!([])))
    else {
        return;
    };
    let Some(parameters) = parameters.as_array_mut() else {
        return;
    };
    for filter in viewset.filterset().filters().iter() {
        let name = match filter.lookup() {
            crate::Lookup::Exact => filter.name.to_owned(),
            crate::Lookup::Contains => return_filter_name(filter.name, "__contains"),
            crate::Lookup::Gt => return_filter_name(filter.name, "__gt"),
            crate::Lookup::Gte => return_filter_name(filter.name, "__gte"),
            crate::Lookup::Lt => return_filter_name(filter.name, "__lt"),
            crate::Lookup::Lte => return_filter_name(filter.name, "__lte"),
        };
        let model_schema = V::Model::schema();
        let column = model_schema
            .columns
            .iter()
            .find(|column| column.name == filter.source());
        let mut schema = column
            .map(openapi_column_schema)
            .unwrap_or_else(|| json!({"type": "string"}));
        schema["nullable"] = json!(false);
        parameters.push(json!({"name": name, "in": "query", "required": false, "schema": schema}));
    }
    parameters.push(
        json!({"name": "limit", "in": "query", "required": false, "schema": {"type": "integer"}}),
    );
    parameters.push(
        json!({"name": "offset", "in": "query", "required": false, "schema": {"type": "integer"}}),
    );
    parameters.push(
        json!({"name": "ordering", "in": "query", "required": false, "schema": {"type": "string"}}),
    );
}

fn return_filter_name(name: &'static str, suffix: &str) -> String {
    format!("{name}{suffix}")
}
