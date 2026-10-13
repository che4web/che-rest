# Changelog

## 0.2.5

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
