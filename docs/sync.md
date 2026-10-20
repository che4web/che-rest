# Todo synchronization through a ViewSet

Run the complete local HTTP demo from the `che-rest` directory:

```bash
cargo run --example todo_sync
```

It starts an authenticated HTTP server on an ephemeral loopback port, opens two
independent SQLite clients, creates a Project and Todo offline, automatically
sends the required Project through its own endpoint, merges title/done edits,
synchronizes a Project edit through its separate feed, and propagates deletion.
It shuts down when finished. Databases are
in-memory. The token is generated and stored hashed for this local demo.

The model and server configuration live in
`examples/todo_sync/support.rs`; the HTTP transport and client workflow live in
`examples/todo_sync.rs`.

## Connect a ViewSet

A synchronized model implements `SyncModel` and uses UUID keys. Map every
business column once; keys, managed timestamps and image keys are server-owned.
Models also declare `revision: i64`, `updated_at: OffsetDateTime`,
`deleted_at: Option<OffsetDateTime>` and `sync_scope: String`. These service
columns are registered automatically as server-owned; serializers expose them
read-only if needed. Server querysets filter `Model::DELETED_AT.is_null()`.

```rust,ignore
impl SyncModel for Todo {
    const SYNC_NAME: &'static str = "todo.todo";
    fn sync_fields() -> Vec<Field> {
        vec![
            Field::server_owned("id", "id"),
            Field::writable("title", "title"),
            Field::writable("done", "done"),
            Field::writable("project", "project_id"),
        ]
    }
}
```

Implement `SyncViewSet` on an ordinary `ViewSet` and install the extension:

```rust,ignore
impl SyncViewSet for TodoViewSet {
    fn sync_registry(&self) -> AppResult<Registry> {
        Ok(Registry::new().register::<Project>()?.register::<Todo>()?)
    }
    fn sync_scope(&self, _: &AppState, _: &CurrentPrincipal) -> AppResult<String> {
        Ok("todo-board".into())
    }
    fn sync_policy(&self) -> Arc<dyn AccessPolicy> {
        Arc::new(AllowAll)
    }
}

// Inside impl ViewSet for TodoViewSet:
fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
    config.extend(SyncEndpoints::new(self.clone()))
}

// Inside AppModule::init:
context.viewset_with(TodoViewSet);
```

Install `che_rest::auth::module()` to authenticate requests. The demo shares one
board among all authenticated users, so `AllowAll` is intentional. For a private
workspace derive its scope from the trusted `CurrentPrincipal` application
profile and implement `AccessPolicy`. A scope must be uniformly readable.
Ordinary CRUD querysets and permissions must enforce matching read access;
queryset filters are not applied to sync snapshots or pull pages.

Each ViewSet is automatically bound to `Self::Model`. Todo and Project have
separate cursors, outgoing operations and conflict blocking. Their registry
describes relationships and must include the related models needed for cascade
capture; it does not combine their synchronization queues. Parents are registered
before children. The example installs separate Todo and Project application
modules; shared `_sync_*` server tables are included once in migration state.

## Routes

| Method | Path | Protocol |
| --- | --- | --- |
| POST | `/api/todos/` | Serializer create JSON |
| PATCH | `/api/todos/{id}/` | Serializer patch JSON |
| DELETE | `/api/todos/{id}/` | Logical deletion |
| POST | `/api/todos/sync/pull/` | `PullRequest` -> `PullResult` |
| GET | `/api/todos/sync/snapshot/` | `Snapshot` |

Project has analogous CRUD routes and `/api/projects/sync/{pull,snapshot}/`.
Pull/snapshot contain the selected model and its FK parent models, automatically
loading remote dependencies. Writes contain one record; required
FK parents use separate POSTs to their own resources. A null pull cursor
requests `Full(Snapshot)` for both initial loading and journal recovery. The
cursor's `model` identifies its feed and cannot be reused with another endpoint.
Parent deletions retain their cascade/SET NULL effects in pull groups. A reset
snapshot with parent tombstones includes related current rows so these effects
can be applied without false conflicts. Independent model cycles can run
concurrently; only overlapping entities and required dependencies are frozen by
an unresolved outgoing operation.

Tagged integer values, revisions, base revisions and cursor/group sequences are
decimal strings in JSON and generated TypeScript. This preserves the full i64
range in JavaScript. Use BigInt for arithmetic and stringify it before sending.
Legacy integer JSON tokens remain readable. Regenerate existing clients.

Send `Authorization: Token <token>` or use the framework's session/CSRF flow.
The principal comes from authentication. The scope header must match the scope
derived by the ViewSet. Ordinary CRUD body limits apply to writes;
`SyncEndpoints::body_limit` configures the remaining pull/snapshot routes.
OpenAPI and generated TypeScript expose `todoApi.sync.pull` and `.snapshot`;
the separate push route has been removed.

Conditional REST conflicts return 412. Rejected writes return 400, missing credentials return 401,
scope authorization denial returns 403, and oversized bodies return 413.
Operational failures leave the client outbox available for retry.

### Conditional, idempotent REST writes

The write body uses the same serializer JSON as ordinary CRUD. Send these headers
together for synchronization:

```http
PATCH /api/todos/<uuid>/
Authorization: Token <token>
X-Sync-Protocol: 1
X-Sync-Replica: <client UUID>
X-Sync-Scope: todo-board
X-Sync-Version: 1
If-Match: "3"
Idempotency-Key: <operation UUID>
Content-Type: application/json

{"done":true}
```

For POST additionally send `X-Sync-Entity: <offline UUID>` and `If-Match: "0"`.
The UUID is assigned to the insert after serializer validation and preparation;
it does not make the primary-key serializer field writable. PATCH/PUT/DELETE
use the URL's UUID. Revisions are compared in the write transaction, not just
during request preparation. Receipts are scoped by authenticated principal,
scope, replica and operation, and reject reuse with different method/body/target.

Sync-mode responses have `Content-Type: application/vnd.che-sync+json` and contain
the canonical `PushResult` envelope, including protected fields and cascade
tombstones for shadow reconciliation. Accepted POST returns 201, PATCH/PUT/DELETE
return 200 with an ETag revision; conflicts return 412. Ordinary CRUD without
sync headers retains serializer responses and DELETE's 204 status. Validation
errors use the ordinary REST error format. A transport decodes the sync envelope
when its content type is present. Non-sync HTTP errors (including 401, 408, 429
and proxy errors) and network failures preserve the immutable outbox operation.
Only a sync-envelope rejection is an authoritative business rejection. The example
transport reports `Retry-After` in its error; the caller schedules retries and
renews expired credentials without changing the operation ID or body.

`RestSyncRequest::new::<Model, Serializer>(request)` converts an immutable native
operation to method, headers and ordinary serializer JSON. The HTTP adapter
implements `SyncTransport::max_mutations` as 1, routes POST to the collection and
PATCH/DELETE to the returned entity UUID. Sync integer fields use decimal strings
in request bodies, preserving i64 values in JavaScript. The server converts these
to exact JSON integers before invoking the ordinary serializer; historical numeric
tokens are also accepted. Revision headers and tagged pull integers remain decimal strings.
Settle old batched outbox operations through their original transport before
switching a database to single-record REST delivery.

## CRUD and migrations

The extension also installs a mutation backend. Ordinary POST, PUT, PATCH and
DELETE go through `SyncServer`, preserving serializer validation, `prepare_*`,
permissions and lifecycle hooks. Data, revisions, journal and receipts commit
atomically. A concurrent CRUD revision conflict returns 409. Prepared protected
fields are trusted server values. Managed timestamps remain server-generated.
Updates must retain a single UUID-primary-key predicate; arbitrary extra SQL
predicates are rejected instead of silently discarded.

Installation checks serializer metadata: a sync server-owned column must be
read-only in the CRUD serializer. Every writable sync field must have exactly one
writable serializer mapping, including FK and `db_column` aliases; installation
rejects missing or ambiguous mappings. Trusted values added by prepare_create or
prepare_update remain writable by the server.

Ordinary and conditional REST writes dispatch after_commit/after_delete hooks and declared lifecycle
signals to installed SyncViewSets for the affected models, including cascades.
Delete payloads are prepared before mutation. The server rechecks all prepared
rows under its scope lock, so concurrent changes cannot invalidate cleanup
payloads. Each hook and signal has its own durable `_sync_effects` entry written
in the mutation transaction. Failures leave that entry queued and do not skip
other handlers or turn a committed write into an error. Later writes, including
receipt retries, retry queued effects; completed effects are not dispatched again.
Handlers should be idempotent: a process crash between an external effect and
its acknowledgement can cause at-least-once delivery.

Direct ORM writes to synchronized models bypass capture. Application background
jobs must use `SyncServer::save`, `write` or `delete`. The declarative direct-write
image extensions cannot currently share a sync ViewSet; registration rejects
that combination until a journal-aware media adapter is available.

Application modules expose their registries through `AppModule::sync_registry`.
This includes shared sync tables once in the migration planner's
desired state. Add the registry's `migration_operations(SchemaRole::Server)` to
the application's compiled initial migration and freeze the resulting operations.
Clients similarly migrate `SchemaRole::Client` tables. Opening a server or
client does not create model tables. The demo generates its disposable initial
operations at runtime; production histories must preserve their identities and
checksums.

## Offline client

```rust,ignore
let client = SyncClient::open(database, registry, "todo-board").await?;
let project = Project { id: Uuid::now_v7(), title: "Errands".into(), ..Default::default() };
client.save(&project).await?;
client.save(&Todo {
    id: Uuid::now_v7(),
    title: "Buy milk".into(),
    done: false,
    project_id: Some(project.id),
    ..Default::default()
}).await?;

let engine = SyncEngine::for_model::<Todo>(client.clone(), http_transport)?;
loop {
    let report = engine.sync_once().await?;
    if report.blocked || !report.has_more { break; }
}
```

Save the Project through the same client before saving a Todo that references it.
The Todo engine automatically sends a required new Project as a separate step;
no separate Project synchronization call is needed for that create. Existing
Project edits use `SyncEngine::for_model::<Project>` with the Project transport.
All engines for one database share clones of one SyncClient. Required dependency
conflicts block the dependent group; unrelated model conflicts do not.

The example's `HttpTransport` implements `SyncTransport` with `reqwest` and
reuses the protocol types directly, with `push_model` routing dependency steps
to their endpoints. Cycles pull and merge before preparing new patches; unknown
write outcomes are retried before pulling. Local cross-model transactions are
delivered sequentially; server cascades and their journal entries remain atomic.
The caller schedules retries and supplies timeouts/backoff. Resolve durable conflicts through `client.conflicts()` and
`client.resolve(...)`. See the ORM repository's `docs/sync.md` for merge,
ownership and recovery semantics.
