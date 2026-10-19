# Changelog

## 0.3.0

- Upgrade to `che-orm 0.3.0`, using async database connections and the public migration API.
- Keep REST and image actions on integer primary keys; describe real and UUID columns in OpenAPI.
- Use a single connection for configured in-memory SQLite databases in server and management commands.
- Generate numeric admin inputs for real fields and typed TypeScript real/UUID fields, including nullable columns.
- Update frontend dependencies and require patched Axios and Vue versions in generated admin projects.

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
