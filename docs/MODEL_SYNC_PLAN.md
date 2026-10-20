# Per-model REST synchronization

## Agreed contract

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

- Plan saved. Implementation is in progress across `che-rest`, `che-sync-protocol`, `che-sync` and the ORM schema layer in the sibling `../che-orm` repository.
- The existing uncommitted review fixes are retained. No commit, tag movement, publication, or migration of an application's database is authorized by this plan.
