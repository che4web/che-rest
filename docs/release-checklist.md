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
cargo test
cargo test --manifest-path examples/cli_fullstack/Cargo.toml --test smoke
npm --prefix examples/cli_fullstack/frontend/client ci
npm --prefix examples/cli_fullstack/frontend/client run build
npm --prefix examples/cli_fullstack/frontend/admin ci
npm --prefix examples/cli_fullstack/frontend/admin run build
cargo doc --no-deps
cargo package --allow-dirty
cargo publish --dry-run --allow-dirty
```

## Publish

1. Publish the required `che-orm` release first and wait for its crates.io index entry.
2. Run `cargo publish` from a clean `che-rest` worktree.
3. Verify the crate page and docs.rs build, then create the matching Git tag and release.
