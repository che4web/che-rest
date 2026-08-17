# AGENTS.md

## Repo Shape
- This is a single Rust library crate, not a Cargo workspace; it depends on the sibling path `../che-orm2`.
- The ORM2 state/error and typed CRUD REST surface are implemented in this crate. Legacy ORM integrations are not part of the public build.
- Public API is re-exported from `src/lib.rs`; the REST implementation lives under `src/rest`.

## Verification Commands
- `cargo fmt --check` passes and is the fastest formatting check.
- `cargo test` is the main compile/test check; currently there are 0 local tests, so this mostly validates library and doctest compilation.
- `cargo clippy --all-targets -- -D warnings` currently fails on existing lints in `src/management.rs` (`single_match`) and `src/module.rs` (`should_implement_trait`); do not present a change as clippy-clean unless those baseline lints are fixed.

## Framework Gotchas
- `CrudViewSet::<Model, Serializer>::new("/resource")` uses serializer types; no serializer instance is constructed.
- Write handlers call `Serializer::is_valid` to build a `ValidatedWrite`; `ViewSet::prepare_*` may add server-owned fields before `ValidatedWrite::save` executes the mutation.
- Atlas is used to generate migrations; `migrate` and `migrate status` apply/read them through the built-in Refinery runner and do not require Atlas on the target system. Production startup must not create model tables.
- `Server::new(state).install(apps).build().await` installs app routers under `/api` by default; override with `api_prefix(...)`.
- `ModuleContext::viewset` and `viewset_with` both register the model schema, generated API metadata, and router; using only `route(...)` skips schema/codegen metadata.
- `ViewSet::get_queryset` defines filtering scope and relation loading; serializers only convert the materialized queryset item and never query the database.
- Nested serializer fields must name the generated relation marker, for example `#[serializer(one = User, relation = TaskAuthorRelation)]`; the viewset queryset must use the matching `select_related`/`prefetch_related` query type.
- In `examples/cli_fullstack`, `Task.author` is a read-only nested `User`; `Task::AUTHOR_ID` is assigned in `prepare_create` from `CurrentUser`, so clients must submit only task fields.
- Scalar writable relations use `#[serializer(foreign_key = User, relation = TaskAssigneeRelation)]` on an `i64` or `Option<i64>` field. The serializer source must match the generated relation marker; admin metadata then points `AsyncRelationSelect` at the related model endpoint.
- `examples/cli_fullstack` exposes optional writable `Task.assignee_id`; `/auth/users/` is a read-only admin relation endpoint and requires an admin user.
- Installing `che_rest::auth::module()` enables token auth middleware for all routes under the API prefix and separately adds `/api-token-auth/` outside the prefix.
- Management migrations use the project-level Atlas directory `migrations/`; `makemigrations` derives it from all installed app schemas and requires Atlas, while `migrate` applies checked-in SQL files without Atlas.
- Config loading only reads TOML shape `[database] url = "..."`; management commands can override with `--database-url`.
- `generate-ts` writes `api_client.ts`, `channels.ts`, `models.ts`, `api.ts`, `useModelList.ts`, and `useModelItem.ts`; `generate-admin` writes Vue admin files and the same generated client files under `src/generated/`. These outputs are produced from installed app metadata, not by scanning source files.
- `generate-admin` now creates a standalone Vite/Vue/Bootstrap project under `frontend/admin` by default. Re-running without `--force` updates only generated files and creates missing `src/admin/pages/<Model>{List,Form}.vue` wrappers; existing Vue/CSS/config/page files are preserved for user customization. Use `--force` to overwrite static templates and model pages.
- Admin generated metadata lives in `src/admin/generated/adminSchema.ts`, generated routes in `src/admin/generated/adminRoutes.ts`, and API client files in `src/generated/*`. `src/admin/adminSchema.ts` is only a compatibility re-export shim. Do not direct users to edit files under `generated/`.
- Canonical admin templates live under `src/admin/templates/`; the checked-in `examples/cli_fullstack/frontend/admin/` tree is generated output. Update the canonical templates first, then regenerate the example.

## Current Status
- ORM2 querysets are database-independent; terminal database methods execute them.
- Typed REST CRUD, filtering, pagination, relation loading, response reloads, writable FK metadata, and OpenAPI/Swagger integration use the ORM2 API.
- `examples/cli_fullstack` demonstrates nested server-owned `Task.author` plus writable nullable `Task.assignee_id` through REST and the generated admin relation picker.
- Keep `cargo fmt --check`, `cargo test`, frontend builds, and the `cli_fullstack` smoke test passing when changing this surface.
