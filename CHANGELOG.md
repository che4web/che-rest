# Changelog

## Unreleased

## 0.3.1

- Add protocol-2 per-model synchronization, REST HTTP transports and independently
  retryable lifecycle effects; preserve immutable submitted requests and receipts.
- Add asynchronous direct/group object permissions and explicit protocol-3
  object synchronization, including current-rights disclosure and revocation.
- Keep REST request encoding in che-rest and document the shared Tauri sync
  adapter, downstream Todo verification and five-crate package checks.
- Offload and bound password hashing work in authentication handlers.
- Print a shared startup configuration banner from Server::build(), with API URLs,
  installed apps, database pool, media and session settings; allow opting out with
  Server::startup_banner(false).
- Upgrade the ORM requirement to 0.3.2.
- Add single/modular layouts with a starter Task REST resource to cargo-generate
  and CLI startproject; keep first migration generation explicit.
- Generate shared APP_NAME constants and app-prefixed model tables; expose
  createapp as a compatible alias for startapp with automatic REST registration.
- Verify both generators, layouts and auth variants with migrations, cross-app
  foreign keys, generated API metadata and real HTTP CRUD.
- Dispatch durable lifecycle hooks and cascade cleanup for both ordinary and conditional SyncViewSet writes; preserve successful responses when effects need retry.
- Replay concurrent identical REST writes before applying stale UUID or revision checks.
- Preserve offline operations on non-sync HTTP errors, including expired authentication and throttling.
- Reject missing or ambiguous writable sync-to-serializer mappings during installation.
- Preserve UUID keys in admin edit forms, relation pickers and generated filter types.

## 0.3.0

- Upgrade to `che-orm 0.3.0`, using async database connections and the public migration API.
- Support integer and UUID REST primary keys and describe real, UUID and JSON fields in API metadata and generated clients.
- Use a single connection for configured in-memory SQLite databases in server and management commands.
- Generate numeric admin inputs for real fields and typed TypeScript real/UUID fields, including nullable columns.
- Update frontend dependencies and require patched Axios and Vue versions in generated admin projects.
- Add offline synchronization through ordinary REST ViewSet writes with transactional receipts and lifecycle effects.
- Keep new migrations inside each app and collect their histories through AppModule;
  preserve legacy explicit registries and add writable migration source paths.
- Verify follow-up generation and transferring an app unchanged between projects.
- Add a cargo-generate project template with optional authentication and local dependency paths.
- Update generated dependencies and declare Rust 1.88 as the minimum version.
- Fix startapp's empty compiled migration registry so a new project builds before its first migration.
- Correct initial migration commands and target each app's compiled history.
- Update generated agent guidance for async ORM access and role-aware synchronization.
- Add a downstream project build and migration smoke test.

## 0.2.4

- Raise the minimum supported Rust version to 1.88 to match the current dependencies.
- Add validated image storage, authenticated media delivery, and declarative image actions with generated multipart clients.
- Use conditional image replacement and retryable post-commit file cleanup; support async deletion hooks for cascaded records.

## 0.2.2

- Add request-scoped `CurrentPrincipal`, retaining the built-in `auth::User` as the framework
  authentication and authorization model.
- Add async application-user resolvers on `AppState`; applications can resolve and recover their
  own user model in permissions and `ViewSet` write preparation hooks.

## 0.2.0

- Initial crates.io release of the typed ORM2 REST framework.
- Add typed CRUD viewsets, filters, OpenAPI generation, TypeScript clients, and Vue admin generation.
- Add session and token authentication, CSRF protection, rolling session renewal, WebSocket signals,
  management commands, and SQLite migrations.
