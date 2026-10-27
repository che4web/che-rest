# {{project-name}}

REST application with the **{{layout}}** layout and a starter Task resource.
Requires Rust 1.88+, che-rest 0.3.1 and che-orm 0.3.2. Review the starter model,
generate the first migration yourself, then apply it:

```bash
cargo run --bin manage -- schema
cargo run --bin manage -- makemigrations --name initial
cargo run --bin manage -- migrate
cargo run
```

Configuration lives in `app.toml`. The server listens at `http://127.0.0.1:3000`
with API prefix `/api`; OpenAPI is at `/api/openapi.json`.

{% if layout == "single" -%}
The starter app's models, serializers, filters, views and empty `migrations/`
registry live directly in `src/`. `APP_NAME` and `TasksModule` are in `src/lib.rs`.
{% else -%}
The starter app lives in `src/apps/tasks/`, including its empty `migrations/`
registry, models, serializers, filters and views. Its `mod.rs` defines
`APP_NAME` and `TasksModule`.
{% endif %}
Models use `#[orm(app = super::APP_NAME)]`. The starter table is `tasks_task`;
`AppModule::name()` returns the same constant. Explicit `table` remains an override.

## Create an app

```bash
cargo run --bin manage -- createapp blog --model Post
cargo run --bin manage -- makemigrations --name add_blog
cargo run --bin manage -- migrate
```

`createapp` is an alias for `startapp`. Both work with either layout and create
`src/apps/blog/`, define `APP_NAME = "blog"`, generate Post with table `blog_post`
and register its REST module in `src/apps/mod.rs`. Each new app owns its compiled
history in `src/apps/<app>/migrations/`; installing it registers the history.
The default model is derived from the app name; use repeated `--model` options
to specify models. Register FK parents before children. Preserve generator
markers and all applied migration identities/checksums.

The Task API is at `/api/task/`, OpenAPI at `/api/openapi.json`.
This build uses SQLite. Startup does not apply or generate migrations.
{% if with_auth %}Authentication is installed; create a superuser with:

```bash
cargo run --bin manage -- createsuperuser --username admin --password YOUR_PASSWORD
```
{% else %}Authentication is not installed.
{% endif %}
## Verify

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Applications use ordinary CRUD models by default. Local-first synchronization
requires shared UUID models with `#[orm(sync)]`, server registration and a
`SyncViewSet` with `SyncEndpoints` installed in its configuration.
