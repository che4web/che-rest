//! Run: cargo run --example object_permissions
//! Applies frozen migrations to an isolated in-memory demo database, then exercises REST.
#[path = "object_permissions/migration.rs"]
mod migration;
#[path = "object_permissions/models.rs"]
mod models;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use che_orm::{Database, MigrationGraph};
use che_rest::auth::{
    self, AuthToken, Group, User,
    access::{ADD, Access, Subject, VIEW, add_member, remove_member},
};
use che_rest::{AppError, AppModule, AppState, InstalledApps, Server};
use models::{Note, NotesApp};
use serde_json::json;
use tower::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = Database::connect_in_memory().await?;
    // Explicit migration step. Real applications execute this through their management CLI.
    let mut history = auth::migrations();
    history.extend(NotesApp.migrations());
    database.migrate(MigrationGraph::new(history)?).await?;
    let state = AppState::from_database(database);
    let alice = state
        .database()
        .create::<User>()
        .set(User::USERNAME, "alice")
        .set(User::PASSWORD_HASH, "unused")
        .execute()
        .await?;
    let bob = state
        .database()
        .create::<User>()
        .set(User::USERNAME, "bob")
        .set(User::PASSWORD_HASH, "unused")
        .execute()
        .await?;
    for (user, token) in [(alice.id, "alice-demo"), (bob.id, "bob-demo")] {
        state
            .database()
            .create::<AuthToken>()
            .set(AuthToken::USER_ID, user)
            .set(AuthToken::KEY_HASH, auth::token_hash(token))
            .execute()
            .await?;
    }
    let group = state
        .database()
        .create::<Group>()
        .set(Group::NAME, "readers")
        .execute()
        .await?
        .id;
    let alice_id = alice.id;
    let bob_id = bob.id;
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move {
                Access::<Note>::grant_model(tx, Subject::User(alice_id), ADD).await?;
                add_member(tx, bob_id, group).await?;
                Ok::<_, AppError>(())
            })
        })
        .await?;
    let router = Server::new(state.clone())
        .install(InstalledApps::new().add(auth::module()).add(NotesApp))
        .build()
        .await?;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/notes/")
                .header("authorization", "Token alice-demo")
                .header("content-type", "application/json")
                .body(Body::from(json!({"title":"One shared object"}).to_string()))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
    let id = created["id"].as_i64().expect("created note ID");
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(
                async move { Access::<Note>::grant(tx, Subject::Group(group), VIEW, id).await },
            )
        })
        .await?;
    let read = || {
        Request::builder()
            .uri(format!("/api/notes/{id}/"))
            .header("authorization", "Token bob-demo")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        router.clone().oneshot(read()).await?.status(),
        StatusCode::OK
    );
    state
        .database()
        .transaction_result(move |tx| {
            Box::pin(async move { remove_member(tx, bob_id, group).await })
        })
        .await?;
    assert_eq!(
        router.clone().oneshot(read()).await?.status(),
        StatusCode::NOT_FOUND
    );
    println!(
        "Alice created note {id}; Bob read it through a group; removing membership revoked access."
    );
    Ok(())
}
