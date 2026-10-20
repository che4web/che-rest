# Per-model REST synchronization (protocol 2)

`ModelSyncEndpoints` uses ordinary resource POST/PATCH/PUT/DELETE routes for
writes and `GET /api/<resource>/sync/changes/?after=0&limit=100` for downloads.
Each model has its own append-only journal and cursor. Authorization grants
access to the entire synchronized model; this protocol has no workspace or
client-supplied scope. Use it only when that access matches the application.

Shared objects with per-object permissions require an additional access-delivery
contract: grants send current state and revocations remove local visibility.
That extension is **not implemented** by these endpoints. See the
[per-object synchronization plan](OBJECT_SYNC_PLAN.md); a queryset filter or
per-owner scope alone does not implement it.

Run the two-device SQLite/HTTP example:

```bash
cargo run --features sync-http --example model_sync
```

The example coalesces ten offline edits into one POST, merges independent
fields edited on two devices, and downloads deletion. Its model, serializer,
compiled demo migrations and authenticated ViewSet are in
[`examples/model_sync/support.rs`](../examples/model_sync/support.rs).
The integration test in `tests/model_sync_http.rs` additionally simulates a lost
reply after commit and reopens the client before retrying the immutable request.
All example databases are in memory; deployed clients need persistent SQLite.

## Model and server

Use a UUID primary key and stable model identity:

```rust,ignore
#[derive(Debug, Model)]
#[orm(table = "notes", sync, sync_name = "notes.note")]
struct Note {
    #[orm(primary_key)]
    id: Uuid,
    title: String,
}
```

Register `SchemaSet::server_model::<Note>()` on the server and
`SchemaSet::client_model::<Note>()` on the client. Derive registries with
`Registry::from_schema`. Register FK parents before children. The role-specific
schema adds service columns and infrastructure tables through compiled migration
state, including `<table>_sync_log` and `<table>_sync_log_state` on the server.
Do not create production tables at startup or rewrite applied migrations.

Implement `SyncViewSet::sync_registry` and install the extension in the ordinary
ViewSet configuration:

```rust,ignore
fn configure(&self, config: &mut ViewSetConfig<Self>) -> AppResult<()> {
    config.extend(ModelSyncEndpoints::new(self.clone()))
}
```

Use `ModuleContext::viewset_with` to register routes and metadata. Install auth
and require authentication on the ViewSet. Its registry must include related
models needed for cascade/SET NULL capture. Ordinary CRUD and synchronization
must enforce the same model-wide read access. Journal downloads do not apply
ViewSet queryset filters.

Serializer validation and trusted preparation run on ordinary REST writes.
Committed server values, including preparation and defaults, determine journal
diffs. A no-op produces no revision or journal operation. Data, revisions,
receipts and journal changes commit atomically. Configured SyncServer database
handles also capture ORM writes; raw SQL and unconfigured handles bypass capture.

## Native/Tauri client

Enable the `che-rest` feature `sync-http` for `sync_http::RestModelTransport`:

```rust,ignore
let client = ModelLogClient::open(database, client_registry).await?;
let transport = RestModelTransport::new(
    "https://host/api",
    authenticated_reqwest_client,
    vec![RestResource::new::<Note, NoteSerializer>("notes")?],
)?;
let engine = client.engine(transport);
engine.pull().await?; // initialize independent cursors from zero
client.save(&note).await?;
engine.flush().await?;
```

Configure authentication and timeouts on the reqwest client. Each resource
mapping binds stable sync fields to writable serializer fields. Database column
aliases and serializer names are resolved before the request is persisted.
Credentials are supplied by the transport rather than saved in the request.

`pull` downloads one page per registered model; repeat while `has_more` is true.
`flush` sends one coalesced record per model and retries already submitted writes
first. `sync_once` settles pending writes, pulls journals and then sends new
writes when downloads are caught up. Inspect `SyncReport::blocked` and
`conflicts`; expose conflicts to users through `client.conflicts()` and
`client.resolve(...)` rather than looping indefinitely.

Save/delete offline through `ModelLogClient` or its configured database handle.
Unsubmitted edits coalesce; an unsent create followed by delete sends nothing.
Once submitted, a request's resource, body, revision and identity stay immutable
across retries and restarts. Subsequent local edits remain separate work.
Transport, authentication and unexpected HTTP failures leave that request pending;
only a canonical sync rejection settles it as permanently rejected.

The client persists current and confirmed data, dirty records, submitted
requests, cursors and conflicts transactionally. Missing FK parents defer local
projection while preserving confirmed state and cursor. Clone one client to
share its locks; independent concurrent clients/processes against the same
local database are unsupported.

### che-tauri managed state

Enable `che-tauri/sync` alongside `che-rest/sync-http` in the application.
The complete [Todo server and desktop client](../../che-tauri/examples/todo-sync/README.md)
demonstrates two persistent SQLite clients, offline restart, field merging,
conflict selection and deletion through these same REST endpoints.

```rust,ignore
use che_tauri::TauriSync;

// AppModule::schema() declares SchemaSet::new().client_model::<Note>().
// Apply their frozen compiled migrations before opening the managed state.
let sync = TauriSync::open(database, apps.sync_registry()?, transport).await?;
tauri::Builder::default()
    .manage(sync)
    .invoke_handler(tauri::generate_handler![che_tauri::che_model_sync])
    // Register application UUID CRUD commands in this same list.
    .run(tauri::generate_context!())?;
```

The common module lifecycle matches REST: schema describes persistence, init
registers handlers, and subscribe/start run after registration validation. Tauri
install only stores modules; build performs registration without schema writes.
Schema and sync-registry discovery never run init. See the
[AppModule migration guide](../../che-tauri/docs/app-modules.md) for removed
context APIs and existing application/database adoption.

Use fully qualified exported command paths so Tauri resolves their wrapper
macros. `TauriSync` opens without network access; it supplies `save`, `delete`,
`database`, `pull`, `flush`, `sync_once`, `conflicts` and `resolve`. Ordinary
`che_api` resources use i64 IDs, so the application supplies UUID CRUD commands.
The shared `che_orm::ModelSerializer` type used by `RestResource` describes the
server's writable fields; it is distinct from the metadata serializer in
`che-tauri`'s ordinary invoke resources.

The transport takes a reqwest **0.12** client. Ordinary `che-tauri` REST proxies
use reqwest 0.13; configure a matching application client, using a dependency
alias when both versions are needed. Authentication and timeouts belong to this
supplied client. TauriApi login/logout and `che_tauri::AppState` configuration
changes do not update the sync transport's credentials. Renew credentials for
the same account without discarding pending data, and use another database for another
server/account history.

Durable sync tables must not be registered as disposable remote cache tables.
Cache invalidation uses raw deletes. TauriApi build now rejects ordinary local
or cached resources mapped to declared sync tables. The check does not cover
separately configured runtimes or arbitrary triggers/cascades; keep those effects
isolated from durable sync data too.

With feature `sync`, the application's `che-tauri` management binary generates
`sync.ts` with `modelSync` and typed reports/conflicts/resolutions. Edit its
canonical `src/sync_client.ts` in che-tauri and regenerate application files.
Do not add Tauri invoke or frontend client generation to the sync engine.

The Todo workspace is verified separately:

```bash
cargo check --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml --workspace
cargo test --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml -p todo-sync-server -p todo-sync-client --no-default-features
```

Run these from the che-rest root. Tests bind an ephemeral loopback port and use
temporary files. Default-feature workspace checks include the desktop binary;
they do not launch or inspect the window. The example README has manual steps.

## Wire format and history

A changes page contains `model`, `history`, `operations`, `next_cursor` and
`has_more`. Advance using `next_cursor`; operation IDs may have gaps. Each
operation contains `id`, `record_id`, `revision`, `kind`, `diff`, `created_at`
and `schema_version`. Create contains full values, update changed values only,
and delete the last values. Missing old/new values differ from explicit JSON
null. Nullable JSON uses `old_sql_null` / `new_sql_null` to distinguish SQL NULL
from JSON null. Integers, revisions and cursors use decimal strings to retain
full i64 precision. Objects, arrays, booleans and other JSON values retain type.

Conditional writes carry `x-sync-protocol: 2`, `x-sync-replica`,
`idempotency-key`, `if-match`, `x-sync-version` and `x-sync-entity` headers.
The transport sets `x-sync-json-null-fields` when sending explicit JSON null.
Canonical responses use `application/vnd.che-sync.v2+json`: accepted create is
201, other accepted writes 200, conflict 412 and permanent rejection 400.

Journals are retained in full. Existing records require an explicit baseline
through `SyncServer::backfill_model_log::<Model>()` after schema migration and
before serving journals or new writes. Startup/GET never backfills automatically.
Review legacy scopes before migration: backfill refuses incompatible scoped
records. Preserve existing revisions and applied migration identities. History
replacement and cursor rewind require explicit application-level handling.

## Legacy compatibility

`SyncEndpoints`, `SyncClient`, `SyncEngine`, `RestSyncRequest` and the
[`todo_sync` example](sync.md) retain the scoped pull/snapshot protocol.
Keep legacy clients and pending requests on that path until they are settled
and explicitly migrated. `ModelLogClient::open` refuses to reinterpret legacy
outbox/cursor data. Protocol ownership, scope and replica metadata initialize in
one transaction: a failed open leaves the existing database unchanged. Legacy
`SyncClient::open` also refuses a protocol-2 database, including when supplied
the same internal scope. Do not enable both endpoint extensions for one ViewSet
or silently switch existing client databases to protocol 2.

## Durable external effects

Give every mutation hook a stable `MutationHook::id()`. Delete preparation owns
its payload: include an application payload version and enough persisted context
to execute cleanup without a live user credential. Each hook's payload is stored
with its handler identity; changing registration order does not reassign payloads.
Keep handlers idempotent because a crash after an external action but before
queue acknowledgement can cause another attempt.

Call `che_rest::sync::retry_model_effects(&state).await?` from a trusted background
worker after installing application modules. It retries queued effects without a
new mutation or user request and reports the number acknowledged. Missing
handlers and unsupported payload versions leave effects queued. Successful
handlers are acknowledged independently; failed handlers remain retryable.
Retain or explicitly migrate retired handler IDs and payload versions. Positional
legacy cleanup payloads are not assigned to hooks by their new registration order;
they require an application-owned payload migration.

## Rehearsing an existing installation's migration

Perform this sequence on a copy first. The framework cannot choose how an
application combines old scopes into whole-model authorization.

1. Stop new legacy writes, settle every submitted request through its existing
   endpoint/receipt identity, flush remaining local edits and resolve conflicts.
   Retain the old runtime and database backups until these outcomes are known.
2. Freeze the existing compiled history. Add a follow-up compiled migration for
   journal/state tables and any missing role service columns. Preserve business
   UUIDs, revisions, tombstones and the legacy journal/receipt tables. Never
   regenerate or edit an applied initial migration.
3. Review whole-model authorization explicitly. For records with an old nonempty
   scope, write and review an application data migration before backfill; the
   framework refuses to guess this ownership conversion. Do not change scopes
   while old submitted requests still require settlement.
4. Apply the follow-up migration to the copied server, open a model-log runtime
   and call `backfill_model_log::<Model>()` once per existing model. Keep ordinary
   writes stopped until all registered models have baselines. Verify canonical
   values, revisions, FK links and retained tombstones against the backup.
5. Open fresh migrated protocol-2 client databases and pull from cursor zero.
   Do not copy legacy cursors/outboxes into them. If any local changes could not
   be settled, retain the old database and explicitly reconcile those changes;
   switching protocols must never discard them.
6. Rehearse replay of previously settled IDs on the retained legacy path,
   compare histories/checksums and test create/update/delete on protocol 2.
   Enable writes only after verification. Keep the backup and legacy path for
   the application's chosen compatibility period.

The automated tests verify guarded backfill, rollback, unchanged metadata and
settlement of an existing legacy request. They cannot prove a migration of an
application database whose schema, ownership rules and data were not supplied.

## Abstraction boundaries

| Crate | Ownership |
| --- | --- |
| `che-orm` | Model fields, schema, SQL, queries, transactions and compiled migrations |
| `che-sync-protocol` | Model mutations, journal entries, canonical states and results |
| `che-sync` | Capture, revisions, receipts, current/shadow data, outbox, cursors and merge; generic delivery interface |
| `che-rest` | Resource routes, HTTP methods/status/headers, serializer field mapping, JSON request bodies and reqwest delivery |
| `che-tauri` | Managed client state, invoke commands and frontend synchronization client generation |

Dependencies point from `che-rest` through `che-sync` to `che-orm`. The data and
sync crates do not depend on HTTP types or know resource paths. `ModelWrite`
contains model changes and SQL-null metadata. `che_rest::RestModelWrite` owns the
HTTP resource/body representation previously exposed by the sync protocol.

Custom `ModelLogTransport` implementations provide a stable `id()`, encode an
opaque payload once using `prepare_write(&ModelWrite)`, then deliver it using
`write(&ModelWrite, &payload)`. The engine persists the payload with its transport
identity, reuses it after reopening and refuses reinterpretation by another
adapter. Adapters validate any identity metadata in their payload before side
effects. `changes` remains a generic journal-download operation.

`RestModelTransport` owns compatibility with previously saved `rest-request:*`
keys and preserves those payloads without re-encoding. Its current route mapping
can change without rewriting an already submitted resource/body. Unclaimed
legacy operation payloads block another adapter instead of being overwritten.
The storage schema and existing journal/receipt contents remain unchanged.

Source API changes for direct server integration: `request_receipt`,
`write_prepared_request`, `reject_request` and `prepare_conditional_write` replace
the earlier REST-named methods. These APIs accept data mutations and opaque
receipt identities; HTTP request interpretation stays in che-rest. Existing
receipts retain their encoded shape and remain replayable.
