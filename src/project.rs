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
    pub che_orm2_path: String,
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
        &cargo_toml_template(
            &options.name,
            &options.che_rest_path,
            &options.che_orm2_path,
        ),
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
    println!("  cargo run --bin manage -- makemigrations initial");
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

fn cargo_toml_template(name: &str, che_rest_path: &str, che_orm2_path: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
default-run = "{name}"

[dependencies]
axum = "0.8"
che-orm2 = {{ path = "{che_orm2_path}" }}
che-rest = {{ path = "{che_rest_path}" }}
serde = {{ version = "1", features = ["derive"] }}
time = "0.3"
tokio = {{ version = "1", features = ["macros", "net", "rt-multi-thread", "sync"] }}
"#
    )
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
cargo run --bin manage -- makemigrations initial
cargo run --bin manage -- migrate
cargo run
```

`makemigrations` compares all schemas from installed modules with the Atlas migration directory and
requires Atlas. `migrate` applies checked-in SQL migrations through the built-in runner and does not
require Atlas. It is the only command that creates or changes database tables; `Server::build()` does
not alter the schema.

## App Structure

- `src/apps/mod.rs`: installed app registry used by both the server and management commands.
- `src/apps/<app>/models.rs`: `che-orm2` models and database fields.
- `src/apps/<app>/serializers.rs`: generated ORM2 input/output DTOs.
- `src/apps/<app>/filters.rs`: list query filters.
- `src/apps/<app>/views.rs`: typed CRUD viewsets and permissions.
- `src/bin/manage.rs`: management command entrypoint.

Register CRUD with `ctx.viewset_with("/users", views::UserViewSet)`. This registers the model
schema, API metadata, and router together. Do not use only `ctx.route(...)` for a model API.

## Conventions

- Assign server-owned fields in `ViewSet::prepare_create()` or a custom serializer validation flow.
- Mark server-owned serializer fields read-only so clients cannot supply them.
- Use `IsAuthenticated` for resources that require the current user.
- Application events use `AppState::app_channels()` and `AppModule::subscribe()`.
- `AppState::app_channels()` is for internal application events and is never a public WebSocket channel.
- Public WebSocket signals use `AppState::signals()` and must be declared with `ModuleContext::signal(...)`; CRUD viewsets declare authenticated lifecycle signals automatically and publish `{{ "id": ... }}` payloads.
- Generated admin uses session cookies and the readable `csrf_token` cookie.
{auth_note}
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
                r#"#[derive(Debug, che_orm2::Model)]
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
                r#"#[derive(che_orm2::ModelSerializer)]
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
    type QuerySet = che_orm2::DatabaseQuery<{model}>;
    type FilterSet = {model}FilterSet;
    type Permission = AllowAny;

    fn get_queryset(&self) -> Self::QuerySet {{
        che_orm2::DatabaseQuery::new({model}::query())
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
        .map(|model| {
            let path = to_snake_case(model);
            format!("        context.viewset_with(\"/{path}\", views::{model}ViewSet);")
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"pub mod filters;
pub mod models;
pub mod serializers;
pub mod views;

use che_rest::{{AppModule, ModuleContext}};

pub fn module() -> {module_type} {{
    {module_type}
}}

pub struct {module_type};

impl AppModule for {module_type} {{
    fn name(&self) -> &'static str {{
        "{app_name}"
    }}

    fn schema(&self) -> che_orm2::SchemaSet {{
        che_orm2::SchemaSet::new(){schema_models}
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
            che_orm2_path: "../che-orm2".into(),
            with_auth: true,
            force: false,
        })
        .unwrap();

        let project = out.join("todo_api");
        assert!(project.join("src/bin/manage.rs").exists());
        let apps_mod = std::fs::read_to_string(project.join("src/apps/mod.rs")).unwrap();
        assert!(apps_mod.contains("che_rest::auth::module()"));
        assert!(apps_mod.contains("che-rest:startapp modules"));
        assert!(apps_mod.contains("che-rest:startapp installed apps"));

        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn startapp_creates_files_and_registers_module() {
        let out = temp_dir("startapp");
        startproject(StartProjectOptions {
            name: "todo_api".into(),
            out: out.clone(),
            che_rest_path: "../che-rest".into(),
            che_orm2_path: "../che-orm2".into(),
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
            che_orm2_path: "../che-orm2".into(),
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
}
