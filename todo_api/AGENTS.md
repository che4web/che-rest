# Project Guide

This is a `che-rest` application. Follow the workflow below instead of creating database tables in
application startup.

## Canonical Workflow

```bash
cargo run --bin manage -- startapp users
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
cargo run
```

`makemigrations` without an app name processes every installed app. Use
`cargo run --bin manage -- makemigrations users` for one app. `migrate` is the only command that
creates or changes database tables; `Server::build()` does not alter the schema.

## App Structure

- `src/apps/mod.rs`: installed app registry used by both the server and management commands.
- `src/apps/<app>/models.rs`: `che-orm` models and database fields.
- `src/apps/<app>/serializers.rs`: API input/output fields plus typed `create` and `update` persistence.
- `src/apps/<app>/filters.rs`: list query filters.
- `src/apps/<app>/views.rs`: typed CRUD viewsets and permissions.
- `src/apps/<app>/migrations/`: generated SQL migrations and schema snapshot.
- `src/bin/manage.rs`: management command entrypoint.

Register CRUD with `ctx.viewset_with("/users", views::UserViewSet)`. This registers the model
schema, API metadata, and router together. Do not use only `ctx.route(...)` for a model API.

## Conventions

- Assign server-owned fields such as `author_id` in `ViewSet::system_create_values()`.
- Mark server-owned serializer fields with `Field::system()` so clients cannot supply them.
- Serializer `create` and `update` receive `ValidatedData`; extract values with `ModelFields` and write with typed ORM `.set` calls.
- Use `IsAuthenticated` for resources that require the current user.
- WebSocket commands are registered with `ctx.command_handler(...)` and receive `Command.user`.
- `AppState::app_channels()` is for internal application events and is never a public WebSocket channel.
- ORM model signals are separate from application channels; bridge them explicitly in `AppModule::subscribe()`.
- Generated admin uses session cookies and the readable `csrf_token` cookie.
- Authentication is not installed; add `che_rest::auth::module()` before using protected routes.

## Verification

After changing a model, run:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
cargo check
```

When adding or changing an endpoint, also run `cargo run --bin manage -- generate-ts` and inspect the
generated OpenAPI at `/api/openapi.json` while the server is running.

For machine-readable metadata, run:

```bash
cargo run --bin manage -- inspect --format json
```

The migration and code generation commands also support `--format json` for agent workflows.

```bash
cargo run --bin manage -- makemigrations --format json
cargo run --bin manage -- migrate --format json
```
