# Project Plan

## Completed

- Decoupled ORM2 query construction from database execution.
- Updated REST viewsets to return database-independent querysets.
- Added typed CRUD handling for filtering, pagination, relation loading, and response reloads.
- Updated `todo_api` and `examples/cli_fullstack` for the new queryset API.
- Added nested `Task.author` serialization to `examples/cli_fullstack` using `select_related`.
- Made `Task.author_id` server-owned through `ViewSet::prepare_create`.
- Added `generate-ts` metadata collection and typed client generation from installed viewsets.
- `generate-ts` generates `auth.ts` when the auth app is installed.

## Verification

- `cargo fmt --check`
- `cargo test` in `examples/cli_fullstack`
- `git diff --check`
- `cargo run --bin manage -- generate-ts --out /tmp/che-rest-generated-ts`

## Next Steps

- Run the generated Vue client through `npm run build` and tighten generated TypeScript types based
  on compiler feedback.
- Keep the generated TypeScript/admin metadata aligned with installed app metadata.
- Add a REST example covering a reverse one-to-many relation with `prefetch_related` when a suitable example model is introduced.
- Preserve the database-independent queryset contract in new viewsets and examples.
