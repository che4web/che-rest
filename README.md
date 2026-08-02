# che-rest

Base REST layer for `che-orm` applications.

## Management

Applications define one installed app list and use it for both the API server and management commands:

```rust
pub mod users;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new().add(users::module())
}
```

Server startup:

```rust
let app = Server::new(state)
    .install(apps::installed_apps())
    .build()
    .await?;
```

Installed app routes are served under `/api` by default. A viewset registered as `"/users"` is exposed as `/api/users`.

## Auth

`che-rest` includes an optional auth app. Install it to enable token authentication for all routes under `/api`:

```rust
pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(users::module())
}
```

Create the first superuser:

```bash
cargo run --bin manage -- migrate auth
```

Then create the first superuser:

```bash
cargo run --bin manage -- createsuperuser \
  --username admin \
  --password secret
```

Get a token with the Django REST Framework compatible endpoint:

```bash
curl -X POST http://127.0.0.1:3000/api-token-auth/ \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"secret"}'
```

Use the returned token for API requests:

```text
Authorization: Token <token>
```

Generated TypeScript clients expose `setAuthToken(token)` in `api_client.ts`.

Admin-only routers can use `che_rest::auth::admin_required_middleware`. It allows users with `is_admin` or `is_superuser`.

Project-local `src/bin/manage.rs`:

```rust
use che_rest::Management;
use simple_rest_demo::apps;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Management::new(apps::installed_apps()).run().await
}
```

Create a new app module in `src/apps/<name>`:

```bash
cargo run --bin manage -- startapp users
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

Then add the app to `apps::installed_apps()`:

```rust
InstalledApps::new().add(users::module())
```

Create app-scoped migrations from the installed app metadata:

```bash
cargo run --bin manage -- makemigrations users
```

Generate TypeScript models and API client from installed app metadata:

```bash
cargo run --bin manage -- generate-ts --out src/generated
```

This writes:

```text
src/generated/
  api_client.ts
  models.ts
  api.ts
```

Generate a standalone Vue admin project from installed app metadata:

```bash
cargo run --bin manage -- generate-admin --out frontend/admin
cd frontend/admin
npm install
npm run dev
```

This writes a Vite + Vue + Bootstrap project:

```text
frontend/admin/
  package.json
  index.html
  vite.config.ts
  tsconfig.json
  .env.example
  src/
    main.ts
    App.vue
    router.ts
    admin/
      AdminApp.vue
      AdminLogin.vue
      AdminModelList.vue
      AdminModelTable.vue
      AdminModelForm.vue
      adminRoutes.ts
      admin.css
      adminSchema.ts
      components/
        GenericModelTable.vue
        GenericModelForm.vue
      pages/
        UserList.vue
        UserForm.vue
      generated/
        adminSchema.ts
        adminRoutes.ts
    generated/
      api_client.ts
      models.ts
      api.ts
      useModelList.ts
      useModelItem.ts
```

The first run creates the full project and one editable list/form page pair for each model.
Later runs update generated files and create missing model pages, but leave existing Vue, CSS,
config, package, and model page files untouched so local admin customizations are preserved.
Use `--force` to overwrite static project files from templates:

```bash
cargo run --bin manage -- generate-admin --out frontend/admin --force
```

The generated admin login uses `/api-token-auth/` and stores the returned token in `localStorage`.
It requires `che_rest::auth::module()` to be installed. The generated Vite dev server proxies
`/api` and `/api-token-auth/` to `http://127.0.0.1:3000` by default.
Configure API URLs with Vite env variables for deployments or a different backend URL:

```env
VITE_API_BASE_URL=http://127.0.0.1:3000/api
VITE_AUTH_URL=http://127.0.0.1:3000/api-token-auth/
```

Override static admin templates by mirroring output paths in a templates directory:

```bash
cargo run --bin manage -- generate-admin \
  --out frontend/admin \
  --templates-dir admin_templates
```

Example overrides:

```text
admin_templates/
  src/admin/components/GenericModelTable.vue
  src/admin/admin.css
```

Dynamic files are always regenerated from installed app metadata and are not read from `--templates-dir`:
`src/admin/generated/adminSchema.ts`, `src/admin/generated/adminRoutes.ts`, the compatibility shim
`src/admin/adminSchema.ts`, and `src/generated/*`.

Per-model pages in `src/admin/pages` are generated as thin wrappers around the generic CRUD
components. Customize those page files for model-specific fields, actions, and layout; they are
not overwritten on later runs unless `--force` is passed.

Defaults:

```text
--name auto
```

Generated migrations are stored under:

```text
src/apps/users/migrations/
```

Apply migrations for one app. The database URL is read from `[database].url` in `app.toml`:

```bash
cargo run --bin manage -- migrate users
```

Apply migrations for all installed apps:

```bash
cargo run --bin manage -- migrate
```

You can override the config database URL:

```bash
cargo run --bin manage -- migrate users \
  --database-url sqlite://db.sqlite?mode=rwc
```
