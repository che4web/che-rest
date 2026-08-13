# che-rest

Base REST layer for `che-orm` applications.

## Start a Project

Create a new runnable `che-rest` application:

```bash
cargo run --bin che-rest -- startproject my_project
cd my_project
cargo run
```

The generated project includes `app.toml`, a server entrypoint, an empty `apps::installed_apps()`
registry, and a local `manage` binary. Create the first app with:

```bash
cargo run --bin manage -- startapp users
```

By default the generator assumes this repo layout:

```text
../che-rest
../che-orm/crates/che-orm
```

Override paths when needed:

```bash
cargo run --bin che-rest -- startproject my_project \
  --che-rest-path ../che-rest \
  --che-orm-path ../che-orm/crates/che-orm
```

Include the built-in auth module in the generated app registry:

```bash
cargo run --bin che-rest -- startproject my_project --with-auth
```

A complete runnable example with CRUD, authentication, WebSocket commands, internal channels,
TypeScript generation, and Vue admin generation is available in
[`examples/cli_fullstack`](examples/cli_fullstack/README.md).

## Management

Applications define one installed app list and use it for both the API server and management commands:

```rust
pub mod users;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new().add(users::module())
}
```

For machine-readable project metadata, inspect the installed apps from the project directory:

```bash
cargo run --bin manage -- inspect --format json
```

The stable `che-rest.inspect.v1` document includes models, API endpoints, filters, registered
commands, migration files, and session configuration.

Management commands that generate or apply changes support machine-readable output:

```bash
cargo run --bin manage -- makemigrations --format json
cargo run --bin manage -- migrate --format json
cargo run --bin manage -- generate-ts --format json
cargo run --bin manage -- generate-admin --format json
```

The default `text` format remains intended for interactive use.

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

Installed app routes are served under `/api` by default. A viewset registered as `"/users"` is exposed as `/api/users`.
Swagger UI is exposed at `/api/` and the OpenAPI JSON schema at `/api/openapi.json`.
Customize the displayed API metadata during server setup:

```rust
let app = Server::new(state)
    .install(apps::installed_apps())
    .openapi_title("My API")
    .openapi_version("0.1.0")
    .build()
    .await?;
```

## Typed ViewSets

Generated and custom viewsets use associated types for their serializer, filters, and permissions:

```rust
use che_rest::{AllowAny, Field, Filter, FilterSetSpec, Serializer, ViewSet};

#[derive(Clone, Copy, Default)]
pub struct TaskSerializer;

impl Serializer for TaskSerializer {
    type Model = Task;

    fn fields(&self) -> &'static [Field] {
        TASK_FIELDS
    }
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
    type FilterSet = TaskFilterSet;
    type Permission = AllowAny;
}
```

Register a typed viewset with `viewset_with`; this also registers its model schema and API metadata:

```rust
ctx.viewset_with("/tasks", TaskViewSet);
```

`Serializer` and `FilterSetSpec` types must implement `Default`. Their default values are used by the
viewset, so no `serializer()` or `filterset()` method is required. Built-in permissions include
`AllowAny`, `IsAuthenticated`, and `IsAdminUser`.

The `Model` derive generates a `<Model>Fields` type with compile-time-safe database field constants.
Use those constants when declaring filters:

```rust
static TASK_FILTERS: &[Filter<Task>] = &[
    Filter::exact(TaskFields::COMPLETED),
    Filter::contains(TaskFields::TITLE),
    Filter::gte(TaskFields::CREATED_AT),
];
```

The model type is part of `Filter<M>`, so a filter for one model cannot be accidentally used in
another model's `FilterSet`. `Filter::exact_as("assignee", TaskFields::EXECUTOR_ID)` can be used
when the public query name should differ from the database field name. Query builders also accept
typed fields directly:

```rust
Task::objects(db)
    .query()
    .eq(TaskFields::COMPLETED, false)
    .all()
    .await?;
```

Relations use the related serializer type directly:

```rust
Field::related::<EmployeeSerializer>("author", "author_id")
```

Fields populated by the server can be marked as system fields. They are rejected from client input,
omitted from responses, and validated only through `system_create_values`:

```rust
static TASK_FIELDS: &[Field] = &[
    Field::new("author_id").system(),
    Field::new("title"),
];

#[che_rest::async_trait]
impl ViewSet for TaskViewSet {
    type Model = Task;
    type Serializer = TaskSerializer;
    type FilterSet = TaskFilterSet;
    type Permission = IsAuthenticated;

    async fn system_create_values(
        &self,
        state: &AppState,
        extensions: &axum::http::Extensions,
    ) -> che_rest::AppResult<serde_json::Map<String, serde_json::Value>> {
        let employee = current_employee(state, extensions).await?;
        Ok([(String::from("author_id"), serde_json::json!(employee.id))]
            .into_iter()
            .collect())
    }
}
```

Disable the runtime Swagger UI if needed:

```rust
let app = Server::new(state)
    .install(apps::installed_apps())
    .swagger_ui(false)
    .build()
    .await?;
```

## Auth

`che-rest` includes an optional auth app. Install it to resolve token credentials and add
`CurrentUser` to request extensions:

```rust
pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(users::module())
}
```

Create the first superuser:

```bash
cargo run --bin manage -- migrate auth
```

Then create the first superuser:

```bash
cargo run --bin manage -- createsuperuser \
  --username admin \
  --password secret
```

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

Generated TypeScript clients expose `setAuthToken(token)` in `api_client.ts`.

## WebSocket Channels

Install the channel module together with auth. The WebSocket endpoint is available at `/api/ws/`:

```rust
pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(che_rest::channels::module())
        .add(users::module())
}
```

WebSocket channels are public transport channels. Modules can create separate internal application
channels through `AppState::app_channels()`, which are never exposed to WebSocket clients:

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

WebSocket clients still send commands to registered command handlers. The server always associates a
client command with the authenticated user; the client cannot choose another user's channel:

```typescript
await channels.connect();
channels.publish("chat.message.send", { text: "Hello" });
```

Each command has one handler. Internal events can have multiple handlers; all of them run in
independent application-channel subscriptions. If no command handler is registered, the client
receives a `command_error` response.

Connections must be authenticated. Non-browser clients can provide `Authorization: Token <token>`
during the WebSocket upgrade. Browser `WebSocket` connections use the existing same-origin session
cookie, because the browser API cannot attach an `Authorization` header.

The TypeScript generator creates `channels.ts` with a cookie-authenticated `ChannelClient`:

```typescript
import { ChannelClient } from "./generated/channels";

const channels = new ChannelClient({
  onMessage: (event) => console.log(event.channel, event.payload),
  onError: (event) => console.error(event.code, event.detail),
});

await channels.connect();
channels.subscribe("orders:42");
```

The client derives `/api/ws/` from `VITE_API_BASE_URL`, converting `http` to `ws` and `https` to
`wss`. Browser cookies are sent automatically for same-origin connections.

Subscribe and unsubscribe using text JSON frames:

```javascript
const socket = new WebSocket("ws://127.0.0.1:3000/api/ws/");

socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ action: "subscribe", channel: "orders:42" }));
});

socket.addEventListener("message", ({ data }) => {
  console.log(JSON.parse(data));
  // { type: "message", channel: "orders:42", payload: { status: "paid" } }
});
```

Each connection is automatically subscribed to its private `user:{id}` channel. Client publication
is routed to the command handler with the authenticated `CurrentUser`; event handlers may respond
through `publish_user`. Clients cannot subscribe directly to `user:` channels. Other channel names
may contain ASCII letters, numbers, `:`, `-`, `_`, and `.`. The server confirms subscription changes
with `subscribed` and `unsubscribed` messages.

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

Session data can be typed by a downstream application:

```rust
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct AppSession {
    active_workspace_id: Option<i64>,
}

impl che_rest::auth::SessionData for AppSession {}

InstalledApps::new()
    .add(che_rest::auth::module_with_session::<AppSession>());
```

Handlers can extract and persist the typed data:

```rust
async fn select_workspace(
    mut session: che_rest::auth::Session<AppSession>,
) -> che_rest::AppResult<()> {
    session.data.active_workspace_id = Some(42);
    session.save().await
}
```

Session data is JSON stored server-side. Use `#[serde(default)]` when adding fields so existing
sessions remain readable. Sessions are protected by optimistic revision locking.

Admin-only routers can use `che_rest::auth::admin_required_middleware`. It allows users with `is_admin` or `is_superuser`.

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

Create migrations for all installed apps, or scope generation to one app:

```bash
cargo run --bin manage -- makemigrations
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
    channels.ts
    models.ts
    api.ts
    useModelList.ts
    useModelItem.ts
```

Generate an OpenAPI 3.0 JSON schema from installed viewsets, serializers, and filters:

```bash
cargo run --bin manage -- generate-openapi --out openapi.json
```

Configure the generated API metadata:

```bash
cargo run --bin manage -- generate-openapi \
  --out openapi.json \
  --title "My API" \
  --version "0.1.0" \
  --api-prefix /api
```

The generator describes CRUD routes registered with `ModuleContext::viewset` and
`viewset_with`, including list filters, `limit`, `offset`, and `ordering` query
parameters. Custom `extra_routes()` are not included automatically.
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

Override static admin templates by mirroring output paths in a templates directory:

```bash
cargo run --bin manage -- generate-admin \
  --out frontend/admin \
  --templates-dir admin_templates
```

Example overrides:

```text
admin_templates/
  src/admin/components/GenericModelTable.vue
  src/admin/admin.css
```

Dynamic files are always regenerated from installed app metadata and are not read from `--templates-dir`:
`src/admin/generated/adminSchema.ts`, `src/admin/generated/adminRoutes.ts`, the compatibility shim
`src/admin/adminSchema.ts`, and `src/generated/*`.

Per-model pages in `src/admin/pages` are generated as thin wrappers around the generic CRUD
components. Customize those page files for model-specific fields, actions, and layout; they are
not overwritten on later runs unless `--force` is passed.

Defaults:

```text
--name auto
```

`makemigrations` without an app argument processes all installed apps. Pass an app name to limit
generation to one app:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- makemigrations users
```

Generated migrations are stored under:

```text
src/apps/users/migrations/
```

Migration versions are local to each app, so every app can begin at `0001`.
`migrate` applies them with the installed app name as a stable namespace; do
not rename an installed app after its migrations have been deployed.

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
