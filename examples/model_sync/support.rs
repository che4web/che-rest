use che_orm::{Database, DatabaseQuery, Model, ModelSerializer, ProjectState, SchemaSet, Uuid};
use che_rest::{
    AppModule, AppResult, AppState, FilterSet, InstalledApps, IsAuthenticated, ModelSyncEndpoints,
    ModuleContext, SyncViewSet, ViewSet, ViewSetConfig,
};
use che_sync::{Registry, SchemaRole};

#[derive(Debug, Clone, Model)]
#[orm(table = "notes", sync, sync_name = "notes.note")]
pub struct Note {
    #[orm(primary_key)]
    pub id: Uuid,
    #[orm(db_column = "caption", sync_name = "title")]
    pub title: String,
    pub done: bool,
    pub document: Option<che_orm::JsonField>,
    pub number: i64,
}

#[derive(ModelSerializer)]
#[serializer(model = Note)]
pub struct NoteSerializer {
    #[serializer(read_only)]
    pub id: Uuid,
    pub title: String,
    pub done: bool,
    pub document: Option<che_orm::JsonField>,
    pub number: i64,
}

pub fn registry(role: SchemaRole) -> Registry {
    Registry::from_schema(&schema(role)).expect("valid note registry")
}
fn schema(role: SchemaRole) -> SchemaSet {
    match role {
        SchemaRole::Client => SchemaSet::new().client_model::<Note>(),
        SchemaRole::Server => SchemaSet::new().server_model::<Note>(),
    }
}

#[derive(Clone)]
pub struct Notes;
impl ViewSet for Notes {
    type Model = Note;
    type Serializer = NoteSerializer;
    type ListSerializer = NoteSerializer;
    type QuerySet = DatabaseQuery<Note>;
    type FilterSet = FilterSet<Note>;
    type Permission = IsAuthenticated;
    fn path(&self) -> &'static str {
        "/notes"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Note::query())
    }
    fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
        config.extend(ModelSyncEndpoints::new(self.clone()))
    }
}
impl SyncViewSet for Notes {
    fn sync_registry(&self) -> AppResult<Registry> {
        Ok(registry(SchemaRole::Server))
    }
}
pub struct NotesApp;
impl AppModule for NotesApp {
    fn name(&self) -> &'static str {
        "notes"
    }
    fn schema(&self) -> SchemaSet {
        schema(SchemaRole::Server)
    }
    fn sync_registry(&self) -> Option<Registry> {
        Some(registry(SchemaRole::Server))
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(Notes);
    }
}
pub fn apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(NotesApp)
}

/// Demo/test databases only. Applications freeze these operations in compiled migrations.
pub async fn database(role: SchemaRole) -> AppResult<Database> {
    use che_orm::{Migration, MigrationGraph, MigrationId, MigrationOperation};
    let db = Database::connect_in_memory().await?;
    let state = ProjectState::from_app_schemas([("notes", &schema(role))])
        .map_err(|error| che_rest::AppError::BadRequest(error.to_string()))?;
    let mut migrations = if matches!(role, SchemaRole::Server) {
        che_rest::auth::migrations()
    } else {
        vec![]
    };
    migrations.push(Migration::new(
        MigrationId {
            app: "notes".into(),
            name: "0001_initial".into(),
        },
        vec![],
        state
            .models()
            .iter()
            .cloned()
            .map(|model| MigrationOperation::CreateModel { model })
            .collect(),
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
        .set(User::USERNAME, "notes-demo")
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
