use std::sync::{Arc, Weak};

use axum::{Extension, Json, Router, middleware, response::Html, routing::get};
use che_orm::{FieldType, Model, ModelEvent, ModelSchema, SqliteModel, create_table_sql};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

use crate::{
    auth,
    error::AppResult,
    events::{CommandHandler, EventHandler},
    filters::{FilterSet, FilterSetSpec},
    openapi,
    serializer::{ModelSerializer, Serializer},
    state::{AppState, AppStateInner},
    views::{ModelViewSet, ViewSet},
};

pub trait AppModule {
    fn name(&self) -> &'static str;
    fn init(&self, ctx: &mut ModuleContext);
}

#[derive(Default)]
pub struct InstalledApps {
    modules: Vec<Box<dyn AppModule>>,
}

impl InstalledApps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<M>(mut self, module: M) -> Self
    where
        M: AppModule + 'static,
    {
        self.modules.push(Box::new(module));
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn AppModule> {
        self.modules.iter().map(Box::as_ref)
    }

    pub fn find(&self, name: &str) -> Option<&dyn AppModule> {
        self.iter().find(|module| module.name() == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.iter().map(AppModule::name)
    }

    fn into_modules(self) -> Vec<Box<dyn AppModule>> {
        self.modules
    }
}

#[derive(Default)]
pub struct ModuleContext {
    routers: Vec<Router>,
    sql: Vec<String>,
    schemas: Vec<ModelSchema>,
    api_endpoints: Vec<ApiEndpoint>,
    auth_enabled: bool,
    command_handlers: Vec<(String, Arc<dyn CommandHandler>)>,
    event_handlers: Vec<(String, Arc<dyn EventHandler>)>,
    post_save_registrations: Vec<Box<dyn PostSaveRegistration>>,
}

trait PostSaveRegistration: Send + Sync {
    fn install(&self, state: &AppState);
}

struct ModelPostSaveRegistration<M: Model> {
    event_name: String,
    _model: std::marker::PhantomData<M>,
}

impl<M: Model> PostSaveRegistration for ModelPostSaveRegistration<M> {
    fn install(&self, state: &AppState) {
        let mut receiver = state.db().signals().subscribe::<M>();
        let bridge = PostSaveBridge {
            state: state.downgrade(),
            event_name: self.event_name.clone(),
            model: rust_type_name::<M>(),
        };

        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(ModelEvent::PostSave(event)) => bridge.handle(event).await,
                    Ok(ModelEvent::PostUpdate(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        tracing::warn!(
                            model = %bridge.model,
                            event = %bridge.event_name,
                            count,
                            "post_save bridge lagged"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

struct PostSaveBridge {
    state: Weak<AppStateInner>,
    event_name: String,
    model: String,
}

impl PostSaveBridge {
    async fn handle(&self, event: che_orm::PostSaveEvent) {
        let Some(inner) = self.state.upgrade() else {
            return;
        };
        let state = AppState::from_inner(inner);
        state.events().emit(crate::AppEvent {
            name: self.event_name.clone(),
            user: None,
            payload: serde_json::json!({
                "model": self.model.clone(),
                "created": event.created,
                "object": event.object,
            }),
        });
    }
}

#[derive(Debug, Clone)]
pub struct ApiEndpoint {
    pub model_name: String,
    pub path: String,
    pub resource: String,
    pub fields: Vec<ApiField>,
    pub filters: Vec<ApiFilter>,
}

#[derive(Debug, Clone)]
pub struct ApiField {
    pub name: String,
    pub source: String,
    pub ty: FieldType,
    pub related_model: Option<String>,
    pub read_only: bool,
    pub write_only: bool,
    pub required: bool,
    pub nullable: bool,
    pub has_default: bool,
    pub choices: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ApiFilter {
    pub name: String,
    pub source: String,
    pub ty: FieldType,
    pub nullable: bool,
}

impl ModuleContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn model<M>(&mut self)
    where
        M: Model,
    {
        self.sql.push(create_table_sql::<M>());
        self.schemas.push(ModelSchema::from_model::<M>());
    }

    pub fn create_table<M>(&mut self)
    where
        M: Model,
    {
        self.model::<M>();
    }

    pub fn route(&mut self, router: Router) {
        self.routers.push(router);
    }

    pub fn viewset<M>(
        &mut self,
        base_path: &'static str,
        serializer: ModelSerializer<M>,
        filterset: FilterSet<M>,
    ) where
        M: SqliteModel<Id = i64>,
    {
        self.model::<M>();
        self.api_endpoints
            .push(api_endpoint::<M>(base_path, serializer, filterset));
        self.route(ModelViewSet::<M>::router(base_path, serializer, filterset));
    }

    pub fn viewset_with<V>(&mut self, base_path: &'static str, viewset: V)
    where
        V: ViewSet,
    {
        let serializer = viewset.serializer().model_serializer();
        let filterset = viewset.filterset().filterset();

        self.model::<V::Model>();
        self.api_endpoints
            .push(api_endpoint::<V::Model>(base_path, serializer, filterset));
        self.route(ModelViewSet::<V::Model, V>::router_with(base_path, viewset));
    }

    pub fn enable_auth(&mut self) {
        self.auth_enabled = true;
    }

    pub fn command_handler<H>(&mut self, name: impl Into<String>, handler: H)
    where
        H: CommandHandler,
    {
        self.command_handlers.push((name.into(), Arc::new(handler)));
    }

    pub fn auth_enabled(&self) -> bool {
        self.auth_enabled
    }

    pub fn event_handler<H>(&mut self, name: impl Into<String>, handler: H)
    where
        H: EventHandler,
    {
        self.event_handlers.push((name.into(), Arc::new(handler)));
    }

    fn install_event_handlers(&self, state: &AppState) {
        for (name, handler) in &self.event_handlers {
            let mut receiver = state.events().subscribe(name.clone());
            let handler = handler.clone();
            let event_name = name.clone();
            let state = state.downgrade();
            tokio::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            let Some(inner) = state.upgrade() else {
                                break;
                            };
                            let state = AppState::from_inner(inner);
                            match AssertUnwindSafe(handler.handle(&state, event))
                                .catch_unwind()
                                .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => tracing::error!(
                                    event = %event_name,
                                    error = %error,
                                    "event handler failed"
                                ),
                                Err(_) => tracing::error!(
                                    event = %event_name,
                                    "event handler panicked"
                                ),
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            tracing::warn!(event = %event_name, count, "event handler lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
    }

    pub fn post_save<M>(&mut self, event_name: impl Into<String>)
    where
        M: Model,
    {
        self.post_save_registrations
            .push(Box::new(ModelPostSaveRegistration::<M> {
                event_name: event_name.into(),
                _model: std::marker::PhantomData,
            }));
    }

    fn command_handlers(
        &self,
    ) -> AppResult<std::collections::HashMap<String, Arc<dyn CommandHandler>>> {
        let mut commands = std::collections::HashMap::new();
        for (name, handler) in &self.command_handlers {
            if commands.insert(name.clone(), handler.clone()).is_some() {
                return Err(crate::AppError::BadRequest(format!(
                    "duplicate command handler: {name}"
                )));
            }
        }

        Ok(commands)
    }

    pub fn model_schemas(&self) -> &[ModelSchema] {
        &self.schemas
    }

    pub fn api_endpoints(&self) -> &[ApiEndpoint] {
        &self.api_endpoints
    }
}

fn api_endpoint<M>(
    base_path: &'static str,
    serializer: ModelSerializer<M>,
    filterset: FilterSet<M>,
) -> ApiEndpoint
where
    M: SqliteModel<Id = i64>,
{
    let fields = serializer
        .fields()
        .iter()
        .filter_map(|field| {
            let model_field = M::fields()
                .iter()
                .find(|model_field| model_field.db_name == field.source)?;
            Some(ApiField {
                name: field.name.to_string(),
                source: field.source.to_string(),
                ty: model_field.ty,
                related_model: field
                    .relation
                    .map(|relation| relation.model_name().to_string()),
                read_only: field.read_only,
                write_only: field.write_only,
                required: field.required,
                nullable: field.nullable || model_field.nullable,
                has_default: field.has_default() || model_field.default.is_some(),
                choices: model_field
                    .choices
                    .map(|choices| choices.iter().map(|choice| choice.to_string()).collect()),
            })
        })
        .collect();

    let filters = filterset
        .filters()
        .iter()
        .filter_map(|filter| {
            let model_field = M::fields()
                .iter()
                .find(|model_field| model_field.db_name == filter.source)?;
            Some(ApiFilter {
                name: filter.query_name(),
                source: filter.source.to_string(),
                ty: model_field.ty,
                nullable: model_field.nullable,
            })
        })
        .collect();

    ApiEndpoint {
        model_name: rust_type_name::<M>(),
        path: base_path.to_string(),
        resource: base_path.trim_matches('/').to_string(),
        fields,
        filters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCommand;

    #[async_trait::async_trait]
    impl CommandHandler for TestCommand {
        async fn handle(&self, _state: &AppState, _command: crate::Command) -> AppResult<()> {
            Ok(())
        }
    }

    #[test]
    fn duplicate_command_handlers_are_rejected() {
        let mut context = ModuleContext::new();
        context.command_handler("test.command", TestCommand);
        context.command_handler("test.command", TestCommand);

        let error = match context.command_handlers() {
            Ok(_) => panic!("duplicate command handlers were accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("duplicate command handler"));
    }
}

fn rust_type_name<M>() -> String {
    std::any::type_name::<M>()
        .rsplit("::")
        .next()
        .unwrap_or("Model")
        .to_string()
}

pub struct Server {
    state: AppState,
    modules: Vec<Box<dyn AppModule>>,
    api_prefix: String,
    openapi_title: String,
    openapi_version: String,
    swagger_ui_enabled: bool,
}

impl Server {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            modules: Vec::new(),
            api_prefix: "/api".to_string(),
            openapi_title: "che-rest API".to_string(),
            openapi_version: "0.1.0".to_string(),
            swagger_ui_enabled: true,
        }
    }

    pub fn register<M>(mut self, module: M) -> Self
    where
        M: AppModule + 'static,
    {
        self.modules.push(Box::new(module));
        self
    }

    pub fn install(mut self, apps: InstalledApps) -> Self {
        self.modules.extend(apps.into_modules());
        self
    }

    pub fn api_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.api_prefix = prefix.into();
        self
    }

    pub fn openapi_title(mut self, title: impl Into<String>) -> Self {
        self.openapi_title = title.into();
        self
    }

    pub fn openapi_version(mut self, version: impl Into<String>) -> Self {
        self.openapi_version = version.into();
        self
    }

    pub fn swagger_ui(mut self, enabled: bool) -> Self {
        self.swagger_ui_enabled = enabled;
        self
    }

    pub async fn build(self) -> AppResult<Router> {
        let mut ctx = ModuleContext::new();

        for module in &self.modules {
            module.init(&mut ctx);
        }

        let auth_enabled = ctx.auth_enabled();
        let api_endpoints = ctx.api_endpoints().to_vec();

        let state = self.state;
        state.events().configure_commands(ctx.command_handlers()?);

        for sql in &ctx.sql {
            state.db().apply_sql(&sql).await?;
        }

        for registration in &ctx.post_save_registrations {
            registration.install(&state);
        }
        ctx.install_event_handlers(&state);

        let openapi_spec = openapi::openapi_json(
            &api_endpoints,
            openapi::OpenApiOptions {
                title: self.openapi_title.clone(),
                version: self.openapi_version.clone(),
                api_prefix: self.api_prefix.clone(),
            },
        );
        let openapi_json = openapi_spec.clone();
        let mut docs_router = Router::new().route(
            "/openapi.json",
            get(move || async move { Json(openapi_json.clone()) }),
        );

        if self.swagger_ui_enabled {
            let openapi_json_url =
                format!("{}/openapi.json", self.api_prefix.trim_end_matches('/'));
            let swagger_html = openapi::swagger_ui_html(&openapi_json_url, &self.openapi_title);
            docs_router =
                docs_router.route("/", get(move || async move { Html(swagger_html.clone()) }));
        }

        let mut app_router = Router::new();
        for module_router in ctx.routers {
            app_router = app_router.merge(module_router);
        }

        if auth_enabled {
            app_router = app_router.layer(middleware::from_fn(auth::auth_middleware));
        }

        let api_router = docs_router.merge(app_router);

        let mut router = Router::new().nest(&self.api_prefix, api_router);

        if self.swagger_ui_enabled {
            let openapi_json_url =
                format!("{}/openapi.json", self.api_prefix.trim_end_matches('/'));
            let swagger_html = openapi::swagger_ui_html(&openapi_json_url, &self.openapi_title);
            let swagger_slash_path = format!("{}/", self.api_prefix.trim_end_matches('/'));
            router = router.route(
                &swagger_slash_path,
                get(move || async move { Html(swagger_html.clone()) }),
            );
        }

        if auth_enabled {
            router = router.merge(auth::views::routes());
        }

        Ok(router.layer(Extension(state)))
    }
}
