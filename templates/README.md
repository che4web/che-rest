# Cargo project template

Install the tested cargo-generate version:

```bash
cargo install cargo-generate --version 0.23.7 --locked
```

Generate interactively from a local checkout. Prompts ask for the project name,
dependency paths, authentication and the `single`/`modular` layout:

```bash
cargo generate --path templates/project --no-workspace
```

To generate non-interactively (absolute dependency paths are recommended):

```bash
cargo generate --path /home/che/web/rust/che-rest/templates/project \
  --name my-project \
  --define che_rest_path=/home/che/web/rust/che-rest \
  --define che_orm_path=/home/che/web/rust/che-orm \
  --define with_auth=true --define layout=modular --silent --no-workspace
```

Once this template is committed and pushed, generation from Git is available:

```bash
cargo generate --git https://github.com/che4web/che-rest \
  templates/project
```

Use the branch containing the template. Empty dependency paths select crates.io;
use local paths until che-rest 0.3.1 and che-orm 0.3.2 are published. Authentication
is enabled by default; pass `--define with_auth=false` to omit it.

When using a local che-rest checkout, also select the same local che-orm checkout
used by that framework (normally its sibling `../che-orm`). Do not mix local REST
with a crates.io ORM: Cargo treats these as distinct crates even at the same
version, causing Model/ModelSerializer type errors before migrations can run.

Both layouts include a starter Task REST resource and an empty migration
registry. `single` keeps models, serializers, filters, views and migrations
in `src/`; `modular` (default) puts them in `src/apps/tasks/`.
`APP_NAME` is shared by `AppModule::name()` and `#[orm(app = super::APP_NAME)]`.
Task uses `tasks_task`. Generate and apply the first migration explicitly:

```bash
cd my-project
cargo run --bin manage -- makemigrations --name initial
cargo run --bin manage -- migrate
cargo run
```

Additional modules use `cargo run --bin manage -- createapp blog --model Post`
(`startapp` remains supported). They are automatically installed and receive
models, serializers, filters, views, `APP_NAME` and an empty migration registry.
For the built-in generator, use `che-rest startproject my-project --layout single`
or `--layout modular`; local dependencies are selected with `--che-rest-path`
and `--che-orm-path`, authentication with `--with-auth`.

The template retains startapp and compiled migration registry markers. Generated
models are ordinary CRUD models; the project guide documents sync integration.

New apps keep migrations under `src/apps/<app>/migrations/` and expose them through
AppModule. `makemigrations` selects these directories automatically; no shared
`src/migrations` registry or `--dir` is needed. Copy an app with its migrations and
register its module in the destination project's installed apps.

This framework build uses SQLite; PostgreSQL selection requires porting the
framework's SQLite-specific management operations first.

## Verification

```bash
TMPDIR=/tmp/opencode python3 scripts/check-template.py
```

The script checks both generators, both layouts, authentication on/off, initial
and repeated migrations, createapp, a cross-app FK, HTTP CRUD and OpenAPI.
Use `--generator cargo-generate` or `--generator cli` and `--layout single` or
`--layout modular` to narrow it. `--published` uses crates.io after publication.
