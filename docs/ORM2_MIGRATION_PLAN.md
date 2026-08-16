# che-orm2 Migration Plan

This document describes the breaking migration of `che-rest` from the legacy
`che-orm` API to `che-orm2`.

Baseline references:

- `che-orm2` baseline: tag `v0.1.0`, commit `7a48e18`
- `che-rest` baseline: commit `3fe2f35`

The migration intentionally removes the legacy ORM API instead of supporting
two incompatible model and serializer systems in one public API.

## Phase 0: Stabilize ORM2 REST

Complete these changes in `che-orm2` before porting `che-rest`:

- Remove consumer-crate `#[cfg(feature = "sqlite")]` from generated serializer write APIs. Downstream crate features must not control dependency implementation details.
- Keep generated serializer inputs backend-neutral; keep SQLite execution in the REST/runtime layer.
- Make serializer metadata the source for OpenAPI, TypeScript and admin generation.
- Add filtered SQL `COUNT(*)`, typed `LIKE`/contains, bounded pagination and custom query hooks.
- Define strict create/update/patch semantics, defaults and stable validation errors.
- Add tests for downstream applications, write-only fields, empty patches, constraints and nested metadata.

## Phase 1: Replace Runtime ORM Boundary

Update `che-rest/Cargo.toml`, `src/lib.rs`, `src/state.rs`, `src/error.rs`,
`src/views.rs`, `src/serializer.rs`, `src/filters.rs` and
`src/permissions.rs`.

- Replace the legacy `che-orm` dependency with `che-orm2` and the ORM2 REST layer.
- Store `che_orm2::Database` in `AppState`.
- Remove SQLx error matching and map ORM2 errors to stable HTTP responses.
- Remove legacy `Field`, `ValidatedData`, `RelatedSerializer` and dynamic JSON-to-database conversion.
- Use generated `CreateInput`, `UpdateInput` and `PatchInput` types.
- Preserve custom create hooks, system-owned fields, list hooks and custom actions.
- Add explicit query/loading hooks for `select_related` and `prefetch_related`; serializers must not perform database access.

## Phase 2: Filters, Permissions and Modules

- Replace unsafe runtime field reconstruction with typed filter declarations.
- Apply filters to both list and count queries.
- Port permissions from `SqliteModel` to `che_orm2::Model`.
- Preserve global and object-level permissions plus owner/tenant query scoping.
- Replace `ModelSchema`, `ApiField` and legacy `FieldType` metadata with ORM2 model/serializer metadata.
- Keep channels and commands unchanged initially, migrating only their model/database call sites.

## Phase 3: Authentication

Port `src/auth/models.rs`, `src/auth/mod.rs` and `src/auth/views.rs`.

- Convert auth models to `#[derive(Model)]` and `#[orm(...)]` attributes.
- Replace legacy `NaiveDateTime` with `time::OffsetDateTime`.
- Declare all foreign keys and delete actions explicitly.
- Port token and session queries to typed ORM2 builders.
- Preserve session optimistic locking, token expiry/revocation, CSRF and current-user extraction.
- Replace raw SQLx session operations with parameterized ORM2 operations.
- Add token, session, expiry, CSRF and authorization tests.

## Phase 4: Atlas Migration Cutover

Replace the migration implementation in `src/management.rs` and project
templates in `src/project.rs` and `src/bin/che-rest.rs`.

Remove the legacy `Schema`, `diff_schemas`, `sqlite_migration_sql`,
`SqliteBackend` and per-app `schema.json` workflow.

Use ORM2 Atlas commands for:

- schema inspection;
- makemigrations;
- migrate/status/lint;
- application schema registration.

Convert existing auth and application schemas into reviewed initial Atlas
migrations. Preserve table names, foreign-key actions and indexes. Register
parent applications before applications containing foreign keys.

Do not use `Database::create_table` for production deployment.

## Phase 5: OpenAPI, TypeScript and Admin

- Generate one OpenAPI document for all installed viewsets.
- Generate request and response schemas from serializer metadata.
- Include read-only/write-only fields, nested references, filters, ordering, pagination, errors and auth security schemes.
- Port TypeScript interfaces and API methods to generated create/patch DTOs.
- Port admin forms to serializer input metadata rather than raw database columns.

## Phase 6: Application and Template Migration

Migrate applications in this order:

1. `todo_api` as the smallest end-to-end proof.
2. `examples/cli_fullstack` including auth, tasks, notifications, custom writes and channels.
3. `startproject` and `startapp` templates.

Update generated projects to use:

- `che-orm2` model attributes;
- generated serializer DTOs;
- typed viewsets and filters;
- Atlas migrations;
- ORM2 REST state and server setup.

## Phase 7: Remove Legacy API

Remove:

- legacy dynamic serializer implementation;
- legacy filter/query types;
- `SqliteModel` constraints;
- legacy `che-orm` and SQLx ORM dependencies;
- legacy schema snapshots and migration application code;
- documentation and templates referencing `../che-orm/crates/che-orm`.

## Existing Database Decision

Before Phase 4, decide whether deployed legacy SQLite databases must be
preserved:

- **New databases only:** start from a new ORM2 Atlas initial schema.
- **Preserve deployed databases:** create a one-time Atlas baseline/migration
  that matches existing auth and application tables before applying ORM2
  changes.

Use the second option if existing user or application data must survive.

## Verification Gates

Run after every phase:

```bash
cargo fmt --check
cargo test
cargo doc --no-deps
```

After migration support is ported:

```bash
cargo run --bin manage -- schema
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
cargo run --bin manage -- generate-openapi
cargo run --bin manage -- generate-ts
```

Final end-to-end coverage must include strict JSON, serializer visibility,
relation prefetch without N+1, filters, pagination/count, auth flows, Atlas
migrations from an empty database, and generated frontend builds.
