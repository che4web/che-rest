use axum::{Extension, Router};
use che_orm::{Model, ModelSchema, create_table_sql};

use crate::{error::AppResult, state::AppState};

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

#[derive(Debug, Default)]
pub struct ModuleContext {
    routers: Vec<Router>,
    sql: Vec<String>,
    schemas: Vec<ModelSchema>,
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

    pub fn model_schemas(&self) -> &[ModelSchema] {
        &self.schemas
    }
}

pub struct Server {
    state: AppState,
    modules: Vec<Box<dyn AppModule>>,
}

impl Server {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            modules: Vec::new(),
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

    pub async fn build(self) -> AppResult<Router> {
        let mut ctx = ModuleContext::new();

        for module in &self.modules {
            module.init(&mut ctx);
        }

        for sql in ctx.sql {
            self.state.db().apply_sql(&sql).await?;
        }

        let mut router = Router::new();
        for module_router in ctx.routers {
            router = router.merge(module_router);
        }

        Ok(router.layer(Extension(self.state)))
    }
}
