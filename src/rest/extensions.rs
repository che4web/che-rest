use std::{collections::BTreeMap, marker::PhantomData, sync::Arc};

use axum::{Router, handler::Handler, http::StatusCode, routing};
use serde_json::{Map, Value, json};

use super::{ViewSet, router::ViewAction};
use crate::{AppError, AppResult, AppState, CurrentPrincipal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn openapi_key(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiSchemaDefinition {
    pub name: &'static str,
    pub openapi: Value,
    pub typescript: &'static str,
}

pub trait ApiSchema {
    fn api_schema() -> ApiSchemaDefinition;
}

#[derive(Debug, Clone)]
pub struct ClientMethod {
    pub namespace: &'static str,
    pub name: &'static str,
}

#[derive(Debug, Clone)]
pub struct OperationSpec {
    pub operation_id: String,
    pub summary: Option<String>,
    pub request: Option<ApiSchemaDefinition>,
    pub response: Option<(u16, ApiSchemaDefinition)>,
    pub errors: Vec<u16>,
    pub authenticated: bool,
    pub client: Option<ClientMethod>,
}

impl OperationSpec {
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            summary: None,
            request: None,
            response: None,
            errors: Vec::new(),
            authenticated: false,
            client: None,
        }
    }

    pub fn summary(mut self, value: impl Into<String>) -> Self {
        self.summary = Some(value.into());
        self
    }

    pub fn request<T: ApiSchema>(mut self) -> Self {
        self.request = Some(T::api_schema());
        self
    }

    pub fn response<T: ApiSchema>(mut self, status: StatusCode) -> Self {
        self.response = Some((status.as_u16(), T::api_schema()));
        self
    }

    pub fn error(mut self, status: StatusCode) -> Self {
        self.errors.push(status.as_u16());
        self
    }

    pub fn authenticated(mut self) -> Self {
        self.authenticated = true;
        self
    }

    pub fn client(mut self, namespace: &'static str, name: &'static str) -> Self {
        self.client = Some(ClientMethod { namespace, name });
        self
    }
}

#[derive(Debug, Clone)]
pub struct ApiOperation {
    pub extension_id: &'static str,
    pub operation_id: String,
    pub method: HttpMethod,
    pub path: String,
    pub request: Option<ApiSchemaDefinition>,
    pub response: Option<(u16, ApiSchemaDefinition)>,
    pub client: Option<ClientMethod>,
    pub openapi: Value,
}

#[derive(Debug, Clone)]
pub struct ApiExtension {
    pub id: &'static str,
    pub operations: Vec<ApiOperation>,
}

pub trait MutationHook<V: ViewSet>: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn after_commit(
        &self,
        _state: &AppState,
        _current: Option<&CurrentPrincipal>,
        _action: ViewAction,
        _model: &V::Model,
    ) -> AppResult<()> {
        Ok(())
    }
}

pub trait ViewSetExtension<V>: Clone + Send + Sync + 'static
where
    V: ViewSet,
{
    const ID: &'static str;

    fn install(self, context: &mut ExtensionContext<'_, V>) -> AppResult<()>;
}

pub struct ViewSetConfig<V: ViewSet> {
    path: String,
    router: Router,
    extensions: Vec<ApiExtension>,
    schemas: BTreeMap<&'static str, ApiSchemaDefinition>,
    operation_keys: BTreeMap<(String, HttpMethod), String>,
    client_keys: BTreeMap<(&'static str, &'static str), String>,
    hooks: Vec<Arc<dyn MutationHook<V>>>,
    _marker: PhantomData<fn() -> V>,
}

impl<V: ViewSet> ViewSetConfig<V> {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.trim_end_matches('/').to_owned(),
            router: Router::new(),
            extensions: Vec::new(),
            schemas: BTreeMap::new(),
            operation_keys: BTreeMap::new(),
            client_keys: BTreeMap::new(),
            hooks: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn extend<E>(&mut self, extension: E) -> AppResult<()>
    where
        E: ViewSetExtension<V>,
    {
        if self.extensions.iter().any(|item| item.id == E::ID) {
            return Err(AppError::BadRequest(format!(
                "extension `{}` is already installed for {}",
                E::ID,
                self.path
            )));
        }
        let mut extension_context = ExtensionContext {
            config: self,
            extension_id: E::ID,
            operations: Vec::new(),
        };
        extension.install(&mut extension_context)?;
        extension_context.config.extensions.push(ApiExtension {
            id: E::ID,
            operations: extension_context.operations,
        });
        Ok(())
    }

    pub fn into_parts(
        self,
    ) -> (
        Router,
        Vec<ApiExtension>,
        Vec<ApiSchemaDefinition>,
        Vec<Arc<dyn MutationHook<V>>>,
    ) {
        (
            self.router,
            self.extensions,
            self.schemas.into_values().collect(),
            self.hooks,
        )
    }
}

pub struct ExtensionContext<'a, V: ViewSet> {
    config: &'a mut ViewSetConfig<V>,
    extension_id: &'static str,
    operations: Vec<ApiOperation>,
}

impl<'a, V: ViewSet> ExtensionContext<'a, V> {
    pub fn route<'route>(
        &'route mut self,
        relative_path: &str,
        configure: impl FnOnce(&mut ExtensionRoute<'route, 'a, V>) -> AppResult<()>,
    ) -> AppResult<()> {
        let path = normalize_path(&self.config.path, relative_path)?;
        let mut route = ExtensionRoute {
            context: self,
            path,
        };
        configure(&mut route)
    }

    pub fn mutation_hook<H: MutationHook<V>>(&mut self, hook: H) -> AppResult<()> {
        if self.config.hooks.iter().any(|item| item.id() == hook.id()) {
            return Err(AppError::BadRequest(format!(
                "mutation hook `{}` is already installed for {}",
                hook.id(),
                self.config.path
            )));
        }
        self.config.hooks.push(Arc::new(hook));
        Ok(())
    }

    fn register_schema(&mut self, schema: ApiSchemaDefinition) -> AppResult<()> {
        if let Some(existing) = self.config.schemas.get(schema.name) {
            if existing.openapi != schema.openapi || existing.typescript != schema.typescript {
                return Err(AppError::BadRequest(format!(
                    "conflicting API schema `{}`",
                    schema.name
                )));
            }
            return Ok(());
        }
        self.config.schemas.insert(schema.name, schema);
        Ok(())
    }

    fn register<H, T>(
        &mut self,
        path: String,
        method: HttpMethod,
        handler: H,
        spec: OperationSpec,
    ) -> AppResult<()>
    where
        H: Handler<T, ()> + Clone + Send + Sync + 'static,
        T: 'static,
    {
        let key = (path.clone(), method);
        if let Some(existing) = self.config.operation_keys.get(&key) {
            return Err(AppError::BadRequest(format!(
                "route conflict for {} {} between `{}` and `{}`",
                method.openapi_key().to_ascii_uppercase(),
                path,
                existing,
                spec.operation_id
            )));
        }
        if let Some(client) = &spec.client {
            let client_key = (client.namespace, client.name);
            if let Some(existing) = self.config.client_keys.get(&client_key) {
                return Err(AppError::BadRequest(format!(
                    "client method conflict for {}.{} between `{}` and `{}`",
                    client.namespace, client.name, existing, spec.operation_id
                )));
            }
            self.config
                .client_keys
                .insert(client_key, spec.operation_id.clone());
        }
        if let Some(schema) = spec.request.clone() {
            self.register_schema(schema)?;
        }
        if let Some((_, schema)) = spec.response.clone() {
            self.register_schema(schema)?;
        }
        let openapi = operation_json(&spec);
        self.config.router = match method {
            HttpMethod::Get => self
                .config
                .router
                .clone()
                .route(&path, routing::get(handler)),
            HttpMethod::Post => self
                .config
                .router
                .clone()
                .route(&path, routing::post(handler)),
            HttpMethod::Put => self
                .config
                .router
                .clone()
                .route(&path, routing::put(handler)),
            HttpMethod::Patch => self
                .config
                .router
                .clone()
                .route(&path, routing::patch(handler)),
            HttpMethod::Delete => self
                .config
                .router
                .clone()
                .route(&path, routing::delete(handler)),
        };
        self.config
            .operation_keys
            .insert(key, spec.operation_id.clone());
        self.operations.push(ApiOperation {
            extension_id: self.extension_id,
            operation_id: spec.operation_id,
            method,
            path,
            request: spec.request,
            response: spec.response,
            client: spec.client,
            openapi,
        });
        Ok(())
    }
}

pub struct ExtensionRoute<'context, 'config, V: ViewSet> {
    context: &'context mut ExtensionContext<'config, V>,
    path: String,
}

macro_rules! route_method {
    ($name:ident, $method:ident) => {
        pub fn $name<H, T>(&mut self, handler: H, spec: OperationSpec) -> AppResult<()>
        where
            H: Handler<T, ()> + Clone + Send + Sync + 'static,
            T: 'static,
        {
            self.context
                .register(self.path.clone(), HttpMethod::$method, handler, spec)
        }
    };
}

impl<V: ViewSet> ExtensionRoute<'_, '_, V> {
    route_method!(get, Get);
    route_method!(post, Post);
    route_method!(put, Put);
    route_method!(patch, Patch);
    route_method!(delete, Delete);
}

fn normalize_path(prefix: &str, relative: &str) -> AppResult<String> {
    if relative.contains("..") {
        return Err(AppError::BadRequest(
            "extension paths may not contain `..`".into(),
        ));
    }
    let relative = relative.trim_matches('/');
    Ok(format!("{}/{}/", prefix.trim_end_matches('/'), relative))
}

fn operation_json(spec: &OperationSpec) -> Value {
    let mut operation = Map::new();
    operation.insert(
        "operationId".into(),
        Value::String(spec.operation_id.clone()),
    );
    if let Some(summary) = &spec.summary {
        operation.insert("summary".into(), Value::String(summary.clone()));
    }
    if let Some(request) = &spec.request {
        operation.insert(
            "requestBody".into(),
            json!({"required": true, "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", request.name)}}}}),
        );
    }
    let mut responses = Map::new();
    if let Some((status, response)) = &spec.response {
        responses.insert(
            status.to_string(),
            json!({"description": "OK", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", response.name)}}}}),
        );
    }
    for status in &spec.errors {
        responses.insert(status.to_string(), json!({"description": "Error"}));
    }
    operation.insert("responses".into(), Value::Object(responses));
    if spec.authenticated {
        operation.insert(
            "security".into(),
            json!([{"TokenAuth": []}, {"CsrfToken": [], "SessionCookie": []}]),
        );
    }
    Value::Object(operation)
}

pub fn extension_openapi(extensions: &[ApiExtension]) -> Value {
    let mut paths = Map::<String, Value>::new();
    for extension in extensions {
        for operation in &extension.operations {
            let path = paths
                .entry(operation.path.clone())
                .or_insert_with(|| json!({}));
            let object = path
                .as_object_mut()
                .expect("OpenAPI path must be an object");
            if !object.contains_key("parameters") {
                let parameters = path_parameters(&operation.path);
                if !parameters.is_empty() {
                    object.insert("parameters".into(), Value::Array(parameters));
                }
            }
            object.insert(
                operation.method.openapi_key().into(),
                operation.openapi.clone(),
            );
        }
    }
    json!({"paths": paths})
}

fn path_parameters(path: &str) -> Vec<Value> {
    path.split('{')
        .skip(1)
        .filter_map(|segment| segment.split('}').next())
        .filter(|name| !name.is_empty())
        .map(|name| {
            let schema = if name == "id" {
                json!({"type": "integer", "format": "int64"})
            } else {
                json!({"type": "string"})
            };
            json!({"in": "path", "name": name, "required": true, "schema": schema})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AllowAny,
        auth::{AdminUserFilterSet, AdminUserSerializer},
    };
    use che_orm::{DatabaseQuery, Model};

    #[derive(Clone, Copy, Default)]
    struct TestViewSet;

    impl ViewSet for TestViewSet {
        type Model = crate::auth::User;
        type Serializer = AdminUserSerializer;
        type QuerySet = DatabaseQuery<crate::auth::User>;
        type FilterSet = AdminUserFilterSet;
        type Permission = AllowAny;

        fn path(&self) -> &'static str {
            "/users"
        }

        fn get_queryset(&self) -> Self::QuerySet {
            DatabaseQuery::new(crate::auth::User::query())
        }
    }

    #[derive(Clone, Copy)]
    struct PingExtension;

    impl ViewSetExtension<TestViewSet> for PingExtension {
        const ID: &'static str = "ping";

        fn install(self, context: &mut ExtensionContext<'_, TestViewSet>) -> AppResult<()> {
            context.route("/{id}/ping/", |route| {
                route.get(
                    || async { "pong" },
                    OperationSpec::new("ping.retrieve")
                        .response::<PingResponse>(StatusCode::OK)
                        .client("ping", "retrieve"),
                )
            })
        }
    }

    struct PingResponse;

    impl ApiSchema for PingResponse {
        fn api_schema() -> ApiSchemaDefinition {
            ApiSchemaDefinition {
                name: "PingResponse",
                openapi: json!({"type": "string"}),
                typescript: "export type PingResponse = string;",
            }
        }
    }

    #[test]
    fn extension_operation_builds_openapi_from_registered_route() {
        let mut config = ViewSetConfig::<TestViewSet>::new("/users");
        config.extend(PingExtension).unwrap();
        let (_, extensions, schemas, _) = config.into_parts();

        assert_eq!(extensions[0].operations[0].path, "/users/{id}/ping/");
        assert_eq!(extensions[0].operations[0].method, HttpMethod::Get);
        assert_eq!(schemas[0].name, "PingResponse");
        let openapi = extension_openapi(&extensions);
        assert!(openapi["paths"]["/users/{id}/ping/"]["get"].is_object());
        assert_eq!(
            openapi["paths"]["/users/{id}/ping/"]["parameters"][0]["name"],
            "id"
        );
    }

    #[test]
    fn duplicate_extension_is_rejected() {
        let mut config = ViewSetConfig::<TestViewSet>::new("/users");
        config.extend(PingExtension).unwrap();
        assert!(config.extend(PingExtension).is_err());
    }
}
