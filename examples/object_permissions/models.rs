use che_orm::{DatabaseQuery, Migration, Model, ModelField, ModelSerializer, SchemaSet};
use che_rest::auth::{
    Group, User,
    access::{Access, AccessModel, CHANGE, MANAGE_ACCESS, ObjectGrant, VIEW},
};
use che_rest::{AppModule, FilterSet, ModuleContext, ViewSet};

#[derive(Debug, Model)]
#[orm(table = "shared_notes")]
pub struct Note {
    #[orm(primary_key)]
    pub id: i64,
    pub title: String,
}
#[derive(Debug, Model)]
#[orm(
    table = "shared_note_users",
    unique("note_id", "user_id", "permission"),
    index("user_id", "permission", "note_id")
)]
pub struct NoteUserPermission {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(foreign_key = Note, on_delete = "cascade")]
    pub note_id: i64,
    #[orm(foreign_key = User, on_delete = "cascade")]
    pub user_id: i64,
    pub permission: String,
}
#[derive(Debug, Model)]
#[orm(
    table = "shared_note_groups",
    unique("note_id", "group_id", "permission"),
    index("group_id", "permission", "note_id")
)]
pub struct NoteGroupPermission {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(foreign_key = Note, on_delete = "cascade")]
    pub note_id: i64,
    #[orm(foreign_key = Group, on_delete = "cascade")]
    pub group_id: i64,
    pub permission: String,
}
impl ObjectGrant<Note> for NoteUserPermission {
    const OBJECT: ModelField<Self, i64> = Self::NOTE_ID;
    const SUBJECT: ModelField<Self, i64> = Self::USER_ID;
    const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
}
impl ObjectGrant<Note> for NoteGroupPermission {
    const OBJECT: ModelField<Self, i64> = Self::NOTE_ID;
    const SUBJECT: ModelField<Self, i64> = Self::GROUP_ID;
    const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
}
impl AccessModel for Note {
    const ACCESS_KEY: &'static str = "notes.note";
    type UserGrant = NoteUserPermission;
    type GroupGrant = NoteGroupPermission;
    const CREATOR_PERMISSIONS: &'static [&'static str] = &[VIEW, CHANGE, MANAGE_ACCESS];
}
#[derive(ModelSerializer)]
#[serializer(model = Note)]
pub struct NoteSerializer {
    #[serializer(read_only)]
    pub id: i64,
    pub title: String,
}
#[derive(Clone)]
pub struct Notes;
impl ViewSet for Notes {
    type Model = Note;
    type Serializer = NoteSerializer;
    type ListSerializer = Self::Serializer;
    type QuerySet = DatabaseQuery<Note>;
    type FilterSet = FilterSet<Note>;
    type Permission = Access<Note>;
    fn path(&self) -> &'static str {
        "/notes"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Note::query())
    }
}
pub struct NotesApp;
impl AppModule for NotesApp {
    fn name(&self) -> &'static str {
        "notes"
    }
    fn schema(&self) -> SchemaSet {
        SchemaSet::new()
            .model::<Note>()
            .merge(Access::<Note>::schema())
    }
    fn migrations(&self) -> Vec<Migration> {
        vec![super::migration::migration()]
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(Notes);
    }
}
