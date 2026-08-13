# CLI Fullstack Example

This example shows a project created with the `che-rest` CLI. Run the commands from the repository
root unless noted otherwise.

## Create The Project

The repository layout expected by the default generator is:

```text
../che-rest
../che-orm/crates/che-orm
```

Create a fresh project with built-in authentication. The temporary output path keeps the checked-in
example untouched:

```bash
cargo run --bin che-rest -- startproject cli_fullstack \
  --out /tmp \
  --with-auth \
  --che-rest-path "$PWD" \
  --che-orm-path "$PWD/../che-orm/crates/che-orm"

cd /tmp/cli_fullstack
```

Create the generated application modules:

```bash
cargo run --bin manage -- startapp tasks --model Task
cargo run --bin manage -- startapp notifications --model Notification
```

The checked-in example keeps the generated `Task` CRUD module and adds a command handler plus an
internal notification subscriber. In a newly generated project, add both modules to
`src/apps/mod.rs` as shown in this example.

## Migrations And Auth

Generate and apply migrations:

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

Start the API:

```bash
cargo run
```

The API is available at `http://127.0.0.1:3001`. CRUD routes are under `/api/tasks/`, Swagger UI is
at `/api/`, and OpenAPI JSON is at `/api/openapi.json`.

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
the client. Session writes also require the CSRF header:

```bash
CSRF_TOKEN=$(awk '$6 == "csrf_token" { print $7 }' cookies.txt)
curl -X POST http://127.0.0.1:3001/api/tasks/ \
  -b cookies.txt \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Read the documentation"}'
```

Every task creation path, including REST, admin, and the WebSocket command below, emits the
internal `tasks.created` event.

## WebSocket Command

The tasks module registers the `tasks.create` command. Browser WebSockets use the authenticated
same-origin session cookie from the login above.

Then send this command from a browser or WebSocket client:

```json
{
  "action": "publish",
  "event": "tasks.create",
  "payload": { "name": "Write a channel example" }
}
```

The command creates a task. The tasks module bridges the ORM `PostSave` signal to the internal
`tasks.created` application channel, and the notifications module subscribes to that channel in
`AppModule::subscribe`; it is never visible as a WebSocket channel.

Run the end-to-end smoke test:

```bash
cargo test --manifest-path examples/cli_fullstack/Cargo.toml --test smoke
```

## Generated Clients

Generate the TypeScript client:

```bash
cargo run --bin manage -- generate-ts --out frontend/client/src/generated
```

Use the generated WebSocket client:

```typescript
import { ChannelClient } from "./generated/channels";

const channels = new ChannelClient({
  onMessage: (event) => console.log(event.channel, event.payload),
  onError: (event) => console.error(event.code, event.detail),
});

await channels.connect();
channels.publish("tasks.create", { name: "From TypeScript" });
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
`src/generated`, session-cookie authentication, and the CSRF cookie when creating tasks:

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
