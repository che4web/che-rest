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

For pre-publication model-sync verification and the unpublished dependency
packaging procedure, see [MODEL_SYNC_VERIFICATION.md](MODEL_SYNC_VERIFICATION.md).

## Publish

1. Publish the required `che-orm` release first and wait for its crates.io index entry.
2. Run `cargo publish` from a clean `che-rest` worktree.
3. Verify the crate page and docs.rs build, then create the matching Git tag and release.
