# Per-model REST synchronization

## Agreed contract

This plan records the implemented whole-model protocol-2 contract. The later
requirement for objects shared through per-object permissions is a separate,
unimplemented [next-stage plan](OBJECT_SYNC_PLAN.md). Completion of steps below
does not imply that object-level grant/revocation delivery already exists.

- Clients write through ordinary resource POST/PATCH/PUT/DELETE endpoints.
- Each synchronized model owns a separate `<table>_sync_log` table and independent cursor.
- No workspace partition is required by the new API. In the first version, authorization to synchronize a model grants access to all of that model's synchronized records.
- A journal entry describes one object mutation: `id`, `record_id`, `revision`, `kind`, `diff`, `created_at`, and `schema_version`.
- `diff` is an array of `{field, old_value, new_value}`. Missing old/new values differ from explicit JSON null. Create contains the complete synchronized record; delete contains its last values; update contains changed fields only.
- Diff values describe committed server state, including defaults and trusted preparation. JSON values retain their JSON type; i64 values use a lossless decimal representation.
- Data, revision, journal and idempotent write results commit atomically. No-op writes produce no new revision or journal entry.
- Journal cursors follow committed writes. Gaps in IDs are permitted. Model locks are acquired in deterministic order when several models are touched.
- Journals are retained in full initially. A new client starts with cursor zero; existing histories require an explicit baseline/backfill before serving this protocol.
- Tauri stores editable current data, confirmed server data, dirty records, immutable submitted requests, per-model cursors and conflicts in SQLite.
- Unsubmitted writes to one object coalesce to its final state. An unsent create followed by delete sends nothing. A submitted request retains its identity and body until its outcome is known.
- Independent field edits merge; overlapping field changes and delete/update conflicts remain available for explicit user resolution.
- External effects have stable handler identities and versioned, handler-owned payloads. Workers may retry effects independently of user requests.
- Existing databases, applied migration histories and pending legacy requests must not be silently rewritten or discarded.

## Implementation sequence

1. Preserve the current worktree and define typed journal and REST result messages with protocol regression tests.
2. Generate per-model journal schemas through compiled migrations, with explicit baseline/backfill support for existing records.
3. Capture canonical old/new state in the server transaction, suppress no-ops, preserve commit ordering, and journal each affected model.
4. Expose authenticated GET `/{resource}/sync/changes?after=...&limit=...` with `operations`, `next_cursor` and `has_more`. Advance through IDs rather than assuming contiguous sequences.
5. Share ordinary REST validation/preparation/persistence; keep idempotent requests immutable and classify permanent rejection separately from transport/authentication failures.
6. Add client journal application, atomic per-model cursor persistence, dependency-safe deferred projection and final-state REST delivery.
7. Replace positional durable cleanup payloads with stable per-handler payloads, and provide request-independent effect retry.
8. Update generated clients, examples, agent guidance and user documentation. Preserve a documented legacy path until explicit migration.
9. Verify SQLite and PostgreSQL build paths, Rust 1.88, complete tests, fullstack smoke, generated-project migrations, frontend builds and dependency audit.

## Verification scenarios

- Many offline edits produce one final-state request; create/delete before submission produces none.
- A lost reply/restart preserves request identity and does not append a second journal operation.
- Create/update/delete, cascades and SET NULL journal their actual final state atomically.
- No-op writes do not advance revision; rollback leaves no log records.
- Concurrent writers and paginated readers do not lose committed operations.
- JSON objects/arrays/scalars, SQL null, aliases, UUIDs, dates and full-range i64 retain their type/value.
- Missing related records defer local projection without losing the downloaded journal or cursor.
- Concurrent offline edits preserve local data and expose same-field/delete conflicts.
- Hook reordering cannot change cleanup payload ownership; queued effects can be retried after restart without a new mutation.
- Legacy histories remain unchanged and legacy pending requests remain independently settleable.

## Execution status

- Framework implementation and verification of steps 1-9 are complete across `che-rest`, `che-sync-protocol`, `che-sync` and the ORM schema layer in `../che-orm`. See [MODEL_SYNC_VERIFICATION.md](MODEL_SYNC_VERIFICATION.md) for results and deployment boundaries.
- The existing uncommitted review fixes are retained. No commit, tag movement, publication, or migration of an application's database is authorized by this plan.

### 2026-10-20: REST/client integration verification

- Added `tests/model_sync_http.rs` against the real `RestModelTransport` and authenticated `ModelSyncEndpoints`: offline edit coalescing, lost committed reply, reopening the client, immutable replay, retry after authentication failure, follow-up edits, unsent create/delete cancellation, delete journal entries, JSON null and full-range i64.
- Added route authorization/parameter checks and TypeScript journal-client generation coverage. Corrected the legacy migration-state regression to assert the shared tables and each model's new journal/state tables by name.
- Added `cargo run --features sync-http --example model_sync`: two independent SQLite clients merge independent fields and propagate deletion through the per-model journal. Shared example/test setup uses role-specific schemas and compiled demo migrations.
- Documented protocol 2 setup, whole-model authorization, immutable delivery, explicit backfill and the retained legacy path in `docs/model-sync.md`; updated README and agent guidance.
- Verification: `cargo test --features sync-http` passed (115 tests, one generated-project test ignored); the new two-client example passed. This verifies the REST integration portion of steps 6/8, not completion of all plan scenarios.
- Remaining: complete the release matrix in step 9 and extend protocol-2 regression coverage for deferred FK projection, explicit backfill/legacy migration, concurrent pagination and independently retried effects. Existing legacy tests do not establish those properties for protocol 2.

### 2026-10-20: journal recovery and atomic client initialization

- Added protocol-2 regressions in `../che-orm/che-sync/tests/model_log.rs`: deferred FK projection retains the latest downloaded state/cursor across reopening; parent arrival retries projection; an invalid later operation rolls back the whole page and cursor; explicit backfill preserves revisions; incompatible legacy scope rolls back every backfilled row; gaps and concurrent paginated reads retain all committed operations.
- Fixed a reproduced protocol-switch bug: failed `ModelLogClient::open` previously committed `model_log_protocol` before scope validation failed. Protocol checks, scope ownership, replica identity and metadata initialization now share the same transaction in `SyncClient::open_with_protocol`. Legacy clients also reject protocol-2 databases rather than reinterpreting their state.
- Regression tests verify unchanged metadata on failed opening/invalid replica data and an immutable legacy outbox request remaining independently settleable after a rejected protocol switch.
- Formatted the existing sync implementation and resolved clippy findings in sync capture/dispatch without changing its intended behavior.
- Verification passed: ORM workspace tests, workspace documentation, schema CLI, legacy offline example, all 12 model-journal tests on Rust 1.88, PostgreSQL che-sync build/non-runtime tests, che-sync clippy with warnings denied, and che-rest tests/clippy with `sync-http`.
- Live PostgreSQL tests remain unverified because `TEST_POSTGRES_URL` is absent. Still outstanding: protocol-2 independent-effect retry coverage, the remaining release matrix (packaged builds, fullstack/generated-project/frontend checks and dependency audit), and application-owned explicit legacy migration procedures. No application database migration, commit or publication was performed.

### 2026-10-20: completion of framework plan

- Verified request-independent protocol-2 cleanup after runtime restart, absent handlers and hook reordering. Handler-owned payloads retain their identities; successful retries remove effects without new mutations.
- Added explicit same-field/delete conflict resolution and JSON scalar/null, timestamp/alias and full-range i64 regressions.
- Added a live PostgreSQL protocol-2 test for concurrent journal commits, no-ops, typed state, deferred FK projection and SET NULL; wired it into release CI. Ran the existing live PostgreSQL ORM and sync suites on an isolated temporary server and stopped it afterwards.
- Completed generated-project migrations, fullstack smoke, frontend builds, Node tests, rustdoc, examples, formatting/clippy, workspace/all-target MSRV checks and builds of all five extracted public package archives on Rust 1.88.
- Completed RustSec/npm audits; fixed vulnerable rustls/quinn-proto lock entries and verified clean follow-up audits. Source and fullstack smoke checks pass after the dependency changes.
- Documented the trusted background-effect worker and an explicit application-owned legacy migration rehearsal. Existing histories/pending requests remain protected; no application's data was migrated.
- Steps 1-9 are complete for the framework. Standalone registry packaging/publication of che-rest still follows dependency publication order; combined pre-publication packaging was verified without publishing anything. Actual application rollout requires its own supplied schema/data and authorization. Detailed evidence: [MODEL_SYNC_VERIFICATION.md](MODEL_SYNC_VERIFICATION.md).

### Abstraction boundary follow-up

- Removed the HTTP request DTO from che-sync-protocol. `ModelWrite` describes data changes; `che_rest::RestModelWrite` owns the resource/body encoding.
- che-sync persists transport-owned opaque payloads with stable adapter identities. REST-specific route/serializer/null-header encoding and old cache-key compatibility live in che-rest. Unknown legacy operation payloads block adapter changes without being rewritten.
- Renamed the prepared-write/receipt APIs to describe data operations rather than REST; existing database schema and stored receipt representation remain unchanged.
- Regression tests cover native delivery without HTTP, immutable payload reuse across reopen/codec changes, refusal of another adapter for current/legacy pending payloads, and old HTTP requests replayed after a route-mapping upgrade.
- See the abstraction-boundary section of model-sync.md for ownership and source API changes. The initial release-verification snapshot predates this refinement; the follow-up tests and backend/MSRV checks validate the refined layering.

### Reproducible package verification after abstraction follow-up

- Added `scripts/verify_sync_packages.py` to repeat the five-package check from current working sources without publishing or modifying either source checkout. It preserves logs and archive SHA-256 hashes, extracts the built crates and verifies their SQLite/HTTP and PostgreSQL paths on Rust 1.88.
- Rechecked generated authenticated-project builds/repeated migrations and the cli_fullstack smoke against the transport-neutral APIs; both passed.
- The current five-archive run passed, including extracted-source Rust 1.88 SQLite/HTTP and PostgreSQL checks. Its resolved dependency graph has zero RustSec vulnerabilities/warnings. Successful logs and hashes are recorded in MODEL_SYNC_VERIFICATION.md.
- No framework implementation/verification items remain in this plan. Publication, version/tag decisions and migration of an actual supplied application database remain separate authorized work.

### Downstream Tauri integration and review follow-up

- The sibling che-tauri repository now provides optional `TauriSync`, the `che_model_sync` command, generated `sync.ts`, and client-role schema registration. HTTP delivery still uses che-rest; data capture, outbox and merge remain in che-sync.
- A runnable [Todo server and desktop client](../../che-tauri/examples/todo-sync/README.md) has frozen role-specific migrations, offline SQLite writes, bounded manual sync and conflict selection. Its separate workspace check and two-device HTTP test complement root framework tests; desktop compilation is distinct from manual GUI inspection.
- The snapshot/full_load rules documented for legacy `SyncEngine` do not apply to protocol-2 model journals. Keep the user guides and agent instructions explicit about this distinction.
- The core protocol steps above remain complete. Subsequent review found two outstanding package-helper defects: offline checks rely on a warmed optional/dev-dependency cache, and default temporary output is not guarded against a TMPDIR inside a source checkout. See [the verification notes](MODEL_SYNC_VERIFICATION.md#known-package-helper-limitations) for the current workaround; the earlier successful cached archive run does not close these issues.
