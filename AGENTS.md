# AGENTS.md

## Repo Shape
- This is a single Rust library crate, not a Cargo workspace; it depends on the sibling path `../che-orm`.
- The ORM2 state/error and typed CRUD REST surface are implemented in this crate. Legacy ORM integrations are not part of the public build.
- Public API is re-exported from `src/lib.rs`; the REST implementation lives under `src/rest`.
- The runnable Todo desktop integration lives in the sibling `../che-tauri/examples/todo-sync` Cargo workspace. It is a downstream consumer, not a member of this crate's default checks.

## Verification Commands
- `cargo fmt --check` passes and is the fastest formatting check.
- `cargo test` is the main library and integration test check, including media storage, route authorization, and ViewSet image cleanup tests.
- `cargo clippy --all-targets -- -D warnings` passes and is required for release verification.
- The minimum supported Rust version is 1.88; verify packaged builds on that toolchain when changing dependencies.
- After changing project templates, run `cargo test --lib project::tests::generated_project_builds_and_migrates -- --ignored`; it builds a generated authenticated application and verifies initial/repeated compiled migrations.

## Framework Gotchas
- `CrudViewSet::<Model, Serializer>::new("/resource")` uses serializer types; no serializer instance is constructed.
- Write handlers call `Serializer::is_valid` to build a `ValidatedWrite`; `ViewSet::prepare_*` may add server-owned fields before `ValidatedWrite::save` executes the mutation.
- Compiled Rust migrations are the only migration format. `migrate` and `migrate status` use the built-in forward-only SQLite executor. Production startup must not create model tables.
- `Server::new(state).install(apps).build().await` installs app routers under `/api` by default; override with `api_prefix(...)`.
- `ModuleContext::viewset` and `viewset_with` both register the model schema, generated API metadata, and router; using only `route(...)` skips schema/codegen metadata.
- `ViewSet::get_queryset` defines filtering scope and relation loading; serializers only convert the materialized queryset item and never query the database.
- Image files are handled by `src/media.rs`: install `che_rest::media::module()` alongside auth to serve authenticated `/api/media/{key}`, use `ImageStorage` and `ImagePolicy` in application-owned upload handlers, and store `che_orm::ImageField` relative keys rather than bytes or URLs. The default `[media] root` is `uploads`. `read_image_field` limits multipart parts; `save` decodes and validates JPEG/PNG/GIF/WebP and optionally re-encodes bounded PNGs.
- Declare nullable field actions once via `ViewSetConfig::image_field(ImageFieldEndpoint::new(...))`: the framework mounts multipart PUT/DELETE under `/{id}/images/{field}/`, reuses ViewSet permissions/queryset by default, manages file replacement/clear, and generates OpenAPI and typed FormData/DELETE clients. Use `authorize`, `before` (can retain an owned guard until persistence), and `write` callbacks for field-specific authorization, revision checks, and metadata in one CAS write. A custom `authorize` replaces the default ViewSet write permission: it must check authentication/ownership itself. `ImageCreateEndpoint` is the analogous declarative POST `/{resource}/upload/` action for image records with application-owned creation/linking callbacks; create related database rows in one transaction so file cleanup on callback failure cannot leave an orphan row. Non-image file endpoints remain application-owned.
- JSON serializers always treat `ImageField` as server-owned/read-only. `generate-ts` maps it to `string` or `string | null` (including `db_column` aliases), and generated admin metadata uses the `image` field type. Regenerate clients after changes to media serializers.
- `ImageStorage::replace_with`/`clear_with` compare-and-swap the previous image key and permit related server-owned metadata in the same update; stale writes return 409 and discard the new file. `replace`/`clear` are the no-extra-fields variants. Failed post-commit removals are queued under `[media] root/.pending-image-deletions`, retried on startup or later uploads, and must not make committed DELETE/replace return 500. `ViewSetConfig::mutation_hook(ImageCleanup { ... })` handles direct ViewSet deletion; `prepare_delete_async` can collect dependent keys before a parent cascades and `after_delete` removes them after commit. Raw database deletes bypass these hooks and require separate cleanup.
- Nested serializer fields must name the generated relation marker, for example `#[serializer(one = User, relation = TaskAuthorRelation)]`; the viewset queryset must use the matching `select_related`/`prefetch_related` query type.
- In `examples/cli_fullstack`, `Task.author` is a read-only nested `User`; `Task::AUTHOR_ID` is assigned in `prepare_create` from `CurrentUser`, so clients must submit only task fields.
- Scalar writable relations use `#[serializer(foreign_key = User, relation = TaskAssigneeRelation)]` on an `i64` or `Option<i64>` field. The serializer source must match the generated relation marker; admin metadata then points `AsyncRelationSelect` at the related model endpoint.
- `examples/cli_fullstack` exposes optional writable `Task.assignee_id`; `/auth/users/` is a read-only admin relation endpoint and requires an admin user.
- Installing `che_rest::auth::module()` enables token auth middleware for all routes under the API prefix and separately adds `/api-token-auth/` outside the prefix.
- Installing `che_rest::channels::module()` adds `/api/ws/` for declared public signals. CRUD lifecycle signals are opt-in via `ViewSet::signal_access`, use default dotted names like `tasks.created`, and publish `{ "id": ... }` payloads. Use `ModuleContext::signal(...)` for custom public signals. `AppState::app_channels()` remains internal-only.
- New project migrations live in `src/apps/<app>/migrations/`. AppModule::migrations exposes compiled history; migration_path exposes its writable source directory. Management collects installed histories automatically. Library modules omit migration_path. Keep legacy Management::migrations registries compatible.
- `src/project.rs` owns the startproject/startapp templates. Generated dependency requirements must match the current framework and ORM versions; include a compilable empty `all()` registry before the first migration. The initial migration name is passed as `makemigrations --name initial`, not as the positional application argument.
- `templates/project` is the cargo-generate template. Keep its skeleton and AGENTS.md aligned with `src/project.rs`. Default makemigrations routes to app-owned directories; an explicit non-default --dir preserves legacy directory behavior. See `templates/README.md` for generation commands.
- Every `ViewSet` declares `type ListSerializer`: use `Self::Serializer` for matching list/detail shapes or a separate `ModelSerializer` with the same model and queryset input. Its fields drive list responses, OpenAPI, generated TypeScript list items and admin table columns. After serializer changes, regenerate clients.
- Table-rebuild migrations temporarily remove and restore historically declared dependent triggers/views even when another app owns them; review the generated operations and keep unmanaged SQLite objects subject to executor checks.
- Config loads `[database] url = "..."` and optional `[media] root = "uploads"`; use `migrate --config PATH` to select another database configuration. There is no `--database-url` flag in the current CLI.
- `generate-ts` writes `api_client.ts`, `channels.ts`, `models.ts`, `api.ts`, `useModelList.ts`, and `useModelItem.ts`; `generate-admin` writes Vue admin files and the same generated client files under `src/generated/`. These outputs are produced from installed app metadata, not by scanning source files.
- `generate-admin` now creates a standalone Vite/Vue/Bootstrap project under `frontend/admin` by default. Re-running without `--force` updates only generated files and creates missing `src/admin/pages/<Model>{List,Form}.vue` wrappers; existing Vue/CSS/config/page files are preserved for user customization. Use `--force` to overwrite static templates and model pages.
- Admin generated metadata lives in `src/admin/generated/adminSchema.ts`, generated routes in `src/admin/generated/adminRoutes.ts`, and API client files in `src/generated/*`. `src/admin/adminSchema.ts` is only a compatibility re-export shim. Do not direct users to edit files under `generated/`.
- Canonical admin templates live under `src/admin/templates/`; the checked-in `examples/cli_fullstack/frontend/admin/` tree is generated output. Update the canonical templates first, then regenerate the example.

## Current Status
- ORM2 querysets are database-independent; terminal database methods execute them.
- Typed REST CRUD, filtering, pagination, relation loading, response reloads, writable FK metadata, and OpenAPI/Swagger integration use the ORM2 API.
- `examples/cli_fullstack` demonstrates nested server-owned `Task.author` plus writable nullable `Task.assignee_id` through REST and the generated admin relation picker.
- Keep `cargo fmt --check`, `cargo test`, frontend builds, and the `cli_fullstack` smoke test passing when changing this surface.
- SyncViewSet writes use the ordinary POST/PATCH/PUT/DELETE routes with serializer validation and preparation. Conditional sync headers carry revision, offline UUID and receipt identity; there is no sync/push route. RestSyncRequest encodes single-record writes, and HTTP transports advertise max_mutations() == 1. Preserve canonical sync-mode responses, transactional receipts/journal and independently retryable lifecycle effects.

## Per-model Synchronization
- Prefer `ModelSyncEndpoints` with `ModelLogClient` for protocol 2; `SyncEndpoints` and the existing `todo_sync` example retain the scoped legacy protocol. Do not silently migrate existing client databases or pending requests.
- Register role-specific `server_model` / `client_model` schemas and apply compiled migrations. Existing server data requires explicit `SyncServer::backfill_model_log`; startup and journal GET must not backfill automatically.
- Protocol 2 grants whole-model access; journal downloads do not apply ViewSet queryset filters. Match ordinary CRUD permissions to that access contract.
- The next requirement is shared objects with per-object permissions, including grants and revocations independent of data edits. `docs/OBJECT_SYNC_PLAN.md` is an unimplemented design, not a supported endpoint option. Keep one UUID/revision across users; do not substitute per-owner scopes or merely filter the current journal. Preserve existing protocol/database/outbox contracts until explicit versioned migration.
- `sync-http` enables `sync_http::{RestModelTransport, RestResource}`. Run `cargo test --features sync-http` for the real HTTP retry/authentication tests and `cargo run --features sync-http --example model_sync` for the two-client workflow. Both bind an ephemeral loopback port.
- See `docs/model-sync.md` for the current protocol and `docs/sync.md` for legacy compatibility. Preserve immutable submitted paths/bodies/identities when changing serializers or transports.
- Run `che_rest::sync::retry_model_effects(&state)` from a trusted worker after installing modules. Hook IDs and versioned delete payloads must remain stable; missing handlers retain their queued effects. See `docs/MODEL_SYNC_VERIFICATION.md` for the completed protocol-2 verification matrix.
- Keep HTTP DTOs and serializer/resource/header mapping in che-rest. The sync protocol exposes `ModelWrite`; generic adapters implement `ModelLogTransport::{id, prepare_write, write}` and persist opaque payloads. `RestModelWrite` is exported by che-rest. Preserve old submitted payloads through the HTTP adapter's compatibility key.
- The ORM and sync crates must not depend on HTTP or Tauri. `che-tauri/sync` wraps `ModelLogClient` as `TauriSync` and the `che_model_sync` invoke command; it reuses this crate's HTTP transport instead of duplicating the protocol. See `docs/model-sync.md` for managed-state setup.
- `RestResource` uses the server's `che_orm::ModelSerializer` mapping, not `che_tauri::ModelSerializer`. `RestModelTransport` accepts reqwest 0.12; ordinary che-tauri remote resources use 0.13. Keep application dependency versions/aliases explicit and sibling ORM crate identities aligned.
- Protocol-2 `pull` starts at cursor zero and does not use snapshots/full_load. `flush` sends at most one coalesced record per model, retrying immutable submitted payloads first. Keep UI/worker cycles bounded and stop on blocked/conflict/rejected reports.
- Tauri UUID CRUD commands write through the managed sync client; ordinary `che_api` resources keep i64 IDs. Register external Tauri commands with fully qualified paths such as `che_tauri::che_model_sync`. Invoke DTOs and frontend sync client generation belong in che-tauri.
- Sync credentials belong to the supplied transport. AppState login/logout/config changes in che-tauri do not update them. Renew credentials for the same server/account without clearing pending operations or silently reassigning a client database.
- Durable sync tables must not participate in remote response cache invalidation, directly or through cascades. TauriApi build rejects ordinary local/cached resources mapped to declared sync tables; separately configured runtimes and arbitrary triggers/cascades remain application-owned isolation checks.
- For changes affecting native clients, run `cargo test --manifest-path ../che-tauri/Cargo.toml --features sync` and the Todo workspace checks below. Root REST tests do not replace them.
- Before upstream dependency publication, use `python3 scripts/verify_sync_packages.py --output /tmp/NEW_DIRECTORY` for five-crate packaging and extracted-archive Rust 1.88 checks. Python 3.11+, Git and Rust 1.88.0 are required. Results and hashes stay in that new output directory. The current helper needs a warmed cache for all-target/optional-feature dependencies; its offline checks do not fetch them. Keep TMPDIR outside source repositories if omitting --output. These known helper limitations are documented in `docs/MODEL_SYNC_VERIFICATION.md`.

- The downstream AppModule lifecycle now matches the common REST phases: pure schema, declarative init during build, then subscribe/start after validation. Tauri schema/sync-registry discovery does not run init; ordinary tables are migration-owned too. See `../che-tauri/docs/app-modules.md` when updating consumers and do not reintroduce startup DDL.

## Downstream Todo Verification

Run from the che-rest root when changing synchronization or serializer contracts:

```bash
cargo check --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml --workspace
cargo test --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml -p todo-sync-server -p todo-sync-client --no-default-features
cargo clippy --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml --workspace --all-targets --no-deps -- -D warnings
```

The HTTP test binds an ephemeral loopback port and uses temporary file databases.
It covers offline reopen, repeated compiled migrations, two-device field merging,
same-field conflicts, explicit resolution, deletion and invalid credentials.
Default-feature workspace checks compile the Tauri desktop binary and exported
command wrappers. GUI launch/inspection is a separate manual check. Preserve the
example's frozen migrations and explicit migrate commands; do not create model
tables or backfill journals during server/client startup.
