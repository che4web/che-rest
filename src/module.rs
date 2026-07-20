use axum::{Extension, Router};
use che_orm::{Model, create_table_sql};

use crate::{error::AppResult, state::AppState};

pub trait AppModule {
    fn name(&self) -> &'static str;
    fn init(&self, ctx: &mut ModuleContext);
}

#[derive(Debug, Default)]
pub struct ModuleContext {
    routers: Vec<Router>,
    sql: Vec<String>,
}

impl ModuleContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_table<M>(&mut self)
    where
        M: Model,
    {
        self.sql.push(create_table_sql::<M>());
    }

    pub fn route(&mut self, router: Router) {
        self.routers.push(router);
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
