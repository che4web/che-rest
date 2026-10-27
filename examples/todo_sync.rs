//! Run: cargo run --example todo_sync
#[path = "todo_sync/support.rs"]
mod todo;

use che_orm::{OrmError, Uuid};
use che_rest::Server;
use che_sync::{SchemaRole, SyncClient, SyncEngine, SyncTransport, TransportFuture, protocol::*};
use serde::{Serialize, de::DeserializeOwned};

struct HttpTransport {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl HttpTransport {
    async fn write(&self, request: PushRequest) -> Result<PushResult, OrmError> {
        let model = request
            .operation
            .mutations
            .first()
            .ok_or_else(|| OrmError::Configuration("empty operation".into()))?
            .model
            .clone();
        let (resource, write) = match model.as_str() {
            "projects.project" => (
                "projects",
                che_rest::RestSyncRequest::new::<todo::Project, todo::ProjectSerializer>(request)?,
            ),
            "todo.todo" => (
                "todos",
                che_rest::RestSyncRequest::new::<todo::Todo, todo::TodoSerializer>(request)?,
            ),
            _ => return Err(OrmError::Configuration("unknown sync model".into())),
        };
        let base = self
            .base
            .replace("/todos/sync", &format!("/{resource}"))
            .replace("/projects/sync", &format!("/{resource}"));
        let url = if write.method == reqwest::Method::POST {
            format!("{base}/")
        } else {
            format!("{base}/{}/", write.entity_id)
        };
        let response = self
            .http
            .request(write.method, url)
            .headers(write.headers)
            .header("Authorization", format!("Token {}", self.token))
            .json(&write.body)
            .send()
            .await
            .map_err(network)?;
        if response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|value| value.split(';').next())
            == Some("application/vnd.che-sync+json")
        {
            response.json().await.map_err(network)
        } else {
            let status = response.status();
            // Only a sync envelope is an authoritative business rejection.
            // Proxy, throttling and authentication errors leave the immutable
            // operation in the outbox. The caller schedules retry/auth renewal.
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unspecified");
            Err(OrmError::Interaction(format!(
                "unexpected REST response {status}; Retry-After: {retry_after}"
            )))
        }
    }
    async fn post<Q: Serialize, R: DeserializeOwned>(
        &self,
        action: &str,
        request: Q,
    ) -> Result<R, OrmError> {
        self.http
            .post(format!("{}/{action}/", self.base))
            .header("Authorization", format!("Token {}", self.token))
            .json(&request)
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?
            .json()
            .await
            .map_err(network)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    };

    #[tokio::test]
    async fn non_sync_http_errors_preserve_outbox_identity_until_retry() {
        let (state, token) = todo::demo_state().await.unwrap();
        let status = Arc::new(AtomicU16::new(200));
        let failure = status.clone();
        let router = Server::new(state.clone())
            .install(todo::apps())
            .build()
            .await
            .unwrap()
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let failure = failure.clone();
                    async move {
                        let code = failure.load(Ordering::SeqCst);
                        if request.uri().path() == "/api/todos/" && code != 200 {
                            if code == 422 {
                                return axum::response::IntoResponse::into_response((
                                    axum::http::StatusCode::BAD_REQUEST,
                                    [(
                                        "content-type",
                                        "application/vnd.che-sync+json; charset=utf-8",
                                    )],
                                    axum::Json(PushResult::Rejected {
                                        reason: "business validation failed".into(),
                                    }),
                                ));
                            }
                            return axum::response::IntoResponse::into_response((
                                axum::http::StatusCode::from_u16(code).unwrap(),
                                [("retry-after", "0")],
                                "temporary HTTP error",
                            ));
                        }
                        next.run(request).await
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/todos/sync", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let client = SyncClient::open(
            todo::database(SchemaRole::Client).await.unwrap(),
            todo::registry().unwrap(),
            "todo-board",
        )
        .await
        .unwrap();
        let engine = SyncEngine::for_model::<todo::Todo>(
            client.clone(),
            HttpTransport {
                http: reqwest::Client::new(),
                base,
                token,
            },
        )
        .unwrap();
        engine.full_load().await.unwrap();
        let item = todo::Todo {
            id: Uuid::now_v7(),
            title: "retry me".into(),
            ..Default::default()
        };
        client.save(&item).await.unwrap();
        let mut identity = None;
        for code in [401, 408, 429, 503, 400] {
            status.store(code, Ordering::SeqCst);
            let error = engine.flush().await.unwrap_err();
            assert!(error.to_string().contains("Retry-After: 0"));
            let rows = client
                .database()
                .query_sql("SELECT operation_id, payload FROM _sync_outbox", vec![])
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            let pending = (
                rows[0].get::<String>(0).unwrap(),
                rows[0].get::<String>(1).unwrap(),
            );
            if let Some(expected) = &identity {
                assert_eq!(&pending, expected);
            } else {
                identity = Some(pending);
            }
        }
        status.store(200, Ordering::SeqCst);
        engine.flush().await.unwrap();
        assert!(
            client
                .database()
                .query_sql("SELECT operation_id FROM _sync_outbox", vec![])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            state
                .database()
                .get::<todo::Todo>(item.id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "retry me"
        );
        let rejected = todo::Todo {
            id: Uuid::now_v7(),
            title: "rejected".into(),
            ..Default::default()
        };
        client.save(&rejected).await.unwrap();
        status.store(422, Ordering::SeqCst);
        assert_eq!(engine.flush().await.unwrap().rejected_operations, 1);
        assert!(
            client
                .database()
                .query_sql("SELECT operation_id FROM _sync_outbox", vec![])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .database()
                .get::<todo::Todo>(rejected.id)
                .await
                .unwrap()
                .is_none()
        );
        server.abort();
    }
}
fn network(error: reqwest::Error) -> OrmError {
    OrmError::Interaction(error.to_string())
}
impl SyncTransport for HttpTransport {
    fn max_mutations(&self) -> usize {
        1
    }
    fn full_load(&self) -> TransportFuture<'_, Snapshot> {
        Box::pin(async move {
            match self
                .post(
                    "pull",
                    PullRequest {
                        protocol_version: PROTOCOL_VERSION,
                        cursor: None,
                        limit: 100,
                    },
                )
                .await?
            {
                PullResult::Full(snapshot) => Ok(snapshot),
                _ => Err(OrmError::Interaction("expected full load".into())),
            }
        })
    }
    fn push_model(&self, _model: &str, request: PushRequest) -> TransportFuture<'_, PushResult> {
        Box::pin(self.write(request))
    }
    fn push(&self, request: PushRequest) -> TransportFuture<'_, PushResult> {
        Box::pin(self.write(request))
    }
    fn pull(&self, request: PullRequest) -> TransportFuture<'_, PullResult> {
        Box::pin(self.post("pull", request))
    }
    fn snapshot(&self) -> TransportFuture<'_, Snapshot> {
        Box::pin(async move {
            self.http
                .get(format!("{}/snapshot/", self.base))
                .header("Authorization", format!("Token {}", self.token))
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?
                .json()
                .await
                .map_err(network)
        })
    }
}

async fn drain(engine: &SyncEngine<HttpTransport>) -> Result<(), OrmError> {
    loop {
        let report = engine.sync_once().await?;
        if report.conflicts > 0 || report.blocked {
            return Err(OrmError::Configuration(
                "resolve conflicts before continuing".into(),
            ));
        }
        if !report.has_more {
            return Ok(());
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (state, token) = todo::demo_state().await?;
    let router = Server::new(state).install(todo::apps()).build().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/api/todos/sync", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let a = SyncClient::open(
        todo::database(SchemaRole::Client).await?,
        todo::registry()?,
        "todo-board",
    )
    .await?;
    let b = SyncClient::open(
        todo::database(SchemaRole::Client).await?,
        todo::registry()?,
        "todo-board",
    )
    .await?;
    let transport = || HttpTransport {
        http: reqwest::Client::new(),
        base: base.clone(),
        token: token.clone(),
    };
    let ea = SyncEngine::for_model::<todo::Todo>(a.clone(), transport())?;
    let eb = SyncEngine::for_model::<todo::Todo>(b.clone(), transport())?;

    let project = todo::Project {
        id: Uuid::now_v7(),
        title: "Errands".into(),
        ..Default::default()
    };
    a.save(&project).await?;
    let item = todo::Todo {
        id: Uuid::now_v7(),
        title: "Buy milk".into(),
        done: false,
        project_id: Some(project.id),
        ..Default::default()
    };
    a.save(&item).await?; // Entirely offline; no HTTP request.
    println!(
        "Created offline: {:?}",
        a.database().get::<todo::Todo>(item.id).await?
    );
    drain(&ea).await?;
    drain(&eb).await?;
    println!(
        "FK parent synchronized automatically: {:?}",
        b.database().get::<todo::Project>(project.id).await?
    );

    // Independent edits made offline on two devices merge automatically.
    let mut first = a
        .database()
        .get::<todo::Todo>(item.id)
        .await?
        .ok_or("device A todo is missing")?;
    let mut second = b
        .database()
        .get::<todo::Todo>(item.id)
        .await?
        .ok_or("device B todo is missing")?;
    first.title = "Buy milk and bread".into();
    second.done = true;
    // Legacy models expose service columns, so the sync facade selects writable fields.
    a.save(&first).await?;
    b.save(&second).await?;
    drain(&ea).await?;
    drain(&eb).await?;
    drain(&ea).await?;
    println!(
        "Merged over HTTP: {:?}",
        a.database().get::<todo::Todo>(item.id).await?
    );

    // Project has its own endpoint, cursor and outgoing queue. Existing parent
    // edits are sent by its own engine, rather than by the Todo engine.
    let mut project = a
        .database()
        .get::<todo::Project>(project.id)
        .await?
        .ok_or("project is missing")?;
    project.title = "Home errands".into();
    a.save(&project).await?;
    let ep = SyncEngine::for_model::<todo::Project>(
        a.clone(),
        HttpTransport {
            http: reqwest::Client::new(),
            base: base.replace("/todos/", "/projects/"),
            token: token.clone(),
        },
    )?;
    drain(&ep).await?;
    drain(&eb).await?;
    println!(
        "Independent Project feed: {:?}",
        b.database().get::<todo::Project>(project.id).await?
    );

    b.delete::<todo::Todo>(item.id).await?;
    drain(&eb).await?;
    drain(&ea).await?;
    assert!(a.database().get::<todo::Todo>(item.id).await?.is_none());
    println!("Offline delete synchronized to both devices.");
    server.abort();
    Ok(())
}
