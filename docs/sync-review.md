# REST synchronization review

Review date: 2026-10-12. Updated: 2026-10-19. The PostgreSQL feature-selection
finding remains open. The five behavioral findings below are fixed and covered
by regression tests; UUID admin editing and relation selection are also fixed.
Five behavioral scenarios were reproduced in an external scratch crate; the
PostgreSQL integration issue was confirmed by a downstream build.

## P1: PostgreSQL REST integration cannot be selected

`Cargo.toml` enables default SQLite features on both `che-orm` and `che-sync`.
Enabling PostgreSQL downstream produces mutually exclusive backend features,
even with defaults disabled on the downstream `che-rest` dependency.
The portable native sync runtime test does not verify the HTTP layer.

Provide backend features in `che-rest`, disable dependency defaults, and run
the HTTP integration scenario against PostgreSQL.

## Fixed: Concurrent identical POST can return a rejection after commit

`src/sync.rs` looks up the receipt before serializer preparation. Two identical
requests can both miss the receipt. Once the first commits, the second can fail
in `SyncServer::prepare_write` with `UUID already exists`, before reaching the
transactional receipt replay. Reproduction returned 201 and 400 for the same
operation, replica, entity and body.

REST preparation now retains the captured base revision without checking UUID
existence early. The write transaction resolves receipts first; early errors and
conflicts recheck receipts too. Concurrent POST/PATCH/DELETE regression tests
verify identical canonical responses and a single journal change.

## Fixed: Transient HTTP errors consume the outbox

The example HTTP transport classifies all non-sync-envelope 4xx responses as
permanent `PushResult::Rejected`. A reproduced 429 response removed the immutable
outbox operation and created a non-retryable conflict. Review 408 and expired
authentication handling as well.

Non-sync HTTP errors now remain transport errors. The HTTP regression test checks
401, 408, 429, 503 and proxy 400 responses, preserving the original operation and
payload through a later successful retry. Retry-After is reported to the caller.

## Fixed: Ordinary DELETE skips dependent cleanup

Without conditional sync headers, the ordinary router prepares and dispatches
only the root ViewSet's delete hooks. The sync server logically deletes and
journals cascade dependents, but does not queue their lifecycle effects.
Reproduction deleted both parent and child while only invoking parent cleanup.

Both REST modes now use shared cascade preparation and transactional effects.
Regression tests verify dependent cleanup and lifecycle signals.

## Fixed: Ordinary POST hook failure changes a committed response

The ordinary CRUD path stops at the first `after_commit` error. Reproduction
created one row, returned 400, skipped the next hook, and persisted no effect
for retry. Repeating POST can create another row.

Both REST modes now persist independently retryable effects with the mutation.
Regression tests verify successful ordinary responses, continued dispatch and
later retry without repeating completed effects. PUT retains its Update action.

## Fixed: Installation allows unencodable writable sync fields

Installation checks that writable serializer fields are not server-owned, but
does not require writable sync fields to have writable serializer mappings.
An installed ViewSet was reproduced failing `RestSyncRequest::new` with
`sync field public_approved has no writable serializer field`. This can leave
an already prepared outbox operation failing before every subsequent pull.

Installation now validates both directions, requiring exactly one writable
serializer mapping per writable sync field. Aliased and FK fields remain valid.
