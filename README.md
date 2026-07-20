# che-rest

Base REST layer for `che-orm` applications.

## CLI

Create a new app module in `src/apps/<name>`:

```bash
che-rest startapp users
```

Generated files:

```text
src/apps/
  mod.rs
  users/
    mod.rs
    models.rs
    serializers.rs
    filters.rs
    views.rs
```

Then add `mod apps;` to your crate root and register the app module:

```rust
let app = Server::new(state)
    .register(apps::users::module())
    .build()
    .await?;
```

Create app-scoped migrations from a generated schema snapshot:

```bash
che-rest makemigrations users
```

Defaults:

```text
--schema che_orm_schema.json
--name auto
```

Generated migrations are stored under:

```text
src/apps/users/migrations/
```

Apply migrations for one app. The database URL is read from `[database].url` in `app.toml`:

```bash
che-rest migrate users \
  --config app.toml
```

Because `app.toml` is the default config path, this can be shortened to:

```bash
che-rest migrate users
```

You can override the config database URL:

```bash
che-rest migrate users \
  --database-url sqlite://db.sqlite?mode=rwc
```
