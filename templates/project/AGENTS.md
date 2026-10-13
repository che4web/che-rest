# Project Guide

This is a che-rest application using Rust 1.88 or later. Keep application models
and compiled migrations in this project; do not create tables during startup.

## Workflow

```bash
cargo run --bin manage -- startapp tasks --model Task
cargo run --bin manage -- makemigrations --dir src/migrations --name initial
cargo run --bin manage -- migrate
cargo check --all-targets
cargo run
```

`src/apps/mod.rs` registers applications; each application owns models,
serializers, filters and ViewSets. `src/migrations/mod.rs` registers compiled
history. Preserve generator markers and applied migration identities/checksums.
Register model APIs with `ModuleContext::viewset_with`, not only `route`.
Database operations are async. Serializers convert materialized data and must
not query the database; load relations in the ViewSet queryset.

This framework build uses SQLite. Changing only the URL does not enable the
ORM's PostgreSQL backend. `Server::build()` does not apply migrations.

{% if with_auth %}Authentication is installed. Session login is at `/api-session-auth/login/`.
{% else %}Authentication is not installed; add `che_rest::auth::module()` before protected APIs.
{% endif %}Generated CRUD resources use AllowAny by default; choose their application permissions.

## Synchronization

Use shared UUID models with `#[orm(sync, sync_name = "app.model")]`. Register
server/client roles with `SchemaSet::server_model` / `client_model`; only clients
create shadows. Implement `SyncViewSet` and install `SyncEndpoints::new(...)`
in `ViewSet::configure`. Include dependencies in the registry, select scope from
trusted context and authorize it with `AccessPolicy`. A fixed scope with AllowAll
is a shared partition, not per-user isolation. Sync requests require authentication.

Ordinary REST writes provide delivery; there is no sync/push route. Writes outside
the adapter use configured SyncServer/SyncClient database handles or explicit sync
facades. Raw SQL and unconfigured handles bypass capture. Configured server reads
authorize scope and filter scope/tombstones, including transaction and SELECT AST reads.

Clients use independent `pull()` and `flush()` with explicit `full_load()` for
initial/expired cursors. Pending operations retry with their original IDs.
`sync_once()` can load snapshots automatically. Server replacement/history rewind
is unsupported. Preserve revisions, tombstones and receipts when pruning journals.

## Verification

After changing models, review `makemigrations`, apply them and run `cargo check`.
After changing endpoints, run `cargo run --bin manage -- generate-ts` and inspect
OpenAPI. Managed/server-owned serializer fields, including ImageField keys, are
read-only in JSON. Media storage and uploads belong to che-rest, not the ORM.
