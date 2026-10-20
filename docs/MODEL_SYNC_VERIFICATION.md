# Model synchronization verification

Verified on 2026-10-20 against the current local che-rest/che-orm worktrees.
Framework implementation and verification cover steps 1-9 of MODEL_SYNC_PLAN.md.
No publication, commit or application database migration was performed.

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
