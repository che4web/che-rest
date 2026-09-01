# Changelog

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
