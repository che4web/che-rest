# crates.io Release Checklist

## Prepare

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
node --experimental-strip-types --test tests/admin_identity.mjs
cargo test --manifest-path examples/cli_fullstack/Cargo.toml --test smoke
npm --prefix examples/cli_fullstack/frontend/client ci
npm --prefix examples/cli_fullstack/frontend/client run build
npm --prefix examples/cli_fullstack/frontend/admin ci
npm --prefix examples/cli_fullstack/frontend/admin run build
cargo doc --no-deps --features sync-http
cargo audit
cargo audit --file examples/cli_fullstack/Cargo.lock
cargo audit --file todo_api/Cargo.lock
npm --prefix examples/cli_fullstack/frontend/client audit
npm --prefix examples/cli_fullstack/frontend/admin audit
cargo package --allow-dirty
cargo publish --dry-run --allow-dirty
```

Before dependency publication, verify all five sibling packages together:

```bash
python3 scripts/verify_sync_packages.py --output /tmp/che-sync-release-check
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

1. Publish the required `che-orm` release first and wait for its crates.io index entry.
2. Run `cargo publish` from a clean `che-rest` worktree.
3. Verify the crate page and docs.rs build, then create the matching Git tag and release.
