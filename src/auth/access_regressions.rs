//! Regressions for the access-control review: media isolation and fail-closed policies.
use super::*;
use std::marker::PhantomData;

#[derive(Clone)]
struct RestrictedDocuments<P: Permission<Document>> {
    path: &'static str,
    name_filter: Option<&'static str>,
    permission: PhantomData<P>,
}
impl<P: Permission<Document>> ViewSet for RestrictedDocuments<P> {
    type Model = Document;
    type Serializer = DocumentSerializer;
    type ListSerializer = DocumentSerializer;
    type QuerySet = DatabaseQuery<Document>;
    type FilterSet = FilterSet<Document>;
    type Permission = P;
    fn path(&self) -> &'static str {
        self.path
    }
    fn get_queryset(&self) -> Self::QuerySet {
        let query = Document::query();
        DatabaseQuery::new(match self.name_filter {
            Some(name) => query.filter(Document::NAME.eq(name)),
            None => query,
        })
    }
}
struct RestrictedApp<P: Permission<Document>> {
    views: Vec<(&'static str, Option<&'static str>)>,
    permission: PhantomData<P>,
}
impl<P: Permission<Document>> AppModule for RestrictedApp<P> {
    fn name(&self) -> &'static str {
        "access_regressions"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        TestApp.schema()
    }
    fn init(&self, context: &mut ModuleContext) {
        for &(path, name_filter) in &self.views {
            context.viewset_with(RestrictedDocuments::<P> {
                path,
                name_filter,
                permission: PhantomData,
            });
        }
    }
}
fn apps<P: Permission<Document>>(
    views: Vec<(&'static str, Option<&'static str>)>,
) -> InstalledApps {
    InstalledApps::new()
        .add(module())
        .add(RestrictedApp::<P> {
            views,
            permission: PhantomData,
        })
        .add(crate::media::module())
}
async fn build<P: Permission<Document>>(state: &AppState, filter: Option<&'static str>) -> Router {
    Server::new(state.clone())
        .install(apps::<P>(vec![("/documents", filter)]))
        .build()
        .await
        .unwrap()
}
async fn fixture() -> (AppState, i64, i64, std::path::PathBuf) {
    let (seed, _, alice, _) = setup().await;
    let mut state = AppState::from_database(seed.database().clone());
    let root = std::env::temp_dir().join(format!("che-rest-access-regression-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("private.png"), b"private-image").unwrap();
    state.config.media.root = root.to_string_lossy().into();
    let id = state
        .database()
        .create::<Document>()
        .set(Document::NAME, "hidden")
        .set(Document::IMAGE, Some(ImageField::new("private.png")))
        .execute()
        .await
        .unwrap()
        .id;
    (state, alice, id, root)
}
async fn media(app: &Router) -> StatusCode {
    send(
        app,
        "alice",
        "GET",
        "/api/media/private.png",
        serde_json::json!({}),
    )
    .await
    .0
}
#[tokio::test]
async fn media_respects_the_registered_viewset_instances_queryset() {
    let (state, alice, id, root) = fixture().await;
    allow(&state, Subject::User(alice), VIEW, id).await;
    let app = build::<Access<Document>>(&state, Some("visible")).await;
    assert_eq!(
        send(
            &app,
            "alice",
            "GET",
            &format!("/api/documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(media(&app).await, StatusCode::NOT_FOUND);
    // The same concrete ViewSet type with another instance-owned filter permits it.
    let visible = build::<Access<Document>>(&state, Some("hidden")).await;
    assert_eq!(media(&visible).await, StatusCode::OK);
    assert_eq!(media(&app).await, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn router_builds_do_not_share_permissive_media_readers() {
    let (state, _, id, root) = fixture().await;
    let public = build::<crate::AllowAny>(&state, None).await;
    let protected = build::<Access<Document>>(&state, None).await;
    assert_eq!(
        send(
            &protected,
            "alice",
            "GET",
            &format!("/api/documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(media(&protected).await, StatusCode::NOT_FOUND);
    assert_eq!(media(&public).await, StatusCode::OK);
    drop(public);
    assert_eq!(media(&protected).await, StatusCode::NOT_FOUND);
    // Metadata discovery and a later public router must not mutate this server either.
    apps::<crate::AllowAny>(vec![("/documents", None)]).api_endpoints(state.clone());
    let public = build::<crate::AllowAny>(&state, None).await;
    assert_eq!(media(&public).await, StatusCode::OK);
    assert_eq!(media(&protected).await, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn media_readers_for_multiple_viewsets_in_one_build_are_preserved() {
    let (state, alice, id, root) = fixture().await;
    allow(&state, Subject::User(alice), VIEW, id).await;
    let app = Server::new(state.clone())
        .install(apps::<Access<Document>>(vec![
            ("/visible-documents", Some("visible")),
            ("/hidden-documents", Some("hidden")),
        ]))
        .build()
        .await
        .unwrap();
    assert_eq!(
        send(
            &app,
            "alice",
            "GET",
            &format!("/api/visible-documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "alice",
            "GET",
            &format!("/api/hidden-documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(media(&app).await, StatusCode::OK);
    std::fs::remove_dir_all(root).unwrap();
}
#[derive(Clone, Default)]
struct DenyObjects;
impl Permission<Document> for DenyObjects {
    async fn check_request(
        &self,
        _: crate::PermissionContext<'_>,
        current: Option<&CurrentPrincipal>,
        _: ViewAction,
    ) -> AppResult<()> {
        current.ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
        Ok(())
    }
    async fn check_object(
        &self,
        _: crate::PermissionContext<'_>,
        _: Option<&CurrentPrincipal>,
        _: ViewAction,
        _: &Document,
    ) -> AppResult<()> {
        Err(AppError::Forbidden("no object may be read".into()))
    }
}
#[tokio::test]
async fn an_object_policy_without_sql_scope_cannot_disclose_a_list_or_count() {
    let (state, _, id, root) = fixture().await;
    let app = build::<DenyObjects>(&state, None).await;
    assert_eq!(
        send(
            &app,
            "alice",
            "GET",
            &format!("/api/documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = send(
        &app,
        "alice",
        "GET",
        "/api/documents/",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.get("count").is_none());
    assert!(body.get("results").is_none());
    assert_eq!(media(&app).await, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone)]
struct BackendDocuments;
struct BackendScope;
impl crate::MutationBackend<BackendDocuments> for BackendScope {
    fn filter_read(&self, query: DatabaseQuery<Document>) -> DatabaseQuery<Document> {
        query.filter(Document::NAME.eq("visible"))
    }
    fn save<'a>(
        &'a self,
        _: &'a AppState,
        _: Option<&'a CurrentPrincipal>,
        _: crate::ValidatedWrite<Document>,
    ) -> crate::MutationFuture<'a, Option<Document>> {
        Box::pin(async { Err(AppError::Forbidden("read-only test backend".into())) })
    }
    fn delete<'a>(
        &'a self,
        _: &'a AppState,
        _: Option<&'a CurrentPrincipal>,
        _: i64,
    ) -> crate::MutationFuture<'a, ()> {
        Box::pin(async { Err(AppError::Forbidden("read-only test backend".into())) })
    }
}
impl crate::MutationHook<BackendDocuments> for BackendScope {
    fn id(&self) -> &'static str {
        "backend-read-scope"
    }
    fn backend(&self) -> Option<&dyn crate::MutationBackend<BackendDocuments>> {
        Some(self)
    }
}
impl ViewSet for BackendDocuments {
    type Model = Document;
    type Serializer = DocumentSerializer;
    type ListSerializer = DocumentSerializer;
    type QuerySet = DatabaseQuery<Document>;
    type FilterSet = FilterSet<Document>;
    type Permission = IsAuthenticated;
    fn path(&self) -> &'static str {
        "/backend-documents"
    }
    fn get_queryset(&self) -> Self::QuerySet {
        DatabaseQuery::new(Document::query())
    }
    fn configure(&self, config: &mut crate::ViewSetConfig<Self>) -> AppResult<()> {
        config.mutation_hook(BackendScope)
    }
}
struct BackendApp;
impl AppModule for BackendApp {
    fn name(&self) -> &'static str {
        "backend_regression"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        TestApp.schema()
    }
    fn init(&self, context: &mut ModuleContext) {
        // The ACL view enables strict media resolution but has no grants in this test.
        context.viewset_with(Documents);
        context.viewset_with(BackendDocuments);
    }
}
#[tokio::test]
async fn media_preserves_mutation_backend_read_filters() {
    let (state, _, id, root) = fixture().await;
    let app = Server::new(state.clone())
        .install(
            InstalledApps::new()
                .add(module())
                .add(BackendApp)
                .add(crate::media::module()),
        )
        .build()
        .await
        .unwrap();
    assert_eq!(
        send(
            &app,
            "alice",
            "GET",
            &format!("/api/backend-documents/{id}/"),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(media(&app).await, StatusCode::NOT_FOUND);
    state
        .database()
        .update::<Document>(id)
        .set(Document::NAME, "visible")
        .execute()
        .await
        .unwrap();
    assert_eq!(media(&app).await, StatusCode::OK);
    std::fs::remove_dir_all(root).unwrap();
}
