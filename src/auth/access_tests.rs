use super::{access::*, *};
use crate::{
    AppError, AppModule, AppResult, FilterSet, InstalledApps, ModuleContext, Server, ViewSet,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::{
    Database, DatabaseQuery, ImageField, MigrationGraph, Model, ModelField, ModelSerializer, Uuid,
};
use tower::ServiceExt;

#[derive(Debug, Model)]
#[orm(table = "acl_documents")]
struct Document {
    #[orm(primary_key)]
    id: i64,
    name: String,
    image: Option<ImageField>,
}
#[derive(Debug, Model)]
#[orm(
    table = "acl_document_users",
    unique("document_id", "user_id", "permission"),
    index("user_id", "permission", "document_id")
)]
struct DocumentUserGrant {
    #[orm(primary_key)]
    id: i64,
    #[orm(foreign_key = Document, on_delete = "cascade")]
    document_id: i64,
    #[orm(foreign_key = User, on_delete = "cascade")]
    user_id: i64,
    permission: String,
}
#[derive(Debug, Model)]
#[orm(
    table = "acl_document_groups",
    unique("document_id", "group_id", "permission"),
    index("group_id", "permission", "document_id")
)]
struct DocumentGroupGrant {
    #[orm(primary_key)]
    id: i64,
    #[orm(foreign_key = Document, on_delete = "cascade")]
    document_id: i64,
    #[orm(foreign_key = Group, on_delete = "cascade")]
    group_id: i64,
    permission: String,
}
impl ObjectGrant<Document> for DocumentUserGrant {
    const OBJECT: ModelField<Self, i64> = Self::DOCUMENT_ID;
    const SUBJECT: ModelField<Self, i64> = Self::USER_ID;
    const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
}
impl ObjectGrant<Document> for DocumentGroupGrant {
    const OBJECT: ModelField<Self, i64> = Self::DOCUMENT_ID;
    const SUBJECT: ModelField<Self, i64> = Self::GROUP_ID;
    const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
}
impl AccessModel for Document {
    const ACCESS_KEY: &'static str = "tests.document";
    type UserGrant = DocumentUserGrant;
    type GroupGrant = DocumentGroupGrant;
    const CREATOR_PERMISSIONS: &'static [&'static str] = &[VIEW, CHANGE, MANAGE_ACCESS];
}
#[derive(ModelSerializer)]
#[serializer(model = Document)]
struct DocumentSerializer {
    #[serializer(read_only)]
    id: i64,
    name: String,
    image: Option<ImageField>,
}
#[derive(Clone)]
struct Documents;
impl ViewSet for Documents {
    type Model = Document;
    type Serializer = DocumentSerializer;
    type ListSerializer = Self::Serializer;
    type QuerySet = DatabaseQuery<Document>;
    type FilterSet = FilterSet<Document>;
    type Permission = Access<Document>;
    fn path(&self) -> &'static str {
        "/documents"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Document::query().order_by(Document::ID.asc()))
    }
    fn configure(&self, config: &mut crate::ViewSetConfig<Self>) -> AppResult<()> {
        config.image_field(
            crate::media::ImageFieldEndpoint::new(
                "image",
                Document::IMAGE,
                |model: &Document| model.image.as_ref(),
                crate::media::ImagePolicy::new("acl", 1024 * 1024, 32),
            )
            .before(revoke_before_upload)
            .authorize(authorize_image),
        )
    }
}
struct TestApp;
impl AppModule for TestApp {
    fn name(&self) -> &'static str {
        "access_tests"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new()
            .model::<Document>()
            .merge(Access::<Document>::schema())
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(Documents);
    }
}
fn as_principal(user: &User) -> CurrentPrincipal {
    CurrentPrincipal {
        auth_user: CurrentUser {
            id: user.id,
            username: user.username.clone(),
            is_staff: user.is_staff,
            is_admin: user.is_admin,
            is_superuser: user.is_superuser,
        },
        app_user: None,
    }
}
async fn setup() -> (AppState, Router, i64, i64) {
    let state = AppState::from_database(Database::connect_in_memory().await.unwrap());
    state
        .database()
        .migrate(MigrationGraph::new(migrations()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        state
            .database()
            .migrate(MigrationGraph::new(migrations()).unwrap())
            .await
            .unwrap(),
        0
    );
    state.database().create_table::<Document>().await.unwrap();
    state
        .database()
        .create_table::<DocumentUserGrant>()
        .await
        .unwrap();
    state
        .database()
        .create_table::<DocumentGroupGrant>()
        .await
        .unwrap();
    let alice = state
        .database()
        .create::<User>()
        .set(User::USERNAME, "alice")
        .set(User::PASSWORD_HASH, "unused")
        .execute()
        .await
        .unwrap();
    let bob = state
        .database()
        .create::<User>()
        .set(User::USERNAME, "bob")
        .set(User::PASSWORD_HASH, "unused")
        .execute()
        .await
        .unwrap();
    for (id, token) in [(alice.id, "alice"), (bob.id, "bob")] {
        state
            .database()
            .create::<AuthToken>()
            .set(AuthToken::USER_ID, id)
            .set(AuthToken::KEY_HASH, token_hash(token))
            .execute()
            .await
            .unwrap();
    }
    let router = Server::new(state.clone())
        .install(
            InstalledApps::new()
                .add(module())
                .add(TestApp)
                .add(crate::media::module()),
        )
        .build()
        .await
        .unwrap();
    (state, router, alice.id, bob.id)
}
async fn send(
    router: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if !token.is_empty() {
        request = request.header("authorization", format!("Token {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}
async fn document(state: &AppState, name: &str) -> i64 {
    state
        .database()
        .create::<Document>()
        .set(Document::NAME, name)
        .execute()
        .await
        .unwrap()
        .id
}
async fn allow(state: &AppState, subject: Subject, permission: &'static str, id: i64) {
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move { Access::<Document>::grant(tx, subject, permission, id).await })
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn list_count_pagination_and_detail_use_the_same_acl() {
    let (state, router, alice, _) = setup().await;
    let hidden = document(&state, "hidden").await;
    let first = document(&state, "first").await;
    let second = document(&state, "second").await;
    allow(&state, Subject::User(alice), VIEW, first).await;
    allow(&state, Subject::User(alice), VIEW, second).await;
    let (status, page) = send(
        &router,
        "alice",
        "GET",
        "/api/documents/?limit=1&offset=1",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["count"], 2);
    assert_eq!(page["results"][0]["id"], second);
    assert_eq!(
        send(
            &router,
            "alice",
            "GET",
            &format!("/api/documents/{hidden}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            "bob",
            "GET",
            &format!("/api/documents/{first}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&router, "", "GET", "/api/documents/", serde_json::json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &router,
            "alice",
            "PATCH",
            &format!("/api/documents/{first}/"),
            serde_json::json!({"name":"forbidden"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        state
            .database()
            .get::<Document>(first)
            .await
            .unwrap()
            .unwrap()
            .name,
        "first"
    );
}

#[tokio::test]
async fn group_and_direct_sources_union_and_revoke_independently() {
    let (state, router, alice, _) = setup().await;
    let id = document(&state, "shared").await;
    let group = state
        .database()
        .create::<Group>()
        .set(Group::NAME, "editors")
        .execute()
        .await
        .unwrap()
        .id;
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                assert!(add_member(tx, alice, group).await?);
                assert!(!add_member(tx, alice, group).await?);
                Access::<Document>::grant(tx, Subject::Group(group), VIEW, id).await?;
                Access::<Document>::grant(tx, Subject::User(alice), VIEW, id).await?;
                Ok::<_, AppError>(())
            })
        })
        .await
        .unwrap();
    let url = format!("/api/documents/{id}/");
    assert_eq!(
        send(&router, "alice", "GET", &url, serde_json::json!({}))
            .await
            .0,
        StatusCode::OK
    );
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Document>::revoke(tx, Subject::User(alice), VIEW, id).await },
            )
        })
        .await
        .unwrap();
    assert_eq!(
        send(&router, "alice", "GET", &url, serde_json::json!({}))
            .await
            .0,
        StatusCode::OK
    );
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move { remove_member(tx, alice, group).await })
        })
        .await
        .unwrap();
    assert_eq!(
        send(&router, "alice", "GET", &url, serde_json::json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn creation_grants_and_mutations_are_atomic_and_do_not_leak_without_view() {
    let (state, router, alice, _) = setup().await;
    assert_eq!(
        send(
            &router,
            "alice",
            "POST",
            "/api/documents/",
            serde_json::json!({"name":"new"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Document>::grant_model(tx, Subject::User(alice), ADD).await },
            )
        })
        .await
        .unwrap();
    let (status, created) = send(
        &router,
        "alice",
        "POST",
        "/api/documents/",
        serde_json::json!({"name":"new"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_i64().unwrap();
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Document>::revoke(tx, Subject::User(alice), VIEW, id).await },
            )
        })
        .await
        .unwrap();
    let (status, body) = send(
        &router,
        "alice",
        "PATCH",
        &format!("/api/documents/{id}/"),
        serde_json::json!({"name":"edited"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_null());
    assert_eq!(
        state
            .database()
            .get::<Document>(id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "edited"
    );
    allow(&state, Subject::User(alice), DELETE, id).await;
    assert_eq!(
        send(
            &router,
            "alice",
            "DELETE",
            &format!("/api/documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(
        state
            .database()
            .all::<DocumentUserGrant>()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rollback_removes_grants_and_delivery_work_and_rechecks_use_transaction() {
    let (state, _, alice, _) = setup().await;
    let id = document(&state, "private").await;
    let before = state.database().count::<AccessEvent>().await.unwrap();
    let owned = state.clone();
    let user = state.database().get::<User>(alice).await.unwrap().unwrap();
    let principal = as_principal(&user);
    let result: AppResult<()> = state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                Access::<Document>::grant(tx, Subject::User(alice), CHANGE, id).await?;
                let model = tx.get::<Document>(id).await?.unwrap();
                let context = crate::PermissionContext::in_transaction(&owned, tx);
                Access::<Document>::default()
                    .check_write(
                        context,
                        Some(&principal),
                        ViewAction::Patch,
                        Some(&model),
                        None,
                    )
                    .await?;
                Access::<Document>::revoke(tx, Subject::User(alice), CHANGE, id).await?;
                assert!(
                    Access::<Document>::default()
                        .check_write(
                            context,
                            Some(&principal),
                            ViewAction::Patch,
                            Some(&model),
                            None
                        )
                        .await
                        .is_err()
                );
                Err(AppError::Forbidden("rollback".into()))
            })
        })
        .await;
    assert!(matches!(result, Err(AppError::Forbidden(_))));
    assert_eq!(
        state.database().count::<AccessEvent>().await.unwrap(),
        before
    );
    assert!(
        state
            .database()
            .all::<DocumentUserGrant>()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn global_permissions_are_explicit_and_delegation_is_bounded() {
    let (state, router, alice, bob) = setup().await;
    let id = document(&state, "private").await;
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                Access::<Document>::grant_model(tx, Subject::User(alice), VIEW).await
            })
        })
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            "alice",
            "GET",
            &format!("/api/documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    allow(&state, Subject::User(alice), MANAGE_ACCESS, id).await;
    let principal = as_principal(&state.database().get::<User>(alice).await.unwrap().unwrap());
    let owned = state.clone();
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                let context = crate::PermissionContext::in_transaction(&owned, tx);
                assert!(
                    Access::<Document>::change_access(
                        context,
                        &principal,
                        Subject::User(bob),
                        DELETE,
                        id,
                        true,
                        &[VIEW]
                    )
                    .await
                    .is_err()
                );
                assert!(
                    Access::<Document>::change_access(
                        context,
                        &principal,
                        Subject::User(bob),
                        VIEW,
                        id,
                        true,
                        &[VIEW]
                    )
                    .await?
                );
                assert!(
                    !Access::<Document>::change_access(
                        context,
                        &principal,
                        Subject::User(bob),
                        VIEW,
                        id,
                        true,
                        &[VIEW]
                    )
                    .await?
                );
                Ok::<_, AppError>(())
            })
        })
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            "bob",
            "GET",
            &format!("/api/documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn media_download_requires_object_view_permission() {
    let (mut state, _, alice, _) = setup().await;
    let root = std::env::temp_dir().join(format!("che-rest-acl-media-{}", Uuid::now_v7()));
    state.config.media.root = root.to_string_lossy().into();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("private.png"), b"private").unwrap();
    let id = state
        .database()
        .create::<Document>()
        .set(Document::NAME, "image")
        .set(Document::IMAGE, Some(ImageField::new("private.png")))
        .execute()
        .await
        .unwrap()
        .id;
    let router = Server::new(state.clone())
        .install(
            InstalledApps::new()
                .add(module())
                .add(TestApp)
                .add(crate::media::module()),
        )
        .build()
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            "alice",
            "GET",
            "/api/media/private.png",
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    allow(&state, Subject::User(alice), VIEW, id).await;
    assert_eq!(
        send(
            &router,
            "alice",
            "GET",
            "/api/media/private.png",
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Document>::revoke(tx, Subject::User(alice), VIEW, id).await },
            )
        })
        .await
        .unwrap();
    assert_eq!(
        send(
            &router,
            "alice",
            "GET",
            "/api/media/private.png",
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn upgrading_existing_auth_preserves_users_and_frozen_history() {
    let database = Database::connect_in_memory().await.unwrap();
    let initial = migrations().remove(0);
    assert_eq!(initial.checksum, "v1:cf9bef2cfd081db4");
    database
        .migrate(MigrationGraph::new(vec![initial]).unwrap())
        .await
        .unwrap();
    let id = database
        .create::<User>()
        .set(User::USERNAME, "existing")
        .set(User::PASSWORD_HASH, "preserved")
        .execute()
        .await
        .unwrap()
        .id;
    assert_eq!(
        database
            .migrate(MigrationGraph::new(migrations()).unwrap())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        database
            .get::<User>(id)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "preserved"
    );
    assert_eq!(
        database
            .migrate(MigrationGraph::new(migrations()).unwrap())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn permission_explanation_preserves_sources_without_per_object_queries() {
    let (state, _, alice, _) = setup().await;
    let id = document(&state, "shared").await;
    let other = document(&state, "private").await;
    let group = state
        .database()
        .create::<Group>()
        .set(Group::NAME, "readers")
        .execute()
        .await
        .unwrap()
        .id;
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                add_member(tx, alice, group).await?;
                Access::<Document>::grant(tx, Subject::User(alice), VIEW, id).await?;
                Access::<Document>::grant(tx, Subject::Group(group), VIEW, id).await?;
                Ok::<_, AppError>(())
            })
        })
        .await
        .unwrap();
    let principal = as_principal(&state.database().get::<User>(alice).await.unwrap().unwrap());
    let permissions =
        Access::<Document>::effective_permissions((&state).into(), &principal, &[id, other])
            .await
            .unwrap();
    assert_eq!(permissions[0].grants.len(), 2);
    assert!(
        permissions[0]
            .grants
            .iter()
            .any(|grant| grant.source == PermissionSource::Direct)
    );
    assert!(
        permissions[0]
            .grants
            .iter()
            .any(|grant| grant.source == PermissionSource::Group(group))
    );
    assert!(permissions[1].grants.is_empty());
}

mod uuid_objects {
    use super::*;
    #[derive(Debug, Model)]
    #[orm(table = "acl_uuid_objects")]
    pub struct Item {
        #[orm(primary_key)]
        pub id: Uuid,
        pub name: String,
    }
    #[derive(Debug, Model)]
    #[orm(table = "acl_uuid_users", unique("item_id", "user_id", "permission"))]
    pub struct UserGrant {
        #[orm(primary_key)]
        pub id: i64,
        #[orm(foreign_key = Item, on_delete = "cascade")]
        pub item_id: Uuid,
        #[orm(foreign_key = User, on_delete = "cascade")]
        pub user_id: i64,
        pub permission: String,
    }
    #[derive(Debug, Model)]
    #[orm(table = "acl_uuid_groups", unique("item_id", "group_id", "permission"))]
    pub struct GroupGrant {
        #[orm(primary_key)]
        pub id: i64,
        #[orm(foreign_key = Item, on_delete = "cascade")]
        pub item_id: Uuid,
        #[orm(foreign_key = Group, on_delete = "cascade")]
        pub group_id: i64,
        pub permission: String,
    }
    impl ObjectGrant<Item> for UserGrant {
        const OBJECT: ModelField<Self, Uuid> = Self::ITEM_ID;
        const SUBJECT: ModelField<Self, i64> = Self::USER_ID;
        const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
    }
    impl ObjectGrant<Item> for GroupGrant {
        const OBJECT: ModelField<Self, Uuid> = Self::ITEM_ID;
        const SUBJECT: ModelField<Self, i64> = Self::GROUP_ID;
        const PERMISSION: ModelField<Self, String> = Self::PERMISSION;
    }
    impl AccessModel for Item {
        const ACCESS_KEY: &'static str = "tests.uuid_item";
        type UserGrant = UserGrant;
        type GroupGrant = GroupGrant;
        const MODE: PermissionMode = PermissionMode::ModelOrObject;
        const SUPERUSER_BYPASS: bool = true;
    }
}

#[tokio::test]
async fn uuid_keys_group_model_grants_and_superuser_modes() {
    use uuid_objects::*;
    let (state, _, alice, _) = setup().await;
    state.database().create_table::<Item>().await.unwrap();
    state.database().create_table::<UserGrant>().await.unwrap();
    state.database().create_table::<GroupGrant>().await.unwrap();
    let first = state
        .database()
        .create::<Item>()
        .set(Item::NAME, "first")
        .execute()
        .await
        .unwrap()
        .id;
    let second = state
        .database()
        .create::<Item>()
        .set(Item::NAME, "second")
        .execute()
        .await
        .unwrap()
        .id;
    let mut principal = as_principal(&state.database().get::<User>(alice).await.unwrap().unwrap());
    let group = state
        .database()
        .create::<Group>()
        .set(Group::NAME, "all_readers")
        .execute()
        .await
        .unwrap()
        .id;
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                add_member(tx, alice, group).await?;
                Access::<Item>::grant(tx, Subject::User(alice), VIEW, first).await?;
                Ok::<_, AppError>(())
            })
        })
        .await
        .unwrap();
    let context = (&state).into();
    assert!(
        Access::<Item>::can(context, &principal, VIEW, first)
            .await
            .unwrap()
    );
    assert!(
        !Access::<Item>::can(context, &principal, VIEW, second)
            .await
            .unwrap()
    );
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Item>::grant_model(tx, Subject::Group(group), VIEW).await },
            )
        })
        .await
        .unwrap();
    assert!(
        Access::<Item>::can(context, &principal, VIEW, second)
            .await
            .unwrap()
    );
    principal.auth_user.is_superuser = true;
    assert!(
        Access::<Item>::can(context, &principal, DELETE, second)
            .await
            .unwrap()
    );
    // Default policy never treats an admin or superuser flag as an implicit grant.
    let document = document(&state, "restricted").await;
    assert!(
        !Access::<Document>::can(context, &principal, VIEW, document)
            .await
            .unwrap()
    );
    Access::<Item>::default().validate(&[]).unwrap();
}

fn authorize_image<'a>(
    _: crate::PermissionContext<'a>,
    _: Option<&'a CurrentPrincipal>,
    _: &'a Document,
    _: ViewAction,
) -> crate::media::ImageFuture<'a, ()> {
    Box::pin(async {
        tokio::task::yield_now().await;
        Ok(())
    })
}
fn revoke_before_upload<'a>(
    state: &'a AppState,
    model: &'a Document,
    form: &'a crate::media::ImageForm,
) -> crate::media::ImageFuture<'a, crate::media::ImageGuard> {
    Box::pin(async move {
        if form.text("revoke")? == Some("true") {
            let id = model.id;
            state
                .database()
                .transaction_result(move |tx| {
                    Box::pin(async move {
                        for grant in tx
                            .fetch_all(
                                DocumentUserGrant::query().filter(
                                    DocumentUserGrant::DOCUMENT_ID
                                        .eq(id)
                                        .and(DocumentUserGrant::PERMISSION.eq(CHANGE)),
                                ),
                            )
                            .await?
                        {
                            Access::<Document>::revoke(
                                tx,
                                Subject::User(grant.user_id),
                                CHANGE,
                                id,
                            )
                            .await?;
                        }
                        Ok::<_, AppError>(())
                    })
                })
                .await?;
        }
        Ok(Box::new(()) as crate::media::ImageGuard)
    })
}

#[tokio::test]
async fn image_action_rechecks_acl_at_commit_and_discards_the_new_file() {
    let (mut state, _, alice, _) = setup().await;
    let root = std::env::temp_dir().join(format!("che-rest-acl-upload-{}", Uuid::now_v7()));
    state.config.media.root = root.to_string_lossy().into();
    let id = document(&state, "image").await;
    allow(&state, Subject::User(alice), CHANGE, id).await;
    let router = Server::new(state.clone())
        .install(InstalledApps::new().add(module()).add(TestApp))
        .build()
        .await
        .unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut body = b"--test\r\nContent-Disposition: form-data; name=\"revoke\"\r\n\r\ntrue\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
    body.extend(png.into_inner());
    body.extend(b"\r\n--test--\r\n");
    let response = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/documents/{id}/images/image/"))
                .header("authorization", "Token alice")
                .header("content-type", "multipart/form-data; boundary=test")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        state
            .database()
            .get::<Document>(id)
            .await
            .unwrap()
            .unwrap()
            .image
            .is_none()
    );
    assert_eq!(std::fs::read_dir(root.join("acl")).unwrap().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone, Default)]
struct PublicOnly;
impl Permission<Document> for PublicOnly {
    async fn check_request(
        &self,
        _: crate::PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        _: ViewAction,
    ) -> AppResult<()> {
        tokio::task::yield_now().await;
        current.ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
        Ok(())
    }
    async fn scope(
        &self,
        context: crate::PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        action: ViewAction,
    ) -> AppResult<Option<che_orm::Expr>> {
        self.check_request(context, current, action).await?;
        Ok(Some(Document::NAME.eq("public")))
    }
}
#[derive(Clone)]
struct PublicDocuments;
impl ViewSet for PublicDocuments {
    type Model = Document;
    type Serializer = DocumentSerializer;
    type ListSerializer = Self::Serializer;
    type QuerySet = DatabaseQuery<Document>;
    type FilterSet = FilterSet<Document>;
    type Permission = PublicOnly;
    fn path(&self) -> &'static str {
        "/public-documents"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Document::query())
    }
}
#[tokio::test]
async fn async_scope_only_policy_protects_writes_and_their_response() {
    let (state, _, alice, _) = setup().await;
    let private = document(&state, "private").await;
    let public = document(&state, "public").await;
    let principal = as_principal(&state.database().get::<User>(alice).await.unwrap().unwrap());
    let router = crate::router(state.clone(), PublicDocuments, Router::new())
        .layer(axum::Extension(principal));
    let private_url = format!("/public-documents/{private}/");
    assert_eq!(
        send(&router, "", "GET", &private_url, serde_json::json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            "",
            "PATCH",
            &private_url,
            serde_json::json!({"name":"public"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = send(
        &router,
        "",
        "PATCH",
        &format!("/public-documents/{public}/"),
        serde_json::json!({"name":"private"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_null());
    assert_eq!(
        state
            .database()
            .get::<Document>(public)
            .await
            .unwrap()
            .unwrap()
            .name,
        "private"
    );
}

#[test]
fn nested_acl_serializers_are_rejected_before_serving_requests() {
    let mut fields = DocumentSerializer::fields().to_vec();
    fields[1].related_model = Some("User");
    assert!(Access::<Document>::default().validate(&fields).is_err());
}

#[path = "access_regressions.rs"]
mod regressions;
