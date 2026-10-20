//! Run: cargo run --features sync-http --example model_sync
#[path = "model_sync/support.rs"]
mod notes;

use che_orm::Uuid;
use che_rest::{
    Server,
    sync_http::{RestModelTransport, RestResource},
};
use che_sync::{ModelLogClient, SchemaRole};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (state, token) = notes::demo_state().await?;
    let router = Server::new(state).install(notes::apps()).build().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/api", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Token {token}").parse()?);
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;
    let transport = || {
        RestModelTransport::new(
            &base,
            http.clone(),
            vec![RestResource::new::<notes::Note, notes::NoteSerializer>("notes").unwrap()],
        )
    };
    let a = ModelLogClient::open(
        notes::database(SchemaRole::Client).await?,
        notes::registry(SchemaRole::Client),
    )
    .await?;
    let b = ModelLogClient::open(
        notes::database(SchemaRole::Client).await?,
        notes::registry(SchemaRole::Client),
    )
    .await?;
    let device_a = a.engine(transport()?);
    let device_b = b.engine(transport()?);
    device_a.pull().await?;
    device_b.pull().await?;
    let mut note = notes::Note {
        id: Uuid::now_v7(),
        title: "draft".into(),
        done: false,
        document: Some(che_orm::JsonField(json!({"labels":["offline"]}))),
        number: i64::MAX,
    };
    for n in 0..10 {
        note.title = format!("offline draft {n}");
        a.save(&note).await?;
    }
    assert_eq!(device_a.flush().await?.accepted_operations, 1);
    device_b.pull().await?;
    let mut other = b.database().get::<notes::Note>(note.id).await?.unwrap();
    note.title = "edited by A".into();
    other.done = true;
    a.save(&note).await?;
    b.save(&other).await?;
    device_a.flush().await?;
    device_b.pull().await?;
    device_b.flush().await?;
    device_a.pull().await?;
    let merged = a.database().get::<notes::Note>(note.id).await?.unwrap();
    assert_eq!(merged.title, "edited by A");
    assert!(merged.done);
    assert!(a.conflicts().await?.is_empty());
    assert!(b.conflicts().await?.is_empty());
    println!("Ten offline edits became one POST; independent device edits merged: {merged:?}");
    a.delete::<notes::Note>(note.id).await?;
    device_a.flush().await?;
    device_b.pull().await?;
    assert!(b.database().get::<notes::Note>(note.id).await?.is_none());
    println!("Deletion reached device B through the per-model journal.");
    server.abort();
    Ok(())
}
