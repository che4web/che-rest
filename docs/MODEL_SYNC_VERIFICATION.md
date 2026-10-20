# Model synchronization verification

Initial plan-verification snapshot recorded on 2026-10-20. The results below
cover steps 1-9 before the abstraction follow-up described at the end of this
file. That follow-up records verification of the refined layering separately.
No publication or application database migration was part of verification.

## Results

| Check | Result |
| --- | --- |
| che-rest `cargo test --features sync-http` | 116 passed; generated-project check run separately |
| che-orm `cargo test --workspace --locked` | 172 passed |
| Model journal regressions on Rust 1.88 | 14 passed |
| PostgreSQL ORM runtime | 3 passed on a disposable local server |
| PostgreSQL sync runtime/model roles/model journals | 9 passed on the same disposable server |
| Rust 1.88 workspace/all-targets, SQLite and PostgreSQL | Passed |
| Rust 1.88 che-rest/all-targets with sync-http | Passed |
| Five public crate archives, combined packaging and build | Passed |
| Extracted archives on Rust 1.88, SQLite/HTTP and PostgreSQL | Passed |
| che-rest and che-sync clippy, warnings denied | Passed; che-sync checked on both backends |
| Both repositories' formatting and diff whitespace | Passed |
| Workspace and REST rustdoc | Passed |
| Generated authenticated project build and repeated migrations | Passed |
| cli_fullstack smoke after lock-file updates | Passed |
| Admin/client Vue TypeScript and Vite builds | Passed |
| Admin identity Node test | Passed |
| Protocol-2 and legacy HTTP examples | Passed |
| ORM schema CLI and legacy native offline example | Passed |
| RustSec audits: ORM, REST, fullstack and todo lock files | No vulnerabilities or warnings |
| npm audits: admin and client | No vulnerabilities |

The PostgreSQL cluster was created under /tmp, used only for these tests and
stopped afterwards. PostgreSQL release CI now includes `model_log_postgres`.

## Regression coverage

- Server journals: independent cursors, canonical typed create/update/delete,
  no-ops, rollback, cascade/SET NULL, gaps and concurrent paginated reads.
- Local state: atomic page/cursor persistence, deferred FK projection, retry
  after parent arrival, independent field merge, overlapping/delete conflicts
  and explicit resolution.
- Delivery: offline coalescing, unsent create/delete cancellation, immutable
  submitted body/path/identity after lost committed replies and client reopen,
  retryable authentication failure and canonical REST results.
- Effects: failed cleanup survives runtime restart, missing handlers preserve
  payloads, hook reordering preserves ownership, background retries acknowledge
  handlers independently without adding journal operations.
- Compatibility: explicit revision-preserving backfill, rollback on incompatible
  legacy scope, atomic protocol ownership checks and an unchanged legacy request
  remaining settleable after a rejected protocol switch.
- Values: JSON objects, arrays and scalars, JSON null versus SQL NULL, aliases,
  UUIDs, offset/nanosecond timestamps and both i64 extremes.

## Packaging and dependency findings

The standard che-rest package command requires che-sync to exist in the registry.
It is not yet published. Four ORM/sync archives were verified in their ordinary
workspace. All five libraries were additionally copied into a temporary combined
workspace and packaged together with Cargo's temporary package registry. Those
archives were extracted into a separate workspace, patched only to each other's
extracted source, and checked using Rust 1.88 on SQLite/HTTP and PostgreSQL.
No unpublished dependency was substituted with another release.

The audit found rustls advisories in the REST/fullstack lock files and
quinn-proto advisories in the fullstack lock file. Updated rustls to 0.23.46,
fullstack rustls-webpki to 0.103.15 and fullstack quinn-proto to 0.11.18.
All four Rust lock-file audits and both npm audits are now clean.

Actual deployment remains application-owned. Rehearse its data/ownership
conversion on a copy using the migration procedure in model-sync.md. The tests
cannot certify an application database that was not supplied. Publication still
requires the documented dependency order and a reviewed clean release commit.

## Abstraction boundary follow-up

Moved `RestModelWrite`, resource/body encoding and compatibility with old HTTP
payload keys into che-rest. The sync protocol exposes transport-neutral
`ModelWrite`; che-sync persists opaque payloads under stable adapter identities.
Prepared-write/receipt APIs now use neutral names while keeping stored receipt
encoding unchanged. See model-sync.md for ownership and source API changes.

Verification after this refinement: 174 ORM workspace tests and 117 REST tests
passed, including 16 model-journal/native-transport tests and both current and
previously persisted HTTP request formats. Tests verify unchanged retries after
route mapping/codec changes and reject reinterpretation by another adapter.
The nine live PostgreSQL sync/runtime/model-journal tests also passed on a
separate temporary cluster, which was stopped afterwards. SQLite/PostgreSQL
Rust 1.88 source builds, REST sync-http MSRV build, formatting, clippy, rustdoc,
schema CLI and native offline example passed. The initial table remains a
historical snapshot; current archive verification is recorded below.

## Current archive verification

After the transport-neutral refactor, ran the complete reproducible package
check using the current working sources:

```bash
python3 scripts/verify_sync_packages.py
```

All five archives packaged and built successfully. Their extracted sources
passed Rust 1.88 all-target checks for SQLite/HTTP and PostgreSQL. The audit of
the extracted workspace's resolved dependency lock found zero RustSec
vulnerabilities and no advisory warnings. The generated authenticated project
build/repeated migrations and cli_fullstack smoke also passed after the refactor.

The successful run retained its logs, archives and `result.json` (including all
five archive SHA-256 hashes) under `/tmp/che-sync-packages-ntxn656h`. Re-running
creates a fresh directory; `--output NEW_DIRECTORY` chooses another location.
Source checkouts and their lock files are copied, not modified. Cargo's normal
registry/toolchain caches may be used. No crates were published.

Packaging intentionally uses online registry resolution: the offline packaging
attempt hit Cargo 1.98's temporary-registry checksum error for an unpublished
sibling. The standard online package verification succeeded. Extracted-source
MSRV checks run offline with patches only to the other extracted siblings.
The helper seeds their workspace lock from the actual che-rest archive; Cargo
package does not necessarily produce a lock at a temporary workspace root.

This records the successful archive run after the abstraction-boundary follow-up.
It used a warmed Cargo cache; it does not prove a clean-machine helper run.

### Known package-helper limitations

Subsequent review identified two tooling issues that remain unfixed:

- `cargo package` does not fetch every test/optional-feature dependency used by
  the later `--all-targets --offline` checks. Without a warmed cache, an extracted
  check can fail on a missing crate (reproduced with `trybuild`), even after
  packaging succeeds. Fetch the complete check graph first; a successful cached
  run must not be described as evidence that the helper bootstraps a clean cache.
- Only explicit `--output` paths are checked to be outside the source repos.
  Default `tempfile` output honors `TMPDIR`; if that points into a checkout, the
  staged files can be copied back into the source snapshot. Until the guard is
  fixed, use a new external `--output` directory, or keep `TMPDIR` outside both
  repositories.

Core protocol verification is complete; these packaging helper fixes, publication
and migration of a supplied application's database are separate remaining work.

## Downstream che-tauri integration

The sibling `che-tauri` repository now exposes `TauriSync`, `che_model_sync`,
client-schema registration and a generated `sync.ts` client. Its separate
[Todo workspace](../../che-tauri/examples/todo-sync/README.md) shares a UUID model
between a REST server and desktop client, using the same protocol-2 transport.
The example's binaries were built and its repeated server/client CLI migrations
were checked against temporary databases. The `two_clients` HTTP test verifies
offline restart, two devices, independent-field merge, same-field conflict and
explicit resolution, deletion and invalid credentials. This is a distinct
downstream check; it is not part of the historical root test counts above.

To repeat from the che-rest root:

```bash
cargo test --manifest-path ../che-tauri/Cargo.toml --features sync
cargo check --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml --workspace
cargo test --manifest-path ../che-tauri/examples/todo-sync/Cargo.toml -p todo-sync-server -p todo-sync-client --no-default-features
```

HTTP tests need an ephemeral loopback port. The desktop binary compiled with
default features; no GUI launch or visual inspection was performed in that run.
The example README provides manual two-window scenarios. Framework validation
does not validate an application's existing database migration or authorization.
