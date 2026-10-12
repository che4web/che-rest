use std::sync::Arc;

use che_orm::{
    DatabaseQuery, Migration, MigrationGraph, MigrationId, MigrationOperation, Model,
    ModelSerializer, SchemaSet, TableState, Uuid,
};
use che_rest::{
    AppModule, AppResult, AppState, CurrentPrincipal, FilterSet, InstalledApps, IsAuthenticated,
    ModuleContext, SyncEndpoints, SyncViewSet, ViewSet, ViewSetConfig,
};
use che_sync::{AccessPolicy, AllowAll, Field, Registry, SchemaRole, SyncModel};

#[derive(Debug, Model)]
#[orm(table = "projects")]
pub struct Project {
    #[orm(primary_key)]
    pub id: Uuid,
    pub title: String,
    #[orm(default = "0")]
    pub revision: i64,
    #[orm(auto_now)]
    pub updated_at: che_orm::time::OffsetDateTime,
    pub deleted_at: Option<che_orm::time::OffsetDateTime>,
    #[orm(default = "''")]
    pub sync_scope: String,
}
impl Default for Project {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            title: String::new(),
            revision: 0,
            updated_at: che_orm::time::OffsetDateTime::UNIX_EPOCH,
            deleted_at: None,
            sync_scope: String::new(),
        }
    }
}
impl SyncModel for Project {
    const SYNC_NAME: &'static str = "projects.project";
    fn sync_fields() -> Vec<Field> {
        vec![
            Field::server_owned("id", "id"),
            Field::writable("title", "title"),
        ]
    }
}
#[derive(ModelSerializer)]
#[serializer(model = Project)]
pub struct ProjectSerializer {
    #[serializer(read_only)]
    pub id: Uuid,
    pub title: String,
    #[serializer(read_only)]
    pub revision: i64,
    #[serializer(read_only)]
    pub updated_at: che_orm::time::OffsetDateTime,
    #[serializer(read_only)]
    pub deleted_at: Option<che_orm::time::OffsetDateTime>,
}

#[derive(Debug, Model)]
#[orm(table = "todos")]
pub struct Todo {
    #[orm(primary_key)]
    pub id: Uuid,
    pub title: String,
    #[orm(default = "false")]
    pub done: bool,
    #[orm(foreign_key = Project, on_delete = "set null")]
    pub project_id: Option<Uuid>,
    #[orm(default = "0")]
    pub revision: i64,
    #[orm(auto_now)]
    pub updated_at: che_orm::time::OffsetDateTime,
    pub deleted_at: Option<che_orm::time::OffsetDateTime>,
    #[orm(default = "''")]
    pub sync_scope: String,
}
impl Default for Todo {
    fn default() -> Self {
        Self {
            id: Uuid::nil(),
            title: String::new(),
            done: false,
            project_id: None,
            revision: 0,
            updated_at: che_orm::time::OffsetDateTime::UNIX_EPOCH,
            deleted_at: None,
            sync_scope: String::new(),
        }
    }
}

impl SyncModel for Todo {
    const SYNC_NAME: &'static str = "todo.todo";
    fn sync_fields() -> Vec<Field> {
        vec![
            Field::server_owned("id", "id"),
            Field::writable("title", "title"),
            Field::writable("done", "done"),
            Field::writable("project", "project_id"),
        ]
    }
}

#[derive(ModelSerializer)]
#[serializer(model = Todo)]
pub struct TodoSerializer {
    #[serializer(read_only)]
    pub id: Uuid,
    pub title: String,
    pub done: bool,
    #[serializer(foreign_key = Project, relation = TodoProjectRelation)]
    pub project_id: Option<Uuid>,
    #[serializer(read_only)]
    pub revision: i64,
    #[serializer(read_only)]
    pub updated_at: che_orm::time::OffsetDateTime,
    #[serializer(read_only)]
    pub deleted_at: Option<che_orm::time::OffsetDateTime>,
}

pub fn registry() -> AppResult<Registry> {
    Ok(Registry::new().register::<Project>()?.register::<Todo>()?)
}

#[derive(Clone)]
pub struct TodoViewSet;

impl ViewSet for TodoViewSet {
    type Model = Todo;
    type Serializer = TodoSerializer;
    type ListSerializer = TodoSerializer;
    type QuerySet = DatabaseQuery<Todo>;
    type FilterSet = FilterSet<Todo>;
    type Permission = IsAuthenticated;
    fn path(&self) -> &'static str {
        "/todos"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Todo::query().filter(Todo::DELETED_AT.is_null()))
    }
    fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
        config.extend(SyncEndpoints::new(self.clone()))
    }
}

impl SyncViewSet for TodoViewSet {
    fn sync_registry(&self) -> AppResult<Registry> {
        registry()
    }
    fn sync_scope(&self, _: &AppState, _: &CurrentPrincipal) -> AppResult<String> {
        // This demo deliberately shares one board among all authenticated users.
        Ok("todo-board".into())
    }
    fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
        Arc::new(AllowAll)
    }
}

pub struct TodoApp;
impl AppModule for TodoApp {
    fn name(&self) -> &'static str {
        "todo"
    }
    fn schema(&self) -> SchemaSet {
        SchemaSet::new().model::<Todo>()
    }
    fn sync_registry(&self) -> Option<Registry> {
        Some(registry().expect("valid Todo sync registry"))
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(TodoViewSet);
    }
}

#[derive(Clone)]
pub struct ProjectViewSet;
impl ViewSet for ProjectViewSet {
    type Model = Project;
    type Serializer = ProjectSerializer;
    type ListSerializer = ProjectSerializer;
    type QuerySet = DatabaseQuery<Project>;
    type FilterSet = FilterSet<Project>;
    type Permission = IsAuthenticated;
    fn path(&self) -> &'static str {
        "/projects"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Project::query().filter(Project::DELETED_AT.is_null()))
    }
    fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
        config.extend(SyncEndpoints::new(self.clone()))
    }
}
impl SyncViewSet for ProjectViewSet {
    fn sync_registry(&self) -> AppResult<Registry> {
        registry()
    }
    fn sync_scope(&self, _: &AppState, _: &CurrentPrincipal) -> AppResult<String> {
        Ok("todo-board".into())
    }
    fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
        Arc::new(AllowAll)
    }
}
pub struct ProjectApp;
impl AppModule for ProjectApp {
    fn name(&self) -> &'static str {
        "projects"
    }
    fn schema(&self) -> SchemaSet {
        SchemaSet::new().model::<Project>()
    }
    fn sync_registry(&self) -> Option<Registry> {
        Some(registry().expect("valid Project sync registry"))
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(ProjectViewSet);
    }
}

pub fn apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(ProjectApp)
        .add(TodoApp)
}

/// Local demo setup. A deployed application freezes these operations in its
/// compiled migration history instead of rebuilding historical migrations.
pub async fn database(role: SchemaRole) -> AppResult<che_orm::Database> {
    let db = che_orm::Database::connect_in_memory().await?;
    let mut operations = vec![
        MigrationOperation::CreateTable {
            table: TableState::from(&Project::schema()),
        },
        MigrationOperation::CreateTable {
            table: TableState::from(&Todo::schema()),
        },
    ];
    operations.extend(registry()?.migration_operations(role));
    let mut migrations = if matches!(role, SchemaRole::Server) {
        che_rest::auth::migrations()
    } else {
        vec![]
    };
    migrations.push(Migration::new(
        MigrationId {
            app: "todo".into(),
            name: "0001_initial".into(),
        },
        vec![],
        operations,
    ));
    db.migrate(
        MigrationGraph::new(migrations)
            .map_err(|error| che_rest::AppError::BadRequest(error.to_string()))?,
    )
    .await?;
    Ok(db)
}

pub async fn demo_state() -> AppResult<(AppState, String)> {
    use che_rest::auth::{AuthToken, User, generate_token, token_hash};
    let state = AppState::from_database(database(SchemaRole::Server).await?);
    let user = state
        .database()
        .create::<User>()
        .set(User::USERNAME, "todo-demo")
        .set(User::PASSWORD_HASH, "unused-token-only-demo")
        .execute()
        .await?;
    let token = generate_token();
    state
        .database()
        .create::<AuthToken>()
        .set(AuthToken::USER_ID, user.id)
        .set(AuthToken::KEY_HASH, token_hash(&token))
        .execute()
        .await?;
    Ok((state, token))
}
