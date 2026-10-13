# {{project-name}}

Create an application, review its generated models and apply compiled migrations:

```bash
cargo run --bin manage -- startapp tasks --model Task --model Comment
cargo run --bin manage -- schema
cargo run --bin manage -- makemigrations --name initial
cargo run --bin manage -- migrate
cargo run
```

Configuration lives in `app.toml`. The server listens at `http://127.0.0.1:3000`
with API prefix `/api`; OpenAPI is at `/api/openapi.json`.

Each app owns its compiled history in `src/apps/<app>/migrations/`. Installing
the app registers its migrations automatically, so apps can be copied between projects.

Applications use ordinary CRUD models by default. Local-first synchronization
requires shared UUID models with `#[orm(sync)]`, server registration and a
`SyncViewSet` with `SyncEndpoints` installed in its configuration.
