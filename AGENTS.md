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
- The examples use Atlas migrations for schema setup. Production startup must not create model tables.
- `Server::new(state).install(apps).build().await` installs app routers under `/api` by default; override with `api_prefix(...)`.
- `ModuleContext::viewset` and `viewset_with` both register the model schema, generated API metadata, and router; using only `route(...)` skips schema/codegen metadata.
- `ViewSet::get_queryset` defines filtering scope and relation loading; serializers only convert the materialized queryset item and never query the database.
- Installing `che_rest::auth::module()` enables token auth middleware for all routes under the API prefix and separately adds `/api-token-auth/` outside the prefix.
- Management migrations default to project app files under `src/apps/<app>/migrations`; for app `auth`, migration application falls back to this crate's `src/auth/migrations` if the downstream project has no auth migration dir.
- Config loading only reads TOML shape `[database] url = "..."`; management commands can override with `--database-url`.
- `generate-ts` writes `api_client.ts`, `channels.ts`, `models.ts`, `api.ts`, `useModelList.ts`, and `useModelItem.ts`; `generate-admin` writes Vue admin files and the same generated client files under `src/generated/`. These outputs are produced from installed app metadata, not by scanning source files.
- `generate-admin` now creates a standalone Vite/Vue/Bootstrap project under `frontend/admin` by default. Re-running without `--force` updates only generated files and creates missing `src/admin/pages/<Model>{List,Form}.vue` wrappers; existing Vue/CSS/config/page files are preserved for user customization. Use `--force` to overwrite static templates and model pages.
- Admin generated metadata lives in `src/admin/generated/adminSchema.ts`, generated routes in `src/admin/generated/adminRoutes.ts`, and API client files in `src/generated/*`. `src/admin/adminSchema.ts` is only a compatibility re-export shim. Do not direct users to edit files under `generated/`.
