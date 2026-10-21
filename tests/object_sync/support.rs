use che_orm::{Database, DatabaseQuery, Model, ModelField, ModelSerializer, SchemaSet, Uuid};
use che_rest::auth::{self, AuthToken, Group, User, access::*};
use che_rest::{
    AppModule, AppResult, AppState, FilterSet, InstalledApps, ModuleContext, ObjectSyncEndpoints,
    SyncViewSet, ViewSet, ViewSetConfig,
};
use che_sync::{Registry, SchemaRole};
#[derive(Debug, Clone, Model)]
#[orm(table = "object_notes", sync, sync_name = "objects.note")]
pub struct Note {
    #[orm(primary_key)]
    pub id: Uuid,
    pub title: String,
    pub done: bool,
}
#[derive(Debug, Model)]
#[orm(
    table = "object_note_users",
    unique("note_id", "user_id", "permission")
)]
pub struct UserGrant {
    #[orm(primary_key)]
    id: i64,
    #[orm(foreign_key=Note,on_delete="cascade")]
    note_id: Uuid,
    #[orm(foreign_key=User,on_delete="cascade")]
    user_id: i64,
    permission: String,
}
#[derive(Debug, Model)]
#[orm(
    table = "object_note_groups",
    unique("note_id", "group_id", "permission")
)]
pub struct GroupGrant {
    #[orm(primary_key)]
    id: i64,
    #[orm(foreign_key=Note,on_delete="cascade")]
    note_id: Uuid,
    #[orm(foreign_key=Group,on_delete="cascade")]
    group_id: i64,
    permission: String,
}
impl ObjectGrant<Note> for UserGrant {
    const OBJECT: ModelField<Self, Uuid> = Self::NOTE_ID;
    const SUBJECT: ModelField<Self, i64> = Self::USER_ID;
    const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
}
impl ObjectGrant<Note> for GroupGrant {
    const OBJECT: ModelField<Self, Uuid> = Self::NOTE_ID;
    const SUBJECT: ModelField<Self, i64> = Self::GROUP_ID;
    const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
}
impl AccessModel for Note {
    const ACCESS_KEY: &'static str = "objects.note";
    type UserGrant = UserGrant;
    type GroupGrant = GroupGrant;
    const CREATOR_PERMISSIONS: &'static [&'static str] = &[VIEW, CHANGE, DELETE];
}
#[derive(ModelSerializer)]
#[serializer(model=Note)]
pub struct NoteSerializer {
    #[serializer(read_only)]
    pub id: Uuid,
    pub title: String,
    pub done: bool,
}
pub fn schema(role: SchemaRole) -> SchemaSet {
    match role {
        SchemaRole::Client => SchemaSet::new().client_model::<Note>(),
        SchemaRole::Server => SchemaSet::new().server_model::<Note>(),
    }
}
pub fn registry(role: SchemaRole) -> Registry {
    Registry::from_schema(&schema(role)).unwrap()
}
#[derive(Clone)]
pub struct Notes;
impl ViewSet for Notes {
    type Model = Note;
    type Serializer = NoteSerializer;
    type ListSerializer = NoteSerializer;
    type QuerySet = DatabaseQuery<Note>;
    type FilterSet = FilterSet<Note>;
    type Permission = Access<Note>;
    fn path(&self) -> &'static str {
        "/notes"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Note::query().filter(Note::TITLE.ne("outside")))
    }
    fn configure(&self, c: &mut ViewSetConfig<Self>) -> AppResult<()> {
        c.extend(ObjectSyncEndpoints::new(self.clone()))
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
        "objects"
    }
    fn schema(&self) -> SchemaSet {
        schema(SchemaRole::Server).merge(Access::<Note>::schema())
    }
    fn sync_registry(&self) -> Option<Registry> {
        Some(registry(SchemaRole::Server))
    }
    fn init(&self, c: &mut ModuleContext) {
        c.viewset_with(Notes);
    }
}
pub fn apps() -> InstalledApps {
    InstalledApps::new()
        .add(auth::module())
        .add(che_rest::sync::object_schema_module())
        .add(NotesApp)
}
/// Disposable test schemas. Production freezes generated follow-up operations.
pub async fn database(role: SchemaRole) -> Database {
    use che_orm::{Migration, MigrationGraph, MigrationId, MigrationOperation, ProjectState};
    let db = Database::connect_in_memory().await.unwrap();
    let mut schema = schema(role);
    if role == SchemaRole::Server {
        schema = schema.merge(Access::<Note>::schema());
    }
    let state = ProjectState::from_app_schemas([("objects", &schema)]).unwrap();
    let mut migrations = if role == SchemaRole::Server {
        auth::migrations()
    } else {
        vec![]
    };
    let initial = MigrationId {
        app: "objects".into(),
        name: "0001".into(),
    };
    migrations.push(Migration::new(
        initial.clone(),
        if role == SchemaRole::Server {
            vec![MigrationId {
                app: "auth".into(),
                name: "0002_access".into(),
            }]
        } else {
            vec![]
        },
        state
            .models()
            .iter()
            .cloned()
            .map(|model| MigrationOperation::CreateModel { model })
            .collect(),
    ));
    migrations.push(Migration::new(
        MigrationId {
            app: "objects".into(),
            name: "0002_object_delivery".into(),
        },
        vec![initial],
        che_sync::object_log::migration_operations(role),
    ));
    db.migrate(MigrationGraph::new(migrations.clone()).unwrap())
        .await
        .unwrap();
    db.migrate(MigrationGraph::new(migrations).unwrap())
        .await
        .unwrap();
    db
}
pub async fn state() -> (AppState, [i64; 3]) {
    let state = AppState::from_database(database(SchemaRole::Server).await);
    let mut ids = [0; 3];
    for (i, name) in ["alice", "bob", "carol"].into_iter().enumerate() {
        let user = state
            .database()
            .create::<User>()
            .set(User::USERNAME, name)
            .set(User::PASSWORD_HASH, "unused")
            .execute()
            .await
            .unwrap();
        ids[i] = user.id;
        state
            .database()
            .create::<AuthToken>()
            .set(AuthToken::USER_ID, user.id)
            .set(AuthToken::KEY_HASH, auth::token_hash(name))
            .execute()
            .await
            .unwrap();
    }
    let alice = ids[0];
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Note>::grant_model(tx, Subject::User(alice), ADD).await },
            )
        })
        .await
        .unwrap();
    (state, ids)
}
