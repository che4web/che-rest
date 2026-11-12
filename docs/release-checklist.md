# crates.io Release Checklist

The [2026-11-06 verification report](release-verification-2026-11-06.md) is
historical. Reverify the current breaking-change candidates before publication.

## Prepare

The current release candidate is **che-rest 0.4.0**, requiring **che-orm 0.4.0**,
**che-sync 0.2.0**, and Rust **1.88+**. The generated project dependency versions
in `src/project.rs` and `templates/project/Cargo.toml` must match.

Registry checked 2026-11-12: core 0.3.3, macros 0.3.2, protocol 0.1.0, sync 0.1.3
and REST 0.3.4 are already published; che-tauri has no registry entry. The new
core/REST 0.4 and sync 0.2 minor lines reflect breaking bound-query and SQLite
storage contracts. Publish core, then sync, then REST only after separate approval
and dependency indexing. Do not republish old versions or unchanged macros/protocol.

### Upgrade notes for 0.4.0

- Choose exactly one compile-time backend; SQLite remains the default.
- Database-bound query terminal methods no longer take a database argument.
- Existing SQLite apps need explicit compiled datetime normalization migrations
  before startup, including sync current/shadow/journal columns. Preserve opaque
  payloads, receipts and cursors. PostgreSQL stays native TIMESTAMPTZ.

### Upgrade notes for 0.3.4

- Batched journal reads are explicit opt-in and require an application-mounted
  aggregate endpoint; installing `ModelSyncEndpoints` alone does not mount it.
- Standalone actions register schemas/client methods without CRUD/admin pages.
  Downstream `ApiEndpoint` struct literals must include the new `crud` flag.
- Keep durable per-model positions, immutable submitted writes and adapter
  identity. History resets block new submissions without deleting client work.
- No automatic DDL, backfill, protocol switch or database migration is added.

### Upgrade notes for 0.3.3

- Push notifications are optional (`push` feature). Install the `notifications`
  module, explicitly apply its compiled migration, and start delivery separately
  with the configured database and application-owned PushSource.
- Keep Firebase service-account credentials server-side. See [push](push.md).
- Preserve submitted sync operations, journals and receipts across upgrades.
  Large model downloads can use per-resource page limits without changing writes.

### Upgrade notes for 0.3.1

- Custom permissions use the asynchronous `Permission` methods and
  `PermissionContext`. Object-scoped policies must implement SQL `scope`; its
  default denies access. See [access control](access.md).
- Apply installed compiled histories with `manage migrate`, including the auth
  follow-up `auth:0002_access`. Do not rewrite frozen applied migrations.
- Protocol 2 and explicit protocol 3 are separate synchronization contracts.
  Existing clients must not switch protocols or discard outbox operations
  implicitly. See [model sync](model-sync.md) and [object sync](object-sync.md).
- Newly generated projects already contain the `tasks` app in either layout.
  Generate the first migration with `makemigrations --name initial`; use
  `createapp` for additional apps. The original Rust `startproject(options)` API
  still creates an empty app registry.
- New models infer app-prefixed tables via a shared `APP_NAME`. This does not
  rename existing application tables. Preserve existing explicit `table`
  attributes and review all generated migration operations.
- `Server::build()` prints a configuration banner by default. Applications that
  require silent router construction can call `.startup_banner(false)`.

### Preparation steps

- [ ] Decide the `che-rest` version and update `Cargo.toml` and `CHANGELOG.md`.
- [ ] Confirm the required `che-orm` version is available on crates.io.
- [ ] Review public API changes, README examples, and generated-client output.
- [ ] Review `cargo package --list` and exclude files not needed by library users.

## Verify

```bash
cargo fmt --check
cargo clippy --features sync-http,push --all-targets -- -D warnings
cargo test --features sync-http,push
cargo clippy --no-default-features --features postgres,sync-http,push --all-targets -- -D warnings
TEST_POSTGRES_URL='postgresql://localhost/test_database' \
  cargo test --no-default-features --features postgres,sync-http,push -- --include-ignored
cargo run --features sync-http --example model_sync
cargo run --example todo_sync
cargo +1.88.0 check --all-targets --features sync-http,push --locked
cargo +1.88.0 check --all-targets --no-default-features --features postgres,sync-http,push --locked
cargo test --lib project::tests::generated_project_builds_and_migrates -- --ignored
TMPDIR=/tmp/opencode python3 scripts/check-template.py
TEST_POSTGRES_URL='postgresql://localhost/test_database' \
  TMPDIR=/tmp/opencode python3 scripts/check-template.py --backend postgres
node --experimental-strip-types --test tests/admin_identity.mjs
cargo test --manifest-path examples/cli_fullstack/Cargo.toml --test smoke
npm --prefix examples/cli_fullstack/frontend/client ci
npm --prefix examples/cli_fullstack/frontend/client run build
npm --prefix examples/cli_fullstack/frontend/admin ci
npm --prefix examples/cli_fullstack/frontend/admin run build
cargo doc --no-deps --features sync-http
cargo audit
cargo audit --file examples/cli_fullstack/Cargo.lock
npm --prefix examples/cli_fullstack/frontend/client audit
npm --prefix examples/cli_fullstack/frontend/admin audit
cargo package --allow-dirty
cargo publish --dry-run --allow-dirty
```

Before dependency publication, verify all five sibling packages together:

```bash
TMPDIR=/tmp/opencode python3 scripts/verify_sync_packages.py --output /tmp/opencode/che-sync-release-check
```

This requires Python 3.11+, Git, Cargo and Rust 1.88.0. It snapshots the working
sources into a new temporary directory, packages the five crates together,
extracts their archives and checks all targets on Rust 1.88 for SQLite/HTTP and
PostgreSQL. Packaging uses the registry; extracted-source checks run offline.
Source repositories and their lock files stay unchanged. Logs, archives and
SHA-256 hashes are retained in the printed verification directory. Use
`--orm PATH`, `--output NEW_DIRECTORY` or `--toolchain VERSION` when needed.
The output directory must be new. The current helper requires a warmed Cargo
cache for all-target/optional-feature dependencies; packaging alone does not
fetch every dependency needed by its offline checks. Keep TMPDIR outside both
repositories when using default output. See the
[known helper limitations](MODEL_SYNC_VERIFICATION.md#known-package-helper-limitations).

Use a fresh output directory on the main filesystem if the temporary filesystem
is too small for package verification builds. Do not place it inside either source
repository. Keep the SHA-256 report with the release verification results.

Review the archive for local databases, uploads, generated applications (including
`my-project/`) and build artifacts. The standalone cargo-generate project is
distributed through Git; the installed CLI embeds its own templates in
`src/project.rs`.

See [MODEL_SYNC_VERIFICATION.md](MODEL_SYNC_VERIFICATION.md) for recorded results.

When changing synchronization or serializer mapping, also verify the matching
che-tauri Todo consumer from the che-rest root:

```bash
cargo check --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml --workspace
cargo test --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml -p todo-sync-server -p todo-sync-client --no-default-features
```

The latter uses temporary SQLite files and an ephemeral HTTP port. Use the
[example README](../../che-tauri/examples/todo-sync/README.md) for a manual GUI
check; a successful desktop build only verifies compilation.

## Publish

1. Confirm all dependency releases are indexed on crates.io. The registry check on 2026-11-12 confirmed
   `che-orm-macros 0.3.2`, `che-orm 0.3.3`, `che-sync-protocol 0.1.0`,
   `che-sync 0.1.3` and `che-rest 0.3.4` are published and not yanked.
   New candidates are core 0.4.0, then sync 0.2.0, then REST 0.4.0.
   Do not republish unchanged dependencies or move old tags.
2. Run `cargo publish --dry-run` again after dependencies are indexed. Actual
   `cargo publish` from a clean worktree requires separate explicit approval.
3. Verify the crate page and docs.rs build, then create the matching Git tag and release.

### Candidate verification (2026-11-02)

- Tests with all features, all-target Clippy with warnings denied, documentation
  and Rust 1.88 all-target compilation passed.
- The six sibling package archives, including che-sync 0.1.2 and che-tauri 0.1.2,
  packaged together. Extracted ORM/REST/sync sources passed Rust 1.88 SQLite/HTTP
  and PostgreSQL compilation; extracted che-tauri passed on current stable.
- Tacit mobile offline/attachment/comment/whiteboard/push tests and the shared
  Todo HTTP integration passed against the prepared dependency versions.
- No candidate was published. che-orm 0.3.3 and che-sync 0.1.2 must be indexed
  before the ordinary che-rest registry dry run can succeed.

### 0.3.1 publication status

On 2026-10-27, `cargo publish --dry-run --allow-dirty` was blocked because
`che-sync` was not present in the crates.io index. The crates.io API confirmed
that `che-orm-macros 0.3.2` and `che-orm 0.3.2` are published and not yanked,
while `che-sync-protocol 0.1.0`, `che-sync 0.1.0` and this REST release are not
published. The remaining order is protocol, sync, then REST.

The local sibling dependency
build does not prove registry availability. Use the five-archive verifier above
for pre-publication checks, then repeat the ordinary dry run after publishing the
dependencies. No publication is performed by the verifier.
