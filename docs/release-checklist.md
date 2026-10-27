# crates.io Release Checklist

## Prepare

The current release candidate is **che-rest 0.3.1**, requiring **che-orm 0.3.2**,
**che-sync 0.1.0**, and Rust **1.88+**. The generated project dependency versions
in `src/project.rs` and `templates/project/Cargo.toml` must match.

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
cargo clippy --all-targets -- -D warnings
cargo test --features sync-http
cargo run --features sync-http --example model_sync
cargo run --example todo_sync
cargo +1.88.0 check --all-targets --features sync-http --locked
cargo test --lib project::tests::generated_project_builds_and_migrates -- --ignored
TMPDIR=/tmp/opencode python3 scripts/check-template.py
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

1. Confirm all dependency releases are indexed on crates.io. For the sibling
   crates, the publication order is `che-orm-macros 0.3.2`, `che-orm 0.3.2`,
   `che-sync-protocol 0.1.0`, `che-sync 0.1.0`, then `che-rest 0.3.1`. Skip versions
   that are already published.
2. Run `cargo publish --dry-run` again after dependencies are indexed, then
   `cargo publish` from a clean `che-rest` worktree.
3. Verify the crate page and docs.rs build, then create the matching Git tag and release.

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
