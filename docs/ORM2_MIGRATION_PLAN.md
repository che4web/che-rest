# che-orm2 Migration Plan

This document describes the breaking migration of `che-rest` from the legacy
`che-orm` API to `che-orm2`. The ORM2 REST implementation is owned by this
crate under `src/rest`; no separate experimental REST crate is part of the
architecture or dependency graph.

The migration targets new databases only. Existing legacy SQLite databases do
not need to be preserved or converted.

## Current State

The following work is already present:

- `che-orm2::Database` is stored in `AppState`.
- Typed ORM2 CRUD, filters, pagination, count queries and permissions exist.
- Auth models and auth queries use ORM2 models and builders.
- Management has an initial Atlas `schema`, `makemigrations` and `migrate`
  workflow.
- `todo_api` and `examples/cli_fullstack` use ORM2 models and serializers.

## Phase 0: Stabilize ORM2 Contracts

Complete these changes in `che-orm2` and the local REST implementation:

- Make serializer `is_valid` accept JSON and return a typed `ValidatedWrite`;
  serializers must not execute database operations.
- Keep `ValidatedWrite::save(database)` as the only persistence boundary for a
  validated mutation.
- Define strict create/update/patch semantics, defaults, custom validation and
  stable validation errors.
- Add tests for downstream applications, write-only fields, empty patches,
  constraints and nested metadata.
- Preserve server-owned fields through `ViewSet::prepare_*` hooks on the
  validated builder.
- Use `ViewSet::get_queryset` for tenant scoping and `select_related`/
  `prefetch_related`; serializers must only serialize materialized relation
  wrappers and must not access the database.

## Phase 1: Filters, Permissions and REST

- Keep filters typed and apply them consistently to list and count queries.
- Preserve global, object-level and owner/tenant query permissions.
- Map ORM2 errors to stable HTTP responses.
- Cover CRUD, strict JSON, filtering, ordering, pagination and permissions with
  integration tests.

## Phase 2: Authentication

- Complete token expiry and revocation behavior.
- Complete session data handling and optimistic revision updates.
- Preserve CSRF checks and current-user extraction.
- Add token, session, expiry, CSRF and authorization tests.

## Phase 3: New Atlas Migration Workflow

- Treat the ORM2 `SchemaSet` as the source for a new initial Atlas schema.
- Generate reviewed initial migrations for auth and application models.
- Register applications in dependency order when foreign keys are present.
- Verify `schema`, `makemigrations`, `migrate`, `status` and `lint` from an
  empty SQLite database.
- Do not add a legacy database baseline or compatibility migration.
- Do not use `Database::create_table` for production deployment.
- Remove obsolete schema snapshots and legacy migration code.

## Phase 4: OpenAPI, TypeScript and Admin

- Generate one OpenAPI document for all installed viewsets.
- Generate request and response schemas from serializer metadata.
- Include read-only/write-only fields, nested references, filters, ordering,
  pagination, errors and authentication security schemes.
- Generate TypeScript create/patch DTOs and API methods.
- Generate admin forms from serializer input metadata.

## Phase 5: Applications and Templates

Migrate and verify applications in this order:

1. `todo_api` as the smallest end-to-end proof.
2. `examples/cli_fullstack`, including auth, tasks, notifications, custom
   writes and channels.
3. `startproject` and `startapp` templates.

Generated projects must use:

- `che-orm2` model attributes;
- generated serializer DTOs;
- typed viewsets and filters;
- Atlas migrations from an empty database;
- the `che-rest` ORM2 state and server setup.

## Phase 6: Remove Legacy API

Remove:

- legacy dynamic serializers and filter/query types;
- `SqliteModel` constraints;
- legacy `che-orm` and SQLx ORM dependencies;
- legacy schema snapshots and migration application code;
- documentation and templates referring to the legacy ORM.

## Verification Gates

Run after every phase:

```bash
cargo fmt --check
cargo test
cargo doc --no-deps
```

Final end-to-end coverage must include strict JSON, serializer visibility,
relation loading without N+1, filters, pagination/count, auth flows, Atlas
migrations from an empty database, and generated frontend builds.
