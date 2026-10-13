use std::{
    fs,
    path::{Path, PathBuf},
};

type ProjectResult<T> = Result<T, Box<dyn std::error::Error>>;

const APP_MODULES_MARKER: &str = "// che-rest:startapp modules";
const APP_INSTALL_MARKER: &str = "// che-rest:startapp installed apps";

#[derive(Debug, Clone)]
pub struct StartProjectOptions {
    pub name: String,
    pub out: PathBuf,
    pub che_rest_path: String,
    pub che_orm_path: String,
    pub with_auth: bool,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct StartAppOptions {
    pub name: String,
    pub models: Vec<String>,
    pub root: PathBuf,
    pub force: bool,
}

pub fn startproject(options: StartProjectOptions) -> ProjectResult<()> {
    validate_project_name(&options.name)?;

    let project_dir = options.out.join(&options.name);
    prepare_project_dir(&project_dir, options.force)?;

    let crate_name = options.name.replace('-', "_");

    write_file(
        &project_dir.join("Cargo.toml"),
        &cargo_toml_template(&options.name, &options.che_rest_path, &options.che_orm_path),
    )?;
    write_file(&project_dir.join("app.toml"), app_toml_template())?;
    write_file(
        &project_dir.join("AGENTS.md"),
        &agents_md_template(options.with_auth),
    )?;
    write_file(&project_dir.join("src/lib.rs"), lib_rs_template())?;
    write_file(
        &project_dir.join("src/main.rs"),
        &main_rs_template(&crate_name),
    )?;
    write_file(
        &project_dir.join("src/bin/manage.rs"),
        &manage_rs_template(&crate_name),
    )?;
    write_file(
        &project_dir.join("src/apps/mod.rs"),
        &apps_mod_template(options.with_auth),
    )?;

    println!("Created project {}", project_dir.display());
    println!("Next steps:");
    println!("  cd {}", project_dir.display());
    println!("  cargo run --bin manage -- startapp tasks --model Task");
    println!("  cargo run --bin manage -- schema");
    println!("  cargo run --bin manage -- makemigrations --name initial");
    println!("  cargo run --bin manage -- migrate");
    println!("  cargo run");

    Ok(())
}

pub fn startapp(options: StartAppOptions) -> ProjectResult<()> {
    validate_app_name(&options.name)?;
    let models = if options.models.is_empty() {
        vec![default_model_name(&options.name)]
    } else {
        options.models
    };
    for model in &models {
        validate_model_name(model)?;
    }

    let app_dir = options.root.join("src/apps").join(&options.name);
    prepare_app_dir(&app_dir, options.force)?;

    write_file(
        &app_dir.join("models.rs"),
        &models_rs_template(&options.name, &models),
    )?;
    write_file(
        &app_dir.join("serializers.rs"),
        &serializers_rs_template(&models),
    )?;
    write_file(&app_dir.join("filters.rs"), &filters_rs_template(&models))?;
    write_file(&app_dir.join("views.rs"), &views_rs_template(&models))?;
    write_file(
        &app_dir.join("mod.rs"),
        &app_mod_template(&options.name, &models),
    )?;
    register_app(&options.root.join("src/apps/mod.rs"), &options.name)?;
    write_file(
        &app_dir.join("migrations/mod.rs"),
        "// che-orm: migrations begin\npub fn all() -> Vec<che_orm::Migration> { vec![] }\n// che-orm: migrations end\n",
    )?;

    println!("Created app {}", app_dir.display());
    println!("Next steps:");
    println!("  cargo run --bin manage -- makemigrations");
    println!("  cargo run --bin manage -- migrate");
    Ok(())
}

fn prepare_app_dir(app_dir: &Path, force: bool) -> ProjectResult<()> {
    if !app_dir.exists() {
        fs::create_dir_all(app_dir)?;
        return Ok(());
    }

    if !app_dir.is_dir() {
        return Err(format!("app path is not a directory: {}", app_dir.display()).into());
    }

    if force {
        return Ok(());
    }

    if fs::read_dir(app_dir)?.next().is_some() {
        return Err(format!(
            "app directory already exists and is not empty: {}",
            app_dir.display()
        )
        .into());
    }

    Ok(())
}

fn prepare_project_dir(project_dir: &Path, force: bool) -> ProjectResult<()> {
    if !project_dir.exists() {
        fs::create_dir_all(project_dir)?;
        return Ok(());
    }

    if !project_dir.is_dir() {
        return Err(format!("project path is not a directory: {}", project_dir.display()).into());
    }

    if force {
        return Ok(());
    }

    if fs::read_dir(project_dir)?.next().is_some() {
        return Err(format!(
            "project directory already exists and is not empty: {}",
            project_dir.display()
        )
        .into());
    }

    Ok(())
}

fn write_file(path: &Path, content: &str) -> ProjectResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn validate_project_name(name: &str) -> ProjectResult<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("project name cannot be empty".into());
    };

    if !first.is_ascii_lowercase() {
        return Err("project name must start with a lowercase ascii letter".into());
    }

    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-') {
        return Err(
            "project name must contain only lowercase ascii letters, digits, underscores, and hyphens"
                .into(),
        );
    }

    Ok(())
}

fn validate_app_name(name: &str) -> ProjectResult<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("app name cannot be empty".into());
    };

    if !first.is_ascii_lowercase() {
        return Err("app name must start with a lowercase ascii letter".into());
    }

    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
        return Err(
            "app name must contain only lowercase ascii letters, digits, and underscores".into(),
        );
    }

    Ok(())
}

fn validate_model_name(name: &str) -> ProjectResult<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("model name cannot be empty".into());
    };

    if !first.is_ascii_uppercase() {
        return Err("model name must start with an uppercase ascii letter".into());
    }

    if !chars.all(|ch| ch.is_ascii_alphanumeric()) {
        return Err("model name must contain only ascii letters and digits".into());
    }

    Ok(())
}

fn cargo_toml_template(name: &str, che_rest_path: &str, che_orm_path: &str) -> String {
    let che_orm = dependency("che-orm", "0.3.0", che_orm_path);
    let che_rest = dependency("che-rest", env!("CARGO_PKG_VERSION"), che_rest_path);
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.88"
default-run = "{name}"

[dependencies]
axum = "0.8"
{che_orm}
{che_rest}
serde = {{ version = "1", features = ["derive"] }}
time = "0.3"
tokio = {{ version = "1", features = ["macros", "net", "rt-multi-thread", "sync"] }}
"#
    )
}

fn dependency(name: &str, version: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{name} = \"{version}\"")
    } else {
        format!("{name} = {{ version = \"{version}\", path = \"{path}\" }}")
    }
}

fn app_toml_template() -> &'static str {
    r#"[database]
url = "sqlite://db.sqlite?mode=rwc"
max_connections = 64

[server]
host = "127.0.0.1"
port = 3000
api_prefix = "/api"
"#
}

fn agents_md_template(with_auth: bool) -> String {
    let auth_note = if with_auth {
        "- `che_rest::auth::module()` is installed; session login is available at `/api-session-auth/login/`.\n"
    } else {
        "- Authentication is not installed; add `che_rest::auth::module()` before using protected routes.\n"
    };

    format!(
        r#"# Project Guide

This is a `che-rest` application. Follow the workflow below instead of creating database tables in
application startup.

## Canonical Workflow

```bash
cargo run --bin manage -- schema
cargo run --bin manage -- makemigrations --name initial
cargo run --bin manage -- migrate
cargo run
```

`makemigrations` compares installed schemas with the compiled migration registry and writes Rust
migration modules under `src/apps/<app>/migrations/`. Apps expose their compiled
history through `AppModule::migrations()` and their source directory through
`migration_path()`. `migrate` collects installed app histories through the built-in
forward-only SQLite executor used by this che-rest build. The ORM also supports PostgreSQL
in separately configured builds; changing only the database URL does not select a backend.
`migrate` is the deployment command that creates or changes database tables;
`Server::build()` does not alter the schema.

## App Structure

- `src/apps/mod.rs`: installed app registry used by both the server and management commands.
- `src/apps/<app>/models.rs`: `che-orm` models and database fields.
- `src/apps/<app>/serializers.rs`: generated ORM2 input/output DTOs.
- `src/apps/<app>/filters.rs`: list query filters.
- `src/apps/<app>/views.rs`: typed CRUD viewsets and permissions.
- `src/bin/manage.rs`: management command entrypoint.
- `src/apps/<app>/migrations/`: portable application-owned compiled history.

Register CRUD with `ctx.viewset_with(views::UserViewSet)`. This registers the model
schema, API metadata, and router together. Do not use only `ctx.route(...)` for a model API.

## Conventions

- Assign server-owned fields in `ViewSet::prepare_create()` or a custom serializer validation flow.
- Mark server-owned serializer fields read-only so clients cannot supply them.
- Database operations are async. Use awaited methods and transaction callbacks
  `|tx| Box::pin(async move {{ ... }})`.
- Keep serializers database-independent; load relations in the ViewSet queryset.
- Preserve applied migration identities and checksums. Review generated operations
  and use explicit data migrations when existing values need conversion.
- Use `IsAuthenticated` for resources that require the current user.
- Application events use `AppState::app_channels()` and `AppModule::subscribe()`.
- `AppState::app_channels()` is for internal application events and is never a public WebSocket channel.
- Public WebSocket signals use `AppState::signals()` and must be declared with `ModuleContext::signal(...)`; CRUD lifecycle signals are opt-in via `ViewSet::signal_access` and publish `{{ "id": ... }}` payloads.
- Generated admin uses session cookies and the readable `csrf_token` cookie.
{auth_note}
## Synchronization

Generated apps use ordinary CRUD models by default. For local-first models, use
UUID keys and `#[orm(sync, sync_name = "app.model")]` with shared business fields.
Register server models with `SchemaSet::server_model` and client models with
`SchemaSet::client_model`; only clients create shadow tables. Keep stable wire
names with field-level `#[orm(sync_name = "stable_name")]` when renaming fields.

Implement `SyncViewSet` and install `SyncEndpoints::new(...)` in `ViewSet::configure`.
Provide a registry containing the model and its dependencies, select scope from
trusted context, and enforce access with `AccessPolicy`. Sync endpoints require
authentication. A constant scope with `AllowAll` represents a shared partition,
not per-user isolation. Ordinary POST/PATCH/PUT/DELETE handle sync delivery;
there is no separate sync/push route.

Apply compiled migrations for sync infrastructure before serving requests. Writes
outside the ViewSet adapter must use the configured `SyncServer::database()` or
`SyncClient::database()` handle, or an explicit sync facade; raw SQL and original
unconfigured handles bypass capture. Configured server reads authorize scope and
filter scope/tombstones, including SELECT ASTs and transaction reads.

Clients call `pull()` to download, `flush()` to deliver and `full_load()` explicitly
for initial loading or expired cursors. Pending operations retry with their original
IDs. `sync_once()` is a compatibility cycle that can load snapshots automatically.
Clients target one persistent server history; database replacement/history rewind
is unsupported. Preserve receipts, tombstones and record revisions when pruning
journals. Clone one SyncClient to share locks for a client database.

## Verification

After changing a model, run:

```bash
cargo run --bin manage -- makemigrations
cargo run --bin manage -- migrate
cargo check
```

When adding or changing an endpoint, also run `cargo run --bin manage -- generate-ts` and inspect the
generated OpenAPI at `/api/openapi.json` while the server is running.

OpenAPI metadata is available from the running server at `/api/openapi.json` by default.
"#
    )
}

fn lib_rs_template() -> &'static str {
    "pub mod apps;\n"
}

fn main_rs_template(crate_name: &str) -> String {
    format!(
        r#"use che_rest::{{AppState, Server}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let state = AppState::from_config_file("app.toml").await?;
    let server_config = state.config.server.clone();
    let app = Server::new(state)
        .install({crate_name}::apps::installed_apps())
        .api_prefix(&server_config.api_prefix)
        .build()
        .await?;

    let address = format!("{{}}:{{}}", server_config.host, server_config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}}
"#
    )
}

fn manage_rs_template(crate_name: &str) -> String {
    format!(
        r#"use che_rest::Management;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    Management::new({crate_name}::apps::installed_apps())
        .project_root(env!("CARGO_MANIFEST_DIR"))
        .run()
        .await
}}
"#
    )
}

fn apps_mod_template(with_auth: bool) -> String {
    let auth_module = if with_auth {
        "\n        .add(che_rest::auth::module())"
    } else {
        ""
    };

    format!(
        r#"{APP_MODULES_MARKER}

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {{
    InstalledApps::new(){auth_module}
        {APP_INSTALL_MARKER}
}}
"#
    )
}

fn register_app(apps_mod_path: &Path, app_name: &str) -> ProjectResult<()> {
    let content = fs::read_to_string(apps_mod_path).map_err(|error| {
        format!(
            "could not read {}; create it or register `pub mod {app_name};` manually: {error}",
            apps_mod_path.display()
        )
    })?;

    if !content.contains(APP_MODULES_MARKER) || !content.contains(APP_INSTALL_MARKER) {
        return Err(format!(
            "created app files, but {} has no che-rest startapp markers; add `pub mod {app_name};` and `.add({app_name}::module())` manually",
            apps_mod_path.display()
        )
        .into());
    }

    let module_line = format!("pub mod {app_name};");
    let install_line = format!("        .add({app_name}::module())");
    let with_module = if content.contains(&module_line) {
        content
    } else {
        content.replace(
            APP_MODULES_MARKER,
            &format!("{APP_MODULES_MARKER}\n{module_line}"),
        )
    };
    let with_install = if with_module.contains(&install_line) {
        with_module
    } else {
        with_module.replace(
            APP_INSTALL_MARKER,
            &format!("{install_line}\n        {APP_INSTALL_MARKER}"),
        )
    };

    fs::write(apps_mod_path, with_install)?;
    Ok(())
}

fn default_model_name(app_name: &str) -> String {
    let base = app_name.strip_suffix("app").unwrap_or(app_name);
    singular_model_name(base)
}

fn singular_model_name(name: &str) -> String {
    let singular = name.strip_suffix('s').unwrap_or(name);
    to_pascal_case(singular)
}

fn to_pascal_case(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn to_snake_case(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

fn model_blocks(app_name: &str, models: &[String]) -> String {
    models
        .iter()
        .map(|model| {
            let table = format!("{app_name}_{}", to_snake_case(model));
            format!(
                r#"#[derive(Debug, che_orm::Model)]
#[orm(table = "{table}")]
pub struct {model} {{
    #[orm(primary_key)]
    pub id: i64,
    pub name: String,
    #[orm(auto_now_add)]
    pub created_at: OffsetDateTime,
    #[orm(auto_now)]
    pub updated_at: OffsetDateTime,
}}
"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn models_rs_template(app_name: &str, models: &[String]) -> String {
    format!(
        "use time::OffsetDateTime;\n\n{}",
        model_blocks(app_name, models)
    )
}

fn serializers_rs_template(models: &[String]) -> String {
    let imports = models.join(", ");
    let blocks = models
        .iter()
        .map(|model| {
            format!(
                r#"#[derive(che_orm::ModelSerializer)]
#[serializer(model = {model})]
pub struct {model}Serializer {{
    #[serializer(read_only)]
    pub id: i64,
    pub name: String,
    #[serializer(read_only)]
    pub created_at: time::OffsetDateTime,
    #[serializer(read_only)]
    pub updated_at: time::OffsetDateTime,
}}
"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("use super::models::{{{imports}}};\n\n{blocks}")
}

fn filters_rs_template(models: &[String]) -> String {
    let imports = models.join(", ");
    let blocks = models
        .iter()
        .map(|model| {
            format!(
                r#"#[derive(Clone, Copy, Default)]
pub struct {model}FilterSet;

static {upper}_FILTERS: &[Filter<{model}>] = &[
    Filter::exact({model}::ID),
    Filter::contains({model}::NAME),
    Filter::exact({model}::CREATED_AT),
    Filter::exact({model}::UPDATED_AT),
];

impl FilterSetSpec for {model}FilterSet {{
    type Model = {model};

    fn filters(&self) -> &'static [Filter<{model}>] {{
        {upper}_FILTERS
    }}
}}
"#,
                upper = to_snake_case(model).to_ascii_uppercase()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "use che_rest::{{Filter, FilterSetSpec}};\n\nuse super::models::{{{imports}}};\n\n{blocks}"
    )
}

fn views_rs_template(models: &[String]) -> String {
    let model_imports = models.join(", ");
    let filter_imports = models
        .iter()
        .map(|model| format!("{model}FilterSet"))
        .collect::<Vec<_>>()
        .join(", ");
    let serializer_imports = models
        .iter()
        .map(|model| format!("{model}Serializer"))
        .collect::<Vec<_>>()
        .join(", ");
    let blocks = models
        .iter()
        .map(|model| {
            let path = to_snake_case(model);
            format!(
                r#"#[derive(Clone, Copy, Default)]
pub struct {model}ViewSet;

impl ViewSet for {model}ViewSet {{
    type Model = {model};
    type Serializer = {model}Serializer;
    type ListSerializer = Self::Serializer;
    type QuerySet = che_orm::DatabaseQuery<{model}>;
    type FilterSet = {model}FilterSet;
    type Permission = AllowAny;

    fn get_queryset(&self) -> Self::QuerySet {{
        che_orm::DatabaseQuery::new({model}::query())
    }}

    fn path(&self) -> &'static str {{
        "/{path}"
    }}
}}
"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "use che_rest::{{AllowAny, Model, ViewSet}};\n\nuse super::{{\n    filters::{{{filter_imports}}},\n    models::{{{model_imports}}},\n    serializers::{{{serializer_imports}}},\n}};\n\n{blocks}"
    )
}

fn app_mod_template(app_name: &str, models: &[String]) -> String {
    let module_type = format!("{}Module", to_pascal_case(app_name));
    let schema_models = models
        .iter()
        .map(|model| format!(".model::<models::{model}>()"))
        .collect::<String>();
    let viewsets = models
        .iter()
        .map(|model| format!("        context.viewset_with(views::{model}ViewSet);"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"pub mod filters;
pub mod models;
pub mod migrations;
pub mod serializers;
pub mod views;

use che_rest::{{AppModule, ModuleContext}};

pub fn module() -> {module_type} {{
    {module_type}
}}

pub struct {module_type};

impl AppModule for {module_type} {{
    fn migrations(&self) -> Vec<che_orm::Migration> {{
        migrations::all()
    }}

    fn migration_path(&self) -> Option<&'static str> {{
        Some(concat!(env!("CARGO_MANIFEST_DIR"), "/src/apps/{app_name}/migrations"))
    }}
    fn name(&self) -> &'static str {{
        "{app_name}"
    }}

    fn schema(&self) -> che_orm::SchemaSet {{
        che_orm::SchemaSet::new(){schema_models}
    }}

    fn init(&self, context: &mut ModuleContext) {{
{viewsets}
    }}
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{StartAppOptions, StartProjectOptions, startapp, startproject};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("che_rest_{name}_{suffix}"))
    }

    #[test]
    fn startproject_creates_manage_project() {
        let out = temp_dir("startproject");
        startproject(StartProjectOptions {
            name: "todo_api".into(),
            out: out.clone(),
            che_rest_path: "../che-rest".into(),
            che_orm_path: "../che-orm".into(),
            with_auth: true,
            force: false,
        })
        .unwrap();

        let project = out.join("todo_api");
        assert!(project.join("src/bin/manage.rs").exists());
        assert!(!project.join("src/migrations").exists());
        assert!(
            std::fs::read_to_string(project.join("src/bin/manage.rs"))
                .unwrap()
                .contains(".project_root(env!(\"CARGO_MANIFEST_DIR\"))")
        );
        let apps_mod = std::fs::read_to_string(project.join("src/apps/mod.rs")).unwrap();
        assert!(apps_mod.contains("che_rest::auth::module()"));
        assert!(apps_mod.contains("che-rest:startapp modules"));
        assert!(apps_mod.contains("che-rest:startapp installed apps"));
        let manifest: toml::Value =
            toml::from_str(&std::fs::read_to_string(project.join("Cargo.toml")).unwrap()).unwrap();
        let framework: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
        assert_eq!(
            manifest["dependencies"]["che-rest"]["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            manifest["dependencies"]["che-orm"]["version"],
            framework["dependencies"]["che-orm"]["version"]
        );
        assert_eq!(manifest["package"]["rust-version"].as_str(), Some("1.88"));
        let guide = std::fs::read_to_string(project.join("AGENTS.md")).unwrap();
        assert!(guide.contains("SyncEndpoints::new"));
        assert!(guide.contains("database replacement/history rewind"));

        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn startapp_creates_files_and_registers_module() {
        let out = temp_dir("startapp");
        startproject(StartProjectOptions {
            name: "todo_api".into(),
            out: out.clone(),
            che_rest_path: "../che-rest".into(),
            che_orm_path: "../che-orm".into(),
            with_auth: false,
            force: false,
        })
        .unwrap();
        let project = out.join("todo_api");

        startapp(StartAppOptions {
            name: "tasks".into(),
            models: vec!["Task".into(), "Comment".into()],
            root: project.clone(),
            force: false,
        })
        .unwrap();

        assert!(project.join("src/apps/tasks/models.rs").exists());
        let apps_mod = std::fs::read_to_string(project.join("src/apps/mod.rs")).unwrap();
        assert!(apps_mod.contains("pub mod tasks;"));
        assert!(apps_mod.contains(".add(tasks::module())"));
        let app_mod = std::fs::read_to_string(project.join("src/apps/tasks/mod.rs")).unwrap();
        assert!(app_mod.contains("pub mod migrations;"));
        assert!(app_mod.contains("fn migration_path"));
        assert!(project.join("src/apps/tasks/migrations/mod.rs").exists());
        let models = std::fs::read_to_string(project.join("src/apps/tasks/models.rs")).unwrap();
        assert!(models.contains("pub struct Task"));
        assert!(models.contains("pub struct Comment"));

        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn startapp_defaults_model_from_app_name() {
        let out = temp_dir("startapp_default");
        startproject(StartProjectOptions {
            name: "todo_api".into(),
            out: out.clone(),
            che_rest_path: "../che-rest".into(),
            che_orm_path: "../che-orm".into(),
            with_auth: false,
            force: false,
        })
        .unwrap();
        let project = out.join("todo_api");

        startapp(StartAppOptions {
            name: "taskapp".into(),
            models: vec![],
            root: project.clone(),
            force: false,
        })
        .unwrap();

        let models = std::fs::read_to_string(project.join("src/apps/taskapp/models.rs")).unwrap();
        assert!(models.contains("pub struct Task"));

        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    #[ignore = "builds a generated downstream project and executes its migration CLI"]
    fn generated_project_builds_and_migrates() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let out = std::env::temp_dir()
            .join("opencode")
            .join(format!("che_rest_template_{suffix}"));
        let framework = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        startproject(StartProjectOptions {
            name: "template_smoke".into(),
            out: out.clone(),
            che_rest_path: framework.display().to_string(),
            che_orm_path: framework
                .join("../che-orm")
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            with_auth: true,
            force: false,
        })
        .unwrap();
        let project = out.join("template_smoke");
        startapp(StartAppOptions {
            name: "tasks".into(),
            models: vec!["Task".into(), "Comment".into()],
            root: project.clone(),
            force: false,
        })
        .unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new(env!("CARGO"))
                .args(args)
                .current_dir(&project)
                .env("CARGO_TARGET_DIR", framework.join("target"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "cargo {args:?} failed in {}\n{}\n{}",
                project.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(&["check", "--all-targets"]);
        let schema = run(&["run", "--bin", "manage", "--", "schema"]);
        assert!(schema.contains("tasks_task"));
        run(&[
            "run",
            "--bin",
            "manage",
            "--",
            "makemigrations",
            "--name",
            "initial",
        ]);
        assert!(
            project
                .join("src/apps/tasks/migrations/m0001_initial.rs")
                .exists()
        );
        run(&["run", "--bin", "manage", "--", "migrate"]);
        run(&["run", "--bin", "manage", "--", "migrate"]);
        let status = run(&["run", "--bin", "manage", "--", "migrate", "status"]);
        assert!(status.contains("tasks"), "{status}");
        run(&["run", "--bin", "manage", "--", "makemigrations", "--check"]);
        let models_path = project.join("src/apps/tasks/models.rs");
        let models = std::fs::read_to_string(&models_path).unwrap();
        std::fs::write(
            &models_path,
            models.replace(
                "pub name: String,",
                "pub name: String,\n    pub detail: Option<String>,",
            ),
        )
        .unwrap();
        run(&[
            "run",
            "--bin",
            "manage",
            "--",
            "makemigrations",
            "--name",
            "detail",
        ]);
        assert!(
            project
                .join("src/apps/tasks/migrations/m0002_detail.rs")
                .exists()
        );
        run(&["run", "--bin", "manage", "--", "migrate"]);
        run(&[
            "run",
            "--bin",
            "manage",
            "--",
            "makemigrations",
            "tasks",
            "--check",
        ]);

        // Copy the complete module unchanged into an independently named project.
        // Its env!(CARGO_MANIFEST_DIR) source path is recompiled at the destination.
        let destination = out.join("transferred_app");
        startproject(StartProjectOptions {
            name: "transferred_app".into(),
            out: out.clone(),
            che_rest_path: framework.display().to_string(),
            che_orm_path: framework
                .join("../che-orm")
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            with_auth: true,
            force: false,
        })
        .unwrap();
        fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_tree(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        copy_tree(
            &project.join("src/apps/tasks"),
            &destination.join("src/apps/tasks"),
        );
        super::register_app(&destination.join("src/apps/mod.rs"), "tasks").unwrap();
        let output = std::process::Command::new(env!("CARGO"))
            .args(["run", "--bin", "manage", "--", "migrate"])
            .current_dir(&destination)
            .env("CARGO_TARGET_DIR", framework.join("target"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = std::process::Command::new(env!("CARGO"))
            .args(["run", "--bin", "manage", "--", "makemigrations", "--check"])
            .current_dir(&destination)
            .env("CARGO_TARGET_DIR", framework.join("target"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(out).unwrap();
    }
}
