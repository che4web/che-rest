#![cfg(feature = "postgres")]

#[allow(dead_code)]
#[path = "../examples/object_permissions/models.rs"]
mod acl;
#[path = "../examples/cli_fullstack/src/apps/tasks/filters.rs"]
mod filters;
#[allow(dead_code)]
#[path = "../examples/object_permissions/migration.rs"]
mod migration;
#[path = "../examples/cli_fullstack/src/apps/tasks/models.rs"]
mod models;
#[allow(dead_code, unused_imports)]
#[path = "../examples/model_sync/support.rs"]
mod notes;
#[path = "../examples/cli_fullstack/src/apps/tasks/serializers.rs"]
mod serializers;
#[path = "support/postgres.rs"]
mod support;
#[path = "../examples/cli_fullstack/src/apps/tasks/views.rs"]
mod views;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::{
    Database, DatabaseQuery, ImageField, Migration, MigrationGraph, MigrationId,
    MigrationOperation, Model, ModelSerializer, ProjectState, SchemaSet,
};
use che_rest::auth::{AuthToken, User, token_hash};
use che_rest::{AppModule, AppState, InstalledApps, ModuleContext, Server};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Tasks;
impl AppModule for Tasks {
    fn name(&self) -> &'static str {
        "tasks"
    }
    fn schema(&self) -> SchemaSet {
        SchemaSet::new().model::<models::Task>()
    }
    fn init(&self, context: &mut ModuleContext) {
        context.viewset_with(views::TaskViewSet);
    }
}

async fn migrate_fixture(database: &Database, schema: &SchemaSet, app: &str) {
    let auth = che_rest::auth::module().schema();
    let state = ProjectState::from_app_schemas([("auth", &auth), (app, schema)]).unwrap();
    let mut history = che_rest::auth::migrations();
    let dependencies = vec![history.last().unwrap().id.clone()];
    history.push(Migration::new(
        MigrationId {
            app: app.into(),
            name: "0001_test_fixture".into(),
        },
        dependencies,
        state
            .models()
            .iter()
            .filter(|model| model.key.app != "auth")
            .cloned()
            .map(|model| MigrationOperation::CreateModel { model })
            .collect(),
    ));
    let graph = MigrationGraph::new(history).unwrap();
    database.migrate(graph.clone()).await.unwrap();
    assert_eq!(database.migrate(graph).await.unwrap(), 0);
}

async fn user(database: &Database, name: &str) -> (i64, String) {
    let user = database
        .create::<User>()
        .set(User::USERNAME, name)
        .set(
            User::PASSWORD_HASH,
            che_rest::auth::hash_password("secret").unwrap(),
        )
        .execute()
        .await
        .unwrap();
    let token = format!("postgres-test-{name}");
    database
        .create::<AuthToken>()
        .set(AuthToken::USER_ID, user.id)
        .set(AuthToken::KEY_HASH, token_hash(&token))
        .execute()
        .await
        .unwrap();
    (user.id, token)
}

async fn send(
    router: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
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
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL"]
async fn postgres_auth_crud_relations_datetime_and_pagination() {
    support::isolated(|database, _| async move {
        migrate_fixture(&database, &Tasks.schema(), "tasks").await;
        let (alice, token) = user(&database, "alice").await;
        let state = AppState::from_database(database.clone());
        let router = Server::new(state)
            .install(
                InstalledApps::new()
                    .add(che_rest::auth::module())
                    .add(Tasks),
            )
            .build()
            .await
            .unwrap();
        let (status, login) = send(
            &router,
            "",
            "POST",
            "/api-token-auth/",
            json!({"username":"alice","password":"secret"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{login}");
        assert!(login["token"].is_string());
        assert_eq!(
            send(
                &router,
                "",
                "POST",
                "/api-token-auth/",
                json!({"username":"alice","password":"wrong"})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, session) = send(
            &router,
            "",
            "POST",
            "/api-session-auth/login/",
            json!({"username":"alice","password":"secret"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{session}");
        let (status, first) = send(
            &router,
            &token,
            "POST",
            "/api/tasks/",
            json!({"name":"alpha","status":"draft","assignee_id":alice}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{first}");
        assert_eq!(first["author"]["id"], alice);
        assert!(first["created_at"].as_str().unwrap().ends_with('Z'));
        let id = first["id"].as_i64().unwrap();
        let (status, second) = send(
            &router,
            &token,
            "POST",
            "/api/tasks/",
            json!({"name":"beta","status":"in_progress","assignee_id":null}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{second}");
        let (status, page) = send(
            &router,
            &token,
            "GET",
            "/api/tasks/?ordering=-created_at&limit=1",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["count"], 2);
        assert_eq!(page["results"].as_array().unwrap().len(), 1);
        let (_, filtered) = send(
            &router,
            &token,
            "GET",
            "/api/tasks/?name__contains=ALPHA",
            Value::Null,
        )
        .await;
        assert_eq!(filtered["count"], 0); // PostgreSQL LIKE is case-sensitive.
        let (status, updated) = send(
            &router,
            &token,
            "PATCH",
            &format!("/api/tasks/{id}/"),
            json!({"status":"done","assignee_id":null}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["status"], "done");
        assert!(updated["assignee_id"].is_null());
        let loaded = database
            .query::<User>()
            .filter(User::ID.eq(alice))
            .prefetch_related(models::Task::AUTHOR.reverse())
            .all()
            .await
            .unwrap();
        assert_eq!(loaded[0].relations.0.related.len(), 2);
        assert_eq!(
            send(
                &router,
                &token,
                "DELETE",
                &format!("/api/tasks/{id}/"),
                Value::Null
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(
                &router,
                &token,
                "GET",
                &format!("/api/tasks/{id}/"),
                Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL"]
async fn postgres_frozen_acl_migrations_and_http_scopes() {
    support::isolated(|database, _| async move {
        use che_rest::auth::access::{Access, Subject, VIEW};
        let mut history = che_rest::auth::migrations();
        history.push(migration::migration());
        let graph = MigrationGraph::new(history).unwrap();
        database.migrate(graph.clone()).await.unwrap();
        assert_eq!(database.migrate(graph).await.unwrap(), 0);
        let (alice, token) = user(&database, "alice").await;
        let (_, bob_token) = user(&database, "bob").await;
        let hidden = database
            .create::<acl::Note>()
            .set(acl::Note::TITLE, "hidden")
            .execute()
            .await
            .unwrap();
        let visible = database
            .create::<acl::Note>()
            .set(acl::Note::TITLE, "visible")
            .execute()
            .await
            .unwrap();
        let id = visible.id;
        database
            .transaction_result(move |tx| {
                Box::pin(async move {
                    Access::<acl::Note>::grant(tx, Subject::User(alice), VIEW, id).await
                })
            })
            .await
            .unwrap();
        let router = Server::new(AppState::from_database(database.clone()))
            .install(
                InstalledApps::new()
                    .add(che_rest::auth::module())
                    .add(acl::NotesApp),
            )
            .build()
            .await
            .unwrap();
        let (status, page) = send(&router, &token, "GET", "/api/notes/?limit=1", Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["count"], 1);
        assert_eq!(page["results"][0]["id"], id);
        assert_eq!(
            send(
                &router,
                &token,
                "GET",
                &format!("/api/notes/{}/", hidden.id),
                Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                &bob_token,
                "GET",
                &format!("/api/notes/{id}/"),
                Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        database
            .transaction_result(move |tx| {
                Box::pin(async move {
                    Access::<acl::Note>::revoke(tx, Subject::User(alice), VIEW, id).await
                })
            })
            .await
            .unwrap();
        let (_, page) = send(&router, &token, "GET", "/api/notes/", Value::Null).await;
        assert_eq!(page["count"], 0);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL"]
async fn postgres_model_sync_http_writes_are_journaled() {
    support::isolated(|database, _| async move {
        migrate_fixture(&database, &notes::NotesApp.schema(), "notes").await;
        let (_, token) = user(&database, "sync-user").await;
        let router = Server::new(AppState::from_database(database.clone()))
            .install(notes::apps())
            .build()
            .await
            .unwrap();
        assert_eq!(
            send(&router, "", "GET", "/api/notes/", Value::Null).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, created) = send(
            &router,
            &token,
            "POST",
            "/api/notes/",
            json!({"title":"postgres","done":false,"document":{"x":[1,true,null]},"number":42}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["id"].as_str().unwrap();
        let (status, updated) = send(
            &router,
            &token,
            "PATCH",
            &format!("/api/notes/{id}/"),
            json!({"done":true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["done"], true);
        let reader = che_sync::SyncServer::read_model_logs(
            database.clone(),
            notes::registry(che_sync::SchemaRole::Server),
            "pg-test",
            std::sync::Arc::new(che_sync::AllowAll),
        )
        .unwrap();
        let page = reader.changes::<notes::Note>(0, 100).await.unwrap();
        assert_eq!(page.operations.len(), 2);
        assert_eq!(
            send(
                &router,
                &token,
                "DELETE",
                &format!("/api/notes/{id}/"),
                Value::Null
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        let page = reader.changes::<notes::Note>(0, 100).await.unwrap();
        assert_eq!(page.operations.len(), 3);
    })
    .await;
}

#[cfg(feature = "sync-http")]
#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL"]
async fn postgres_native_http_transport_preserves_pending_work_after_auth_failure() {
    use che_rest::sync_http::{RestModelTransport, RestResource};
    use che_sync::{ModelLogClient, SchemaRole};
    support::isolated(|server_db, _| async move {
        migrate_fixture(&server_db, &notes::NotesApp.schema(), "notes").await;
        let (_, token) = user(&server_db, "transport-user").await;
        let router = Server::new(AppState::from_database(server_db.clone()))
            .install(notes::apps())
            .build()
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api", listener.local_addr().unwrap());
        struct AbortServer(tokio::task::JoinHandle<()>);
        impl Drop for AbortServer {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _server = AbortServer(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
        support::isolated(|client_db, _| async move {
            let schema = SchemaSet::new().client_model::<notes::Note>();
            let state = ProjectState::from_app_schemas([("notes", &schema)]).unwrap();
            let operations = state
                .tables()
                .iter()
                .cloned()
                .map(|table| MigrationOperation::CreateTable { table })
                .collect();
            client_db
                .migrate(
                    MigrationGraph::new(vec![Migration::new(
                        MigrationId {
                            app: "notes".into(),
                            name: "0001_client_fixture".into(),
                        },
                        vec![],
                        operations,
                    )])
                    .unwrap(),
                )
                .await
                .unwrap();
            let client =
                ModelLogClient::open(client_db.clone(), notes::registry(SchemaRole::Client))
                    .await
                    .unwrap();
            let transport = |http| {
                RestModelTransport::new(
                    &base,
                    http,
                    vec![RestResource::new::<notes::Note, notes::NoteSerializer>("notes").unwrap()],
                )
                .unwrap()
            };
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("authorization", format!("Token {token}").parse().unwrap());
            let http = reqwest::Client::builder()
                .default_headers(headers)
                .build()
                .unwrap();
            client.engine(transport(http.clone())).pull().await.unwrap();
            let note = notes::Note {
                id: che_orm::Uuid::now_v7(),
                title: "offline postgres".into(),
                done: false,
                document: Some(che_orm::JsonField(json!({"nested":[true,null]}))),
                number: i64::MAX,
            };
            client.save(&note).await.unwrap();
            assert!(
                client
                    .engine(transport(reqwest::Client::new()))
                    .flush()
                    .await
                    .is_err()
            );
            drop(client);
            let client = ModelLogClient::open(client_db, notes::registry(SchemaRole::Client))
                .await
                .unwrap();
            let engine = client.engine(transport(http));
            assert_eq!(engine.flush().await.unwrap().accepted_operations, 1);
            engine.pull().await.unwrap();
            let saved = server_db
                .get::<notes::Note>(note.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(saved.title, note.title);
            assert_eq!(saved.number, i64::MAX);
            assert_eq!(engine.flush().await.unwrap().accepted_operations, 0);
        })
        .await;
    })
    .await;
}

#[derive(Debug, Model)]
#[orm(table = "pg_photos")]
struct Photo {
    #[orm(primary_key)]
    id: i64,
    image: Option<ImageField>,
}
#[derive(ModelSerializer)]
#[serializer(model=Photo)]
struct PhotoSerializer {
    #[serializer(read_only)]
    id: i64,
    image: Option<ImageField>,
}
#[derive(Clone)]
struct Photos;
impl che_rest::ViewSet for Photos {
    type Model = Photo;
    type Serializer = PhotoSerializer;
    type ListSerializer = PhotoSerializer;
    type QuerySet = DatabaseQuery<Photo>;
    type FilterSet = che_rest::FilterSet<Photo>;
    type Permission = che_rest::AllowAny;
    fn path(&self) -> &'static str {
        "/photos"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Photo::query())
    }
    fn configure(&self, config: &mut che_rest::ViewSetConfig<Self>) -> che_rest::AppResult<()> {
        config.mutation_hook(che_rest::media::ImageCleanup {
            id: "pg-photo-image",
            image: |photo: &Photo| photo.image.as_ref(),
        })?;
        config.image_field(che_rest::media::ImageFieldEndpoint::new(
            "image",
            Photo::IMAGE,
            |photo: &Photo| photo.image.as_ref(),
            che_rest::media::ImagePolicy::new("photos", 1_000_000, 32),
        ))
    }
}
struct PhotoApp;
impl AppModule for PhotoApp {
    fn name(&self) -> &'static str {
        "photos"
    }
    fn schema(&self) -> SchemaSet {
        SchemaSet::new().model::<Photo>()
    }
    fn init(&self, ctx: &mut ModuleContext) {
        ctx.viewset_with(Photos);
    }
}

#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL"]
async fn postgres_image_upload_clear_and_delete() {
    support::isolated(|database, _| async move {
        migrate_fixture(&database,&PhotoApp.schema(),"photos").await;
        let (_,token)=user(&database,"image-user").await;
        let mut state=AppState::from_database(database.clone());
        let root=std::env::temp_dir().join(format!("che-rest-pg-media-{}",che_orm::Uuid::now_v7()));
        state.config.media.root=root.to_string_lossy().into_owned();
        let router=Server::new(state.clone()).install(InstalledApps::new().add(che_rest::auth::module()).add(PhotoApp).add(che_rest::media::module())).build().await.unwrap();
        let photo=database.create::<Photo>().set(Photo::IMAGE,None::<ImageField>).execute().await.unwrap();
        let mut png=std::io::Cursor::new(Vec::new());image::DynamicImage::new_rgb8(1,1).write_to(&mut png,image::ImageFormat::Png).unwrap();
        let mut body=b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();body.extend(png.into_inner());body.extend(b"\r\n--boundary--\r\n");
        let response=router.clone().oneshot(Request::builder().method("PUT").uri(format!("/api/photos/{}/images/image/",photo.id)).header("authorization",format!("Token {token}")).header("content-type","multipart/form-data; boundary=boundary").body(Body::from(body)).unwrap()).await.unwrap();
        let status=response.status();let bytes=to_bytes(response.into_body(),usize::MAX).await.unwrap();
        assert_eq!(status,StatusCode::OK,"{}",String::from_utf8_lossy(&bytes));
        let key=database.get::<Photo>(photo.id).await.unwrap().unwrap().image.unwrap();
        let storage=che_rest::media::ImageStorage::from_state(&state);assert!(storage.read(&key).is_ok());
        let response=router.clone().oneshot(Request::builder().method("DELETE").uri(format!("/api/photos/{}/images/image/",photo.id)).header("authorization",format!("Token {token}")).header("content-type","multipart/form-data; boundary=boundary").body(Body::from("--boundary--\r\n")).unwrap()).await.unwrap();
        assert_eq!(response.status(),StatusCode::NO_CONTENT);assert!(database.get::<Photo>(photo.id).await.unwrap().unwrap().image.is_none());assert!(storage.read(&key).is_err());
        assert_eq!(send(&router,&token,"DELETE",&format!("/api/photos/{}/",photo.id),Value::Null).await.0,StatusCode::NO_CONTENT);
        if root.exists(){std::fs::remove_dir_all(root).unwrap();}
    }).await;
}
