# CLI Fullstack Example

This example shows a project created with the `che-rest` CLI. Run the commands from the repository
root unless noted otherwise.

## Create The Project

The repository layout expected by the default generator is:

```text
../che-rest
../che-orm2
```

Create a fresh project with built-in authentication. The temporary output path keeps the checked-in
example untouched:

```bash
cargo run --bin manage -- startproject cli_fullstack \
  --out /tmp \
  --with-auth \
  --che-rest-path "$PWD" \
  --che-orm2-path "$PWD/../che-orm2"

cd /tmp/cli_fullstack
```

Create the generated application modules:

```bash
cargo run --bin manage -- startapp tasks --model Task
cargo run --bin manage -- startapp notifications --model Notification
```

The checked-in example keeps the generated `Task` CRUD module and adds a command handler plus an
internal notification subscriber. `startapp` registers generated modules in `src/apps/mod.rs`.

## Migrations And Auth

Generate and apply migrations. `makemigrations` requires Atlas; `migrate` uses the built-in runner:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
```

Create an admin user:

```bash
cargo run --bin manage -- createsuperuser \
  --username admin \
  --password secret
```

The command reads `app.toml` by default and creates an active superuser. Pass `--config` to use a
different configuration file; the auth migrations must be applied first.

Start the API:

```bash
cargo run
```

The API is available at `http://127.0.0.1:3001`. CRUD routes are under `/api/tasks/`, Swagger UI is
at `/api/`, and OpenAPI JSON is at `/api/openapi.json`. Swagger uses `/api` as the CRUD server base;
the authentication paths remain at the root, such as `/api-session-auth/login/`.

Use the session login operation in Swagger first. The browser keeps the session and CSRF cookies,
and the Swagger request interceptor sends `X-CSRF-Token` automatically for write operations. Token
authentication can be used through the `Authorize` dialog with the value `Token <key>`.

## HTTP CRUD

Log in first and keep the returned cookies in `cookies.txt`:

```bash
curl -i -c cookies.txt -X POST \
  http://127.0.0.1:3001/api-session-auth/login/ \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"secret"}'
```

Create a task through the generated REST endpoint. Tasks require an authenticated user, and the
server assigns the current session user as the author. The `author_id` field cannot be supplied by
the client. The optional `assignee_id` field is a writable user relation; the generated admin uses
`/api/auth/users/` to search available users. Session writes also require the CSRF header:

```bash
CSRF_TOKEN=$(awk '$6 == "csrf_token" { print $7 }' cookies.txt)
curl -X POST http://127.0.0.1:3001/api/tasks/ \
  -b cookies.txt \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Read the documentation","status":"draft"}'
```

To assign a user from HTTP, include their user id:

```bash
curl -X POST http://127.0.0.1:3001/api/tasks/ \
  -b cookies.txt \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Pair on the task","status":"in_progress","assignee_id":2}'
```

Every task creation path, including REST and admin, emits the public WebSocket `tasks.created`
signal with a `{ "id": ... }` payload.

List tasks in ascending or descending order with the `ordering` query key. The example exposes
`id`, `name`, `created_at`, and `updated_at`:

```bash
curl 'http://127.0.0.1:3001/api/tasks/?ordering=name'
curl 'http://127.0.0.1:3001/api/tasks/?ordering=-created_at'
```

## WebSocket Signals

The project installs `che_rest::channels::module()`. Browser WebSockets use the authenticated
same-origin session cookie from the login above.

Subscribe from a browser or WebSocket client:

```json
{
  "action": "subscribe",
  "signal": "tasks.created"
}
```

Creating a task through REST or admin emits a `tasks.created` signal to subscribed clients. Clients
that need the current object representation should reload it through REST. The notifications module
demonstrates the separate internal `AppChannels` API in `AppModule::subscribe`; internal channels are
not exposed as WebSocket signals.

Run the end-to-end smoke test:

```bash
cargo test --manifest-path examples/cli_fullstack/Cargo.toml --test smoke
```

## Generated Clients

Generate the TypeScript client:

```bash
cargo run --bin manage -- generate-ts --out frontend/client/src/generated
```

The command reads installed app and viewset metadata, including nested `Task.author` fields and the
task list filter/order keys.

Use the generated WebSocket client:

```typescript
import { ChannelClient } from "./generated/channels";

const channels = new ChannelClient({
  onMessage: (event) => console.log(event.signal, event.payload),
  onError: (event) => console.error(event.code, event.detail),
});

await channels.connect();
channels.subscribe("tasks.created");
```

Generate the standalone Vue admin project:

```bash
cargo run --bin manage -- generate-admin --out frontend/admin
cd frontend/admin
npm install
npm run dev
```

The generated admin uses session-cookie authentication for HTTP API requests and the CSRF cookie
`csrf_token`. Same-origin browser WebSockets use the same session cookie. For this example, set
`VITE_API_TARGET=http://127.0.0.1:3001` before starting the generated admin.

## Vue Task Desk

`frontend/client` is a small hand-written Vue task board. It uses the generated REST client under
`frontend/client/src/generated`, session-cookie authentication, and the CSRF cookie when creating
tasks:

```bash
cargo run --bin manage -- generate-ts --out frontend/client/src/generated
cd frontend/client
npm install
npm run dev
```

Run the Rust server separately with `cargo run`. The Vite dev server proxies API and session-auth
requests to `http://127.0.0.1:3001` by default; copy `.env.example` to `.env` to override it.

## Internal Channels

Modules use `AppState::app_channels()` for server-only events:

```rust
fn subscribe(&self, state: &che_rest::AppState) {
    let mut events = state.app_channels().subscribe("tasks.created");
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => println!("tasks.created: {event}"),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    eprintln!("resync required, dropped {count} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
```

Application channels are in-memory, bounded, and process-local. They are separate from `che-orm`
model signals and from public WebSocket channels.
