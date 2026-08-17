# Todo API From Scratch

This tutorial creates a small unauthenticated todo API backed by SQLite. It uses the local
development layout of this repository:

```text
che-rest/
che-orm2/
```

## 1. Create A Project

From the `che-rest` repository root, create the project next to the two framework repositories:

```bash
cargo run --bin manage -- startproject todo_api \
  --out .. \
  --che-rest-path ../che-rest \
  --che-orm2-path ../che-orm2
cd ../todo_api
```

The generated `app.toml` points to `sqlite://db.sqlite?mode=rwc`; SQLite will create `db.sqlite`
when migrations are applied. The same file controls the HTTP listener:

```toml
[server]
host = "127.0.0.1"
port = 3000
api_prefix = "/api"
```

Change `host`, `port`, or `api_prefix` there before starting the server. For example, `host =
"0.0.0.0"` exposes the server on all network interfaces.

## 2. Generate The Tasks App

Generate an application module with a `Task` model:

```bash
cargo run --bin manage -- startapp tasks --model Task
```

This creates `src/apps/tasks/` with a model, serializer, filters, and CRUD viewset. The generated
model has `id`, `name`, `created_at`, and `updated_at` fields.

The command also adds `pub mod tasks;` to `src/apps/mod.rs`. Register the module in the same file
so the server and management commands can discover it:

```rust
pub mod tasks;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new().add(tasks::module())
}
```

The generated route is singular. Change it to a plural resource URL in `src/apps/tasks/mod.rs`:

```rust
fn init(&self, ctx: &mut ModuleContext) {
    ctx.viewset_with("/tasks", views::TaskViewSet);
}
```

`viewset_with` registers the router as well as the model schema and OpenAPI metadata. Do not
replace it with `route` for a CRUD model.

## 3. Create The Database Schema

Generate a migration from the registered model, then apply it:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
```

Migrations are stored in the project-level `migrations/` directory. `makemigrations` requires Atlas;
`migrate` applies the checked-in SQL files without Atlas. Run these commands after every model
schema change; starting the server does not create tables.

## 4. Start The API

Start the server:

```bash
cargo run
```

It listens on `http://127.0.0.1:3001`. Swagger UI is at `http://127.0.0.1:3001/api/` and its
OpenAPI document is at `http://127.0.0.1:3001/api/openapi.json`.

The generated viewset uses `AllowAny`, so its API needs no login for this tutorial.

## 5. Use The Todo API

In a second terminal, create a task:

```bash
curl -X POST http://127.0.0.1:3001/api/tasks/ \
  -H 'Content-Type: application/json' \
  -d '{"name":"Buy milk"}'
```

List tasks:

```bash
curl http://127.0.0.1:3001/api/tasks/
```

Filter by a substring in the generated `name` filter:

```bash
curl 'http://127.0.0.1:3001/api/tasks/?name__contains=milk'
```

Update task `1`:

```bash
curl -X PATCH http://127.0.0.1:3001/api/tasks/1/ \
  -H 'Content-Type: application/json' \
  -d '{"name":"Buy oat milk"}'
```

Delete task `1`:

```bash
curl -X DELETE http://127.0.0.1:3001/api/tasks/1/
```

## 6. Customize The Model

Add a completion flag to `src/apps/tasks/models.rs`:

```rust
pub completed: bool,
```

Expose it through the serializer in `src/apps/tasks/serializers.rs`:

```rust
Field::new("completed"),
```

Optionally enable exact filtering in `src/apps/tasks/filters.rs`:

```rust
Filter::exact(TaskFields::COMPLETED),
```

Then generate and apply the next migration:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
```

Use generated `TaskFields` descriptors whenever application code writes a model directly:

```rust
state
    .db()
    .create::<Task>()
    .set(TaskFields::NAME, "Write tests")
    .set(TaskFields::COMPLETED, false)
    .execute()
    .await?;
```

For protected tasks, create the project with `--with-auth`, register `che_rest::auth::module()`,
and change the viewset permission from `AllowAny` to `IsAuthenticated`.
