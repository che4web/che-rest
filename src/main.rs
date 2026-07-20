#![allow(dead_code)]

mod config;
mod error;
mod serializer;
mod serializers;
mod state;

use che_orm::Model;
use error::AppResult;
use state::AppState;

#[derive(Debug, Clone, Model)]
#[model(table = "users")]
struct User {
    #[field(primary_key)]
    id: i64,

    #[field(unique, max_length = 255)]
    email: String,

    name: String,

    #[field(default = true)]
    is_active: bool,
}

#[tokio::main]
async fn main() -> AppResult<()> {
    let config_path = std::env::var("APP_CONFIG").unwrap_or_else(|_| "app.toml".to_string());
    let state = AppState::from_config_file(config_path).await?;
    let db = state.db();

    db.create_table::<User>().await?;

    let user = User::objects(&db)
        .create()
        .set("email", "alice@example.com")
        .set("name", "Alice")
        .set("is_active", true)
        .execute()
        .await?;

    println!("created: {user:?}");

    let mut fetched = User::objects(&db).get(user.id).await?;
    println!("fetched: {fetched:?}");
    println!(
        "public json: {}",
        serializers::UserPublicSerializer::to_json(&fetched)
    );

    fetched.name = "Alicia".to_string();
    fetched.is_active = false;
    let updated = fetched.save(&db).await?;
    println!("updated: {updated:?}");

    let users = User::objects(&db).all().await?;
    println!("all users: {users:?}");

    User::objects(&db).delete(user.id).await?;
    println!("deleted user id {}", user.id);

    Ok(())
}
