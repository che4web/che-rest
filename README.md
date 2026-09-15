# che-rest

Breaking v2 REST layer for `che-orm` applications.

The current v2 surface is the typed ORM2 CRUD router. A viewset builds its
database-independent queryset in `get_queryset`; database access happens only
when the queryset is materialized:

```rust
use che_rest::{CrudViewSet, Server};

let app = Server::new(state)
    .install(apps::installed_apps())
    .build()
    .await?;
```

The migration plan is documented in [`docs/ORM2_MIGRATION_PLAN.md`](docs/ORM2_MIGRATION_PLAN.md).

The minimal migrated example is available under `todo_api`:

```bash
cargo run --manifest-path todo_api/Cargo.toml
```

## Start a Project

Create a new runnable `che-rest` application:

```bash
cargo install che-rest --bin che-rest
che-rest startproject my_project
cd my_project
cargo run
```

### cargo-generate Template

Use the standalone [`che-rest-template`](https://github.com/che4web/che-rest-template) repository
to generate a minimal project with [`cargo-generate`](https://github.com/cargo-generate/cargo-generate):

```bash
cargo install cargo-generate
cargo generate --git https://github.com/che4web/che-rest-template.git --name my_api \
  --define che_rest_path=/path/to/che-rest \
  --define che_orm_path=/path/to/che-orm
```

See that repository for its requirements and complete setup instructions.

When developing against local checkouts with the built-in project generator, point the generated
project at them:

```bash
cargo run --bin manage -- startproject my_project \
  --che-rest-path ../che-rest \
  --che-orm-path ../che-orm
cd my_project
cargo run
```

The generated project includes `app.toml`, a server entrypoint, an `apps::installed_apps()` registry,
and a local `manage` binary. Create the first app with:

```bash
cargo run --bin manage -- startapp users
```

Path overrides are useful for this repo layout:

```text
../che-rest
../che-orm
```

Include the built-in auth module in the generated app registry:

```bash
cargo run --bin manage -- startproject my_project --with-auth
```

A complete runnable example with CRUD, authentication, WebSocket signals, internal channels,
TypeScript generation, and Vue admin generation is available in
[`examples/cli_fullstack`](examples/cli_fullstack/README.md).

For a minimal SQLite todo API, see the step-by-step
[`Todo API From Scratch`](docs/TODO_TUTORIAL.md) tutorial.

## Management

Applications define one installed app list and use it for both the API server and management commands:

```rust
pub mod users;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new().add(users::module())
}
```

Common management commands:

```bash
cargo run --bin manage -- startapp tasks --model Task
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
cargo run --bin manage -- migrate status
cargo run --bin manage -- generate-ts
cargo run --bin manage -- generate-admin
```

OpenAPI is served by the running application at `/api/openapi.json` by default, and follows custom
`Server::api_prefix(...)` values.

Server startup:

```rust
let app = Server::new(state)
    .install(apps::installed_apps())
    .build()
    .await?;
```

Server startup does not create model tables. Generate and apply migrations before starting the
server:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
```

The generated `app.toml` also configures the listener and API prefix:

```toml
[server]
host = "127.0.0.1"
port = 3000
api_prefix = "/api"
```

The application entrypoint reads these values, so changing the bind address, port, or API prefix
does not require editing Rust code.

Installed app routes are served under `/api` by default. A viewset registered as `"/users"` is exposed as `/api/users`.
Swagger UI is exposed at `/api/` and the OpenAPI JSON schema at `/api/openapi.json`.

Writable foreign-key fields can be exposed separately from nested read-only output. Use
`#[serializer(foreign_key = User, relation = TaskAssigneeRelation)]` on a scalar `Option<i64>`
field. The generated admin uses an async relation selector, while nested fields such as
`Task.author` remain read-only response data.

## Typed ViewSets

Generated and custom viewsets use associated types for their serializer, queryset, filters, and
permissions:

```rust
use che_rest::{AllowAny, Filter, FilterSetSpec, Model, ViewSet};

#[derive(che_orm::ModelSerializer, serde::Serialize)]
#[serializer(model = Task)]
pub struct TaskSerializer {
    pub id: i64,
    pub name: String,
    pub completed: bool,
}

#[derive(Clone, Copy, Default)]
pub struct TaskFilterSet;

impl FilterSetSpec for TaskFilterSet {
    type Model = Task;

    fn filters(&self) -> &'static [Filter] {
        TASK_FILTERS
    }
}

#[derive(Clone, Copy, Default)]
pub struct TaskViewSet;

#[che_rest::async_trait]
impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type QuerySet = che_orm::DatabaseQuery<Task>;
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;

    fn get_queryset(&self) -> Self::QuerySet {
        che_orm::DatabaseQuery::new(Task::query())
    }

    fn path(&self) -> &'static str {
        "/tasks"
    }
}
```

Register a typed viewset with `viewset_with`; it uses `ViewSet::path()` for the router, model schema,
and API metadata:

```rust
ctx.viewset_with(TaskViewSet);
```

`FilterSetSpec` types must implement `Default`. Built-in permissions include `AllowAny`,
`IsAuthenticated`, and `IsAdminUser`.

The `Model` derive generates compile-time-safe field constants on the model type. Use those constants
when declaring filters:

```rust
static TASK_FILTERS: &[Filter<Task>] = &[
    Filter::exact(Task::COMPLETED),
    Filter::contains(Task::NAME),
    Filter::gte(Task::CREATED_AT),
];
```

The model type is part of `Filter<M>`, so a filter for one model cannot be accidentally used in
another model's `FilterSet`. `Filter::exact_as("assignee", Task::ASSIGNEE_ID)` can be used
when the public query name should differ from the database field name. Query builders also accept
typed fields directly:

```rust
Task::query()
    .filter(Task::COMPLETED.eq(false))
    .all(database)
    .await?;
```

Relations use serializer attributes with generated ORM2 relation markers:

```rust
#[serializer(one = User, relation = TaskAuthorRelation)]
pub author: UserSerializer,
```

Fields populated by the server should be read-only in the serializer and assigned in the viewset's
write preparation hook:

```rust
impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type QuerySet = che_orm::DatabaseQuery<Task>;
    type FilterSet = che_rest::FilterSet<Task>;
    type Permission = IsAuthenticated;

    fn prepare_create(
        &self,
        _state: &che_rest::AppState,
        current: Option<&che_rest::CurrentPrincipal>,
        write: che_rest::ValidatedWrite<Self::Model>,
    ) -> che_rest::AppResult<che_rest::ValidatedWrite<Self::Model>> {
        let user = current
            .map(che_rest::CurrentPrincipal::auth_user)
            .ok_or_else(|| che_rest::AppError::Unauthorized("authentication required".into()))?;
        Ok(write.set(Task::AUTHOR_ID, user.id))
    }
}
```

## Breaking API Changes

ORM writes use generated field descriptors: use `.set(Task::NAME, value)` instead of string field
names. Serializer validation returns `ValidatedWrite`; viewset `prepare_*` hooks can add server-owned
fields before `ValidatedWrite::save(database)` persists the change.

## Auth

`che-rest` includes an optional auth app. Install it to resolve token credentials and add a
request-scoped `CurrentPrincipal` to request extensions. Its `auth_user()` is always the built-in
framework user:

```rust
pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(users::module())
}
```

Create the first superuser:

```bash
cargo run --bin manage -- migrate
```

Then create the first superuser:

```bash
cargo run --bin manage -- createsuperuser \
  --username admin \
  --password secret
```

The command reads `app.toml` by default. Use `--config path/to/app.toml` to select another
configuration file. The auth users table must exist before running the command.

Get a token with the Django REST Framework compatible endpoint:

```bash
curl -X POST http://127.0.0.1:3000/api-token-auth/ \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"secret"}'
```

Use the returned token for API requests:

```text
Authorization: Token <token>
```

Authentication is optional at the middleware level. Protect a typed viewset by selecting
`type Permission = IsAuthenticated`; unauthenticated requests then receive `401`. Use
`AllowAny` for public viewsets and `IsAdminUser` for administrator-only viewsets.

### Application User

Applications can define one domain profile model related to `auth::User`. Configure an async
resolver when the application state is created; the resolved model is stored only on the current
request.

```rust
#[derive(Debug, Clone, che_orm::Model)]
pub struct Profile {
    #[orm(primary_key)]
    pub id: i64,
    #[orm(foreign_key = che_rest::auth::User, unique)]
    pub user_id: i64,
    pub organization_id: i64,
}

pub async fn resolve_profile(
    state: &AppState,
    user: &che_rest::auth::User,
) -> AppResult<Option<Profile>> {
    state
        .database()
        .query::<Profile>()
        .filter(Profile::USER_ID.eq(user.id))
        .first(state.database())
        .await
        .map_err(Into::into)
}

let state = AppState::from_config_file("app.toml")
    .await?
    .with_current_user_resolver(|state, user| Box::pin(resolve_profile(state, user)));
```

Keep the type recovery and the authorization error in one application function:

```rust
pub fn current_profile(current: Option<&CurrentPrincipal>) -> AppResult<&Profile> {
    let current = current.ok_or_else(|| AppError::Unauthorized("authentication required".into()))?;
    current
        .app::<Profile>()
        .ok_or_else(|| AppError::Forbidden("profile required".into()))
}
```

Viewsets and permissions then only call that function:

```rust
let profile = profiles::current_profile(current)?;
```

`Profile` is never part of `che-rest`; it is defined and resolved by the application. If the
resolver returns `None`, the request remains framework-authenticated; only application code that
calls `current_profile` requires a profile.

Generate the typed TypeScript client from installed app metadata:

```bash
cargo run --bin manage -- generate-ts --out frontend/client/src/generated
```

The generator writes `api_client.ts`, `channels.ts`, `models.ts`, `api.ts`, `useModelList.ts`, and
`useModelItem.ts`; it also writes `auth.ts` when the auth app is installed. It does not scan Rust
source files or require the HTTP server to be running.

Generated TypeScript clients expose `setAuthToken(token)` in `api_client.ts`.

## WebSocket Signals

Install the channel module. The WebSocket endpoint is available at `/api/ws/` under the configured
API prefix:

```rust
pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(che_rest::channels::module())
        .add(users::module())
}
```

WebSocket clients subscribe only to declared public signals. CRUD viewsets can opt in to lifecycle
signals by overriding `ViewSet::signal_access`; the default names are `<resource>.created`,
`<resource>.updated`, and `<resource>.deleted`. The default lifecycle payload is
`{ "id": <primary key> }`; clients that need current object data should reload it through the REST
endpoint, so viewset permissions still apply. Nested resource names use dots, for example
`/auth/users` maps to `auth.users.created` when lifecycle signals are enabled. Modules can declare
additional signals in `init`:

```rust
fn init(&self, ctx: &mut che_rest::ModuleContext) {
    ctx.signal("chat.message.created", che_rest::SignalAccess::Authenticated);
}
```

Publish a declared signal with:

```rust
state.signals().publish("chat.message.created", serde_json::json!({ "id": 42 }));
```

Modules can create separate internal application channels through `AppState::app_channels()`, which
are never exposed to WebSocket clients:

```rust
let channels = state.app_channels().clone();
let app = Server::new(state)
    .install(apps::installed_apps())
    .build()
    .await?;

channels.publish("chat.message.created", serde_json::json!({ "id": 42 }));
```

Modules subscribe to internal channels in `subscribe`, before any module `start` hooks publish
initial events. Both hooks run after the database schema is ready:

```rust
pub struct Notifications;

impl che_rest::AppModule for Notifications {
    fn name(&self) -> &'static str { "notifications" }

    fn init(&self, _ctx: &mut che_rest::ModuleContext) {}

    fn subscribe(&self, state: &che_rest::AppState) {
        let mut events = state.app_channels().subscribe("chat.message.created");
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        // Send notifications, update counters, or write audit records.
                        println!("notification event: {event}");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        eprintln!("resync notification channel, dropped {count} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn start(&self, state: &che_rest::AppState) {
        state.app_channels().publish(
            "chat.message.created",
            serde_json::json!({ "kind": "startup" }),
        );
    }
}
```

Application channels use bounded broadcast queues. A slow subscriber receives `Lagged` and should
resynchronize as needed. They are only available inside the server process.

Internal events can have multiple handlers; all of them run in independent application-channel
subscriptions. Public WebSocket signals are independent from internal application channels.

Connections must be authenticated. Non-browser clients can provide `Authorization: Token <token>`
during the WebSocket upgrade. Browser `WebSocket` connections use the existing same-origin session
cookie, because the browser API cannot attach an `Authorization` header.

The TypeScript generator creates `channels.ts` with a cookie-authenticated `ChannelClient`:

```typescript
import { ChannelClient } from "./generated/channels";

const channels = new ChannelClient({
  onMessage: (event) => console.log(event.signal, event.payload),
  onError: (event) => console.error(event.code, event.detail),
});

await channels.connect();
channels.subscribe("orders.created");
```

The client derives `/api/ws/` from `VITE_API_BASE_URL`, converting `http` to `ws` and `https` to
`wss`. Browser cookies are sent automatically for same-origin connections.

Subscribe and unsubscribe using text JSON frames:

```javascript
const socket = new WebSocket("ws://127.0.0.1:3000/api/ws/");

socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ action: "subscribe", signal: "orders.created" }));
});

socket.addEventListener("message", ({ data }) => {
  console.log(JSON.parse(data));
  // { type: "signal", signal: "orders.created", payload: { id: 42 } }
});
```

Clients cannot subscribe to undeclared signals. Private user signals can be published with
`publish_user` and use `user:{id}:<signal>` names; only that user can subscribe after the signal has
been created by server-side publication. Signal names may contain ASCII letters, numbers, `:`, `-`,
`_`, and `.`. The server confirms subscription changes with `subscribed` and `unsubscribed` messages.

This is in-memory pub/sub for one server process: messages are delivered only to currently connected
clients, are not persisted, and do not cross process boundaries. A slow receiver receives a `lagged`
error when messages are dropped; it does not receive a replay.

### Cookie Sessions

Token authentication and cookie sessions can be enabled together. Sessions use an `HttpOnly`
cookie and a readable CSRF cookie. For same-origin applications, add optional configuration:

```toml
[auth.session]
cookie_name = "che_rest_session"
csrf_cookie_name = "csrf_token"
ttl_seconds = 604800
renewal_window_seconds = 86400 # renew active sessions during their final day
absolute_ttl_seconds = 2592000 # require login again after 30 days
secure = false # set true when serving over HTTPS
same_site = "Lax"
```

Login with a session cookie:

```bash
curl -i -c cookies.txt -X POST http://127.0.0.1:3000/api-session-auth/login/ \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"secret"}'
```

Unsafe requests made with a session must include the value from the `csrf_token` cookie in
the `X-CSRF-Token` header. Token-authenticated requests do not require CSRF validation.

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
cargo run --bin manage -- startapp taskapp
```

By default `startapp` creates one model from the app name. If the app name ends with
`app`, that suffix is removed first, so `taskapp` creates model `Task`.
Pass one or more `--model` options to generate specific models:

```bash
cargo run --bin manage -- startapp taskapp --model Task --model Comment
```

Generated tables use `<app>_<model_snake_case>` names, for example `taskapp_task`.
Routes use the model snake-case without adding an `s`, for example `/api/task/`.

Generated files:

```text
src/apps/
  mod.rs
  taskapp/
    mod.rs
    models.rs
    serializers.rs
    filters.rs
    views.rs
```

The generated `serializers.rs`, `filters.rs`, and `views.rs` use the typed API shown above;
the app module registers each generated viewset with `ctx.viewset_with(...)`.

Then add the app to `apps::installed_apps()`:

```rust
InstalledApps::new().add(taskapp::module())
```

Create migrations for all installed apps. The optional positional value names the generated migration:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- makemigrations add_users
```

Generate TypeScript models and API client from installed app metadata:

```bash
cargo run --bin manage -- generate-ts --out src/generated
```

This writes:

```text
  src/generated/
    api_client.ts
    channels.ts
    models.ts
    api.ts
    useModelList.ts
    useModelItem.ts
```

The OpenAPI document describes CRUD routes registered with `ModuleContext::viewset` and
`viewset_with`, including list filters, `limit`, `offset`, and `ordering` query
parameters. A ViewSet can add reusable behavior through `ViewSetExtension` instances in
`ViewSet::configure()`. Extension routes, OpenAPI operations, and generated TypeScript methods
are registered from the same operation descriptors.
The same schema is also served by running applications at `/api/openapi.json`, with
Swagger UI available at `/api/` by default.

Generate a standalone Vue admin project from installed app metadata:

```bash
cargo run --bin manage -- generate-admin --out frontend/admin
cd frontend/admin
npm install
npm run dev
```

This writes a Vite + Vue + Bootstrap project:

```text
frontend/admin/
  package.json
  index.html
  vite.config.ts
  tsconfig.json
  .env.example
  src/
    main.ts
    App.vue
    router.ts
    admin/
      AdminApp.vue
      AdminLogin.vue
      AdminModelList.vue
      AdminModelTable.vue
      AdminModelForm.vue
      adminRoutes.ts
      admin.css
      adminSchema.ts
      components/
        GenericModelTable.vue
        GenericModelForm.vue
      pages/
        UserList.vue
        UserForm.vue
    generated/
      adminSchema.ts
      adminRoutes.ts
    generated/
      api_client.ts
      channels.ts
      models.ts
      api.ts
      useModelList.ts
      useModelItem.ts
```

The first run creates the full project and one editable list/form page pair for each model.
Later runs update generated files and create missing model pages, but leave existing Vue, CSS,
config, package, and model page files untouched so local admin customizations are preserved.
Use `--force` to overwrite static project files from templates:

```bash
cargo run --bin manage -- generate-admin --out frontend/admin --force
```

The generated admin uses session-cookie authentication through `/api-session-auth/login/` and
requires `che_rest::auth::module()` to be installed. The generated Vite dev server proxies `/api`
and `/api-session-auth/` to `http://127.0.0.1:3000` by default. Configure the backend target and
relative API URLs with Vite env variables:

```env
VITE_API_TARGET=http://127.0.0.1:3000
VITE_API_BASE_URL=/api
VITE_AUTH_URL=/api-session-auth/login/
VITE_LOGOUT_URL=/api-session-auth/logout/
```

Static files are copied from the framework's canonical admin templates. Dynamic files are regenerated
from installed app metadata:

Canonical templates are stored in the repository under `src/admin/templates/`. Change those files
when changing the framework default admin UI; `examples/cli_fullstack/frontend/admin/` is generated
output and is not the template source.
`src/admin/generated/adminSchema.ts`, `src/admin/generated/adminRoutes.ts`, the compatibility shim
`src/admin/adminSchema.ts`, and `src/generated/*`.

Per-model pages in `src/admin/pages` are generated as thin wrappers around the generic CRUD
components. Customize those page files for model-specific fields, actions, and layout; they are
not overwritten on later runs unless `--force` is passed.

Defaults:

```text
--name auto
```

`makemigrations` compares installed app schemas with the compiled migration history and writes
reviewable Rust migration modules under `src/migrations/<app>/`. It does not invoke an external
migration tool:

```bash
cargo run --bin manage -- makemigrations
```

Generated migrations are stored under:

```text
src/migrations/<app>/
```

The database URL is read from `[database].url` in `app.toml`. Apply all project migrations with the
built-in forward-only SQLite executor:

```bash
cargo run --bin manage -- migrate
```

Do not create model tables from application startup; production servers only apply already-created
SQL migrations.

### Compiled forward-only migrations

`Management::migrations(...)` supplies the project registry, usually through
`Management::new(apps).migrations(project::migrations::all())`. An empty registry is valid while
generating the first migration. Management has no SQL-file migration fallback.

Preview pending compiled migrations before applying them:

```bash
cargo run --bin manage -- migrate --plan --config app.toml
```

The plan lists migrations in dependency order, their dependencies and operations,
and warnings for table/column deletion, column changes and manual SQL. It validates
the registered graph, applied checksums, dependency history and historical state.
SQLite is opened read-only; a missing database is treated as empty and is not
created. No migrations or history records are written. The command requires a
file database path (plain or `sqlite://`) and cannot be combined with a `migrate`
subcommand. It previews compiled migrations; SQL execution and live-schema
compatibility are checked when applying them. Rebuild after changing migration
sources, and inspect `RunSql` source before applying manual SQL.

Inspect the SQLite SQL for one registered migration without opening a database:

```bash
cargo run --bin manage -- sqlmigrate tasks 0002_add_title
```

`sqlmigrate` rebuilds only the selected migration's dependency state in memory,
then prints its SQLite statements. Declarative table changes therefore show the
same create-copy-drop-rename SQL used by the executor. `RunSql` is printed as
authored. The command requires compiled migrations and does not validate the
live database schema or execute any statement.

List registered compiled migrations and their database status with:

```bash
cargo run --bin manage -- showmigrations
cargo run --bin manage -- showmigrations tasks
```

`[X]` marks a migration recorded in `che_migration_history`; `[ ]` marks a
pending migration. Dependencies are listed beneath each migration. The command
opens a file database only in read-only mode, treats a missing database as an
empty history and never creates the history table.

Apply a selected application's forward migration target with:

```bash
cargo run --bin manage -- migrate tasks
cargo run --bin manage -- migrate tasks latest
cargo run --bin manage -- migrate tasks 0002_add_title
```

The first two forms select the application's single latest migration. The
executor applies only that target and its missing dependencies, each in a
separate atomic SQLite transaction; unrelated pending migrations are left untouched. A target
already passed by a later migration in the same application is rejected because
rollback is not supported. Unknown applications and migration names are also
rejected before the database is opened.

When two reviewed branches create compatible heads for one application, create
an empty merge migration with:

```bash
cargo run --bin manage -- makemigrations tasks --merge --name merge
```

The command accepts exactly two heads. It rebuilds their common historical
state, replays the two branches in both orders, and creates a migration that
depends on both heads only when the resulting declarative states agree. It
rejects `RunSql`, conflicting operations, and three or more heads; resolve
those with an explicit reviewed migration. Use `--dry-run` to inspect generated
Rust before the file and registry are changed.

For example, generate an application's migration with:

```bash
cargo run --bin manage -- makemigrations tasks --name initial --dir src/apps/tasks/migrations
```

The writer creates `m0001_initial.rs` and a managed section of `mod.rs` containing
module declarations and `pub fn all() -> Vec<che_orm::Migration>`. Declare the
`migrations` module in the owning application and pass its collector to management:

```rust,ignore
Management::new(installed_apps())
    .migrations(tasks::migrations::all())
    .run()
    .await?;
```

For multiple applications, concatenate their `all()` results. Rebuild the
management binary after generation. Existing handwritten `mod.rs` files must
have `// che-orm: migrations begin` and `// che-orm: migrations end` markers;
the writer owns the declarations and collector inside those markers. Keep
handwritten code outside them, and avoid another `all()` in the same module.

`--empty` produces zero operations even when models differ from history.
Normal generation rejects conflicting branches and cross-application references
whose target table or field is not yet present in compiled migration history.
Generate and register the target application's migration first. Operations are
ordered and replayed in memory before writing; unsupported dependency sequences
require an explicit migration.

Use `makemigrations <app> --check` in CI: it exits with an error when the
installed models have changes without a compiled migration. Use `--dry-run` to
print the prospective Rust source without changing files. Before either command
creates a migration, it compares `m000N_*.rs` files with the compiled registry;
if a prior generation has not been rebuilt yet, it stops and asks for a rebuild.

For a new project whose applications contain cyclic cross-application foreign
keys, omit the app name and provide a directory that will contain one directory
per application:

```bash
cargo run --bin manage -- makemigrations --dir src/apps
```

This empty-history batch mode writes each app's `0001_initial` without the
cross-application FKs, then writes `0002_relationships` migrations that depend
on both initial migrations. Rebuild after generation and combine each app's
`migrations::all()` when constructing `Management`.

New migration files are created exclusively. Registry updates use a directory
lock and atomic replacement. A competing writer fails without overwriting files.
An interrupted process may leave `.che-migrations.lock` or `.che-mod.rs.tmp`;
inspect the directory and confirm no writer is active before removing them.
