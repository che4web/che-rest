#[path = "../examples/todo_sync/support.rs"]
mod todo;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::{OrmError, Uuid};
use che_rest::Server;
use che_sync::{SchemaRole, SyncClient, SyncEngine, SyncTransport, TransportFuture, protocol::*};
use serde::{Serialize, de::DeserializeOwned};
use tower::ServiceExt;

struct Transport {
    router: Router,
    token: String,
    resource: &'static str,
}
impl Transport {
    async fn write(&self, request: PushRequest) -> Result<PushResult, OrmError> {
        let (resource, write) = match request.operation.mutations[0].model.as_str() {
            "projects.project" => (
                "projects",
                che_rest::RestSyncRequest::new::<todo::Project, todo::ProjectSerializer>(request)?,
            ),
            "todo.todo" => (
                "todos",
                che_rest::RestSyncRequest::new::<todo::Todo, todo::TodoSerializer>(request)?,
            ),
            _ => return Err(OrmError::Configuration("unknown model".into())),
        };
        let uri = if write.method == axum::http::Method::POST {
            format!("/api/{resource}/")
        } else {
            format!("/api/{resource}/{}/", write.entity_id)
        };
        let mut builder = Request::builder()
            .method(write.method)
            .uri(uri)
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "application/json");
        builder.headers_mut().unwrap().extend(write.headers);
        let response = self
            .router
            .clone()
            .oneshot(
                builder
                    .body(Body::from(serde_json::to_vec(&write.body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        match serde_json::from_slice(&bytes) {
            Ok(result) => Ok(result),
            Err(_) if status.is_client_error() => Ok(PushResult::Rejected {
                reason: String::from_utf8_lossy(&bytes).into_owned(),
            }),
            Err(error) => Err(OrmError::Interaction(error.to_string())),
        }
    }
    async fn send<T: DeserializeOwned>(
        &self,
        action: &str,
        body: impl Serialize,
        method: &str,
    ) -> Result<T, OrmError> {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/{}/sync/{action}/", self.resource))
                    .header("Authorization", format!("Token {}", self.token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        if response.status() != StatusCode::OK {
            return Err(OrmError::Interaction(format!("HTTP {}", response.status())));
        }
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .map_err(|error| OrmError::Interaction(error.to_string()))
    }
}
impl SyncTransport for Transport {
    fn max_mutations(&self) -> usize {
        1
    }
    fn full_load(&self) -> TransportFuture<'_, Snapshot> {
        Box::pin(async move {
            match self
                .send(
                    "pull",
                    PullRequest {
                        protocol_version: PROTOCOL_VERSION,
                        cursor: None,
                        limit: 100,
                    },
                    "POST",
                )
                .await?
            {
                PullResult::Full(snapshot) => Ok(snapshot),
                _ => Err(OrmError::Interaction("expected full load".into())),
            }
        })
    }
    fn push_model(&self, model: &str, request: PushRequest) -> TransportFuture<'_, PushResult> {
        let resource = match model {
            "projects.project" => "projects",
            "todo.todo" => "todos",
            _ => return Box::pin(async { Err(OrmError::Configuration("unknown model".into())) }),
        };
        Box::pin(async move {
            Transport {
                router: self.router.clone(),
                token: self.token.clone(),
                resource,
            }
            .write(request)
            .await
        })
    }
    fn push(&self, request: PushRequest) -> TransportFuture<'_, PushResult> {
        Box::pin(self.write(request))
    }
    fn pull(&self, request: PullRequest) -> TransportFuture<'_, PullResult> {
        Box::pin(self.send("pull", request, "POST"))
    }
    fn snapshot(&self) -> TransportFuture<'_, Snapshot> {
        Box::pin(self.send("snapshot", (), "GET"))
    }
}
async fn drain(engine: &SyncEngine<Transport>) {
    for _ in 0..20 {
        let report = engine.sync_once().await.unwrap();
        assert!(!report.blocked);
        if !report.has_more {
            return;
        }
    }
    panic!("HTTP model stream did not converge");
}

#[tokio::test]
async fn todo_endpoint_automatically_synchronizes_fk_parent_and_project_has_its_own_feed() {
    let (state, token) = todo::demo_state().await.unwrap();
    let router = Server::new(state.clone())
        .install(todo::apps())
        .build()
        .await
        .unwrap();
    let client = SyncClient::open(
        todo::database(SchemaRole::Client).await.unwrap(),
        todo::registry().unwrap(),
        "todo-board",
    )
    .await
    .unwrap();
    let transport = |resource| Transport {
        router: router.clone(),
        token: token.clone(),
        resource,
    };
    let projects =
        SyncEngine::for_model::<todo::Project>(client.clone(), transport("projects")).unwrap();
    let todos = SyncEngine::for_model::<todo::Todo>(client.clone(), transport("todos")).unwrap();
    let project = todo::Project {
        id: Uuid::now_v7(),
        title: "auto parent".into(),
        ..Default::default()
    };
    let item = todo::Todo {
        id: Uuid::now_v7(),
        title: "child".into(),
        done: false,
        project_id: Some(project.id),
        ..Default::default()
    };
    client.save(&project).await.unwrap();
    client.save(&item).await.unwrap();
    drain(&todos).await;
    assert!(
        state
            .database()
            .get::<todo::Project>(project.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        state
            .database()
            .get::<todo::Todo>(item.id)
            .await
            .unwrap()
            .is_some()
    );
    let before = transport("projects").snapshot().await.unwrap();
    assert_eq!(before.cursor.model.as_deref(), Some("projects.project"));
    assert_eq!(before.entities.len(), 1);
    assert_eq!(
        transport("todos").snapshot().await.unwrap().entities.len(),
        2
    );
    assert!(
        transport("todos")
            .pull(PullRequest {
                protocol_version: PROTOCOL_VERSION,
                cursor: Some(before.cursor),
                limit: 10
            })
            .await
            .is_err()
    );

    // An existing parent's edit is left for the Project engine, even when a
    // Todo edit is sent at the same time.
    client
        .save(&todo::Project {
            id: project.id,
            title: "independent parent edit".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    client
        .save(&todo::Todo { done: true, ..item })
        .await
        .unwrap();
    drain(&todos).await;
    assert_eq!(
        state
            .database()
            .get::<todo::Project>(project.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "auto parent"
    );
    drain(&projects).await;
    assert_eq!(
        state
            .database()
            .get::<todo::Project>(project.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "independent parent edit"
    );
    // Optional UUID relation querysets are supported by the REST adapter too.
    let queryset = state
        .database()
        .query::<todo::Todo>()
        .select_related(todo::Todo::PROJECT);
    let loaded = che_rest::RestQuerySet::all(queryset, state.database())
        .await
        .unwrap();
    assert_eq!(loaded.len(), 1);
    let reverse = state
        .database()
        .query::<todo::Project>()
        .prefetch_related(todo::Todo::PROJECT.reverse());
    assert_eq!(
        che_rest::RestQuerySet::all(reverse, state.database())
            .await
            .unwrap()
            .len(),
        1
    );
}
