# Project Guide

This example uses the ORM2 module and typed CRUD APIs.

```bash
cargo run
```

The application registers `Tasks` through `InstalledApps`, creates the local
example table, and serves CRUD plus OpenAPI routes under `/api`.

Important files:

- `src/apps/mod.rs`: installed app registry.
- `src/apps/tasks/models.rs`: ORM2 `Task` model.
- `src/apps/tasks/serializers.rs`: generated response and write serializer.
- `src/apps/tasks/filters.rs`: typed task filters.
- `src/apps/tasks/views.rs`: task ViewSet.
- `src/apps/tasks/mod.rs`: module schema and route registration.
- `src/main.rs`: config, server construction and listener.
- `app.toml`: SQLite path and server settings.

The example intentionally does not include authentication or management
commands yet; those belong to later migration phases.
