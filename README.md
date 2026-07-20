# che-rest

Base REST layer for `che-orm` applications.

## Management

Applications define one installed app list and use it for both the API server and management commands:

```rust
pub mod users;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new().add(users::module())
}
```

Server startup:

```rust
let app = Server::new(state)
    .install(apps::installed_apps())
    .build()
    .await?;
```

Project-local `src/bin/manage.rs`:

```rust
use che_rest::Management;
use simple_rest_demo::apps;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Management::new(apps::installed_apps()).run().await
}
```

Create a new app module in `src/apps/<name>`:

```bash
cargo run --bin manage -- startapp users
```

Generated files:

```text
src/apps/
  mod.rs
  users/
    mod.rs
    models.rs
    serializers.rs
    filters.rs
    views.rs
```

Then add the app to `apps::installed_apps()`:

```rust
InstalledApps::new().add(users::module())
```

Create app-scoped migrations from the installed app metadata:

```bash
cargo run --bin manage -- makemigrations users
```

Generate TypeScript models and API client from installed app metadata:

```bash
cargo run --bin manage -- generate-ts --out src/generated
```

This writes:

```text
src/generated/
  api_client.ts
  models.ts
  api.ts
```

Defaults:

```text
--name auto
```

Generated migrations are stored under:

```text
src/apps/users/migrations/
```

Apply migrations for one app. The database URL is read from `[database].url` in `app.toml`:

```bash
cargo run --bin manage -- migrate users
```

Apply migrations for all installed apps:

```bash
cargo run --bin manage -- migrate
```

You can override the config database URL:

```bash
cargo run --bin manage -- migrate users \
  --database-url sqlite://db.sqlite?mode=rwc
```
