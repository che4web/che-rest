use std::{
    fs,
    path::{Path, PathBuf},
};

use che_orm::{Schema, SqliteBackend, diff_schemas, sqlite_migration_sql};
use clap::{Parser, Subcommand};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(name = "che-rest")]
#[command(about = "CLI tools for che-rest applications")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Startapp {
        name: String,

        #[arg(long, default_value = "src/apps")]
        apps_dir: String,
    },
    Makemigrations {
        app: String,

        #[arg(long, default_value = "src/apps")]
        apps_dir: PathBuf,

        #[arg(long, default_value = "che_orm_schema.json")]
        schema: PathBuf,

        #[arg(long, default_value = "auto")]
        name: String,
    },
    Migrate {
        app: String,

        #[arg(long, default_value = "src/apps")]
        apps_dir: PathBuf,

        #[arg(long, default_value = "app.toml")]
        config: PathBuf,

        #[arg(long)]
        database_url: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Startapp { name, apps_dir } => startapp(&name, Path::new(&apps_dir))?,
        Command::Makemigrations {
            app,
            apps_dir,
            schema,
            name,
        } => makemigrations(&app, &apps_dir, &schema, &name)?,
        Command::Migrate {
            app,
            apps_dir,
            config,
            database_url,
        } => migrate(&app, &apps_dir, &config, database_url).await?,
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    database: DatabaseConfig,
}

#[derive(Debug, Deserialize)]
struct DatabaseConfig {
    url: String,
}

fn startapp(name: &str, apps_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    validate_app_name(name)?;

    let app_dir = apps_dir.join(name);
    if app_dir.exists() {
        return Err(format!("app already exists: {}", app_dir.display()).into());
    }

    fs::create_dir_all(&app_dir)?;
    fs::write(app_dir.join("mod.rs"), mod_template(name))?;
    fs::write(app_dir.join("models.rs"), models_template(name))?;
    fs::write(app_dir.join("serializers.rs"), serializers_template(name))?;
    fs::write(app_dir.join("filters.rs"), filters_template(name))?;
    fs::write(app_dir.join("views.rs"), views_template(name))?;

    update_apps_mod(apps_dir, name)?;

    println!("created app {}", app_dir.display());
    println!("add to your crate root: mod apps;");
    println!("register it with: Server::new(state).register(apps::{name}::module())");

    Ok(())
}

fn makemigrations(
    app: &str,
    apps_dir: &Path,
    schema_path: &Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_app_name(app)?;
    ensure_app_exists(apps_dir, app)?;

    let migrations_dir = app_migrations_dir(apps_dir, app);
    fs::create_dir_all(&migrations_dir)?;

    let snapshot_path = migrations_dir.join("schema.json");
    let old_schema = Schema::load_or_empty(&snapshot_path)?;
    let new_schema = Schema::load(schema_path)?;
    let migration = diff_schemas(&old_schema, &new_schema);

    if migration.changes.is_empty() {
        println!("No schema changes detected for app {app}");
        return Ok(());
    }

    let sql = sqlite_migration_sql(&migration);
    let file_name = format!(
        "{:04}_{}.sql",
        next_migration_number(&migrations_dir)?,
        slugify(name)
    );
    let migration_path = migrations_dir.join(file_name);
    fs::write(&migration_path, format!("{sql}\n"))?;
    new_schema.save(snapshot_path)?;

    println!("Created {}", migration_path.display());

    Ok(())
}

async fn migrate(
    app: &str,
    apps_dir: &Path,
    config: &Path,
    database_url: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_app_name(app)?;
    ensure_app_exists(apps_dir, app)?;

    let database_url = match database_url {
        Some(database_url) => database_url,
        None => database_url_from_config(config)?,
    };
    let migrations_dir = app_migrations_dir(apps_dir, app);
    let db = SqliteBackend::connect(&database_url).await?;

    for name in db
        .apply_migrations_dir_with_namespace(app, &migrations_dir)
        .await?
    {
        println!("Applied {app}: {name}");
    }

    Ok(())
}

fn database_url_from_config(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let config = fs::read_to_string(path)?;
    let config: AppConfig = toml::from_str(&config)?;
    Ok(config.database.url)
}

fn ensure_app_exists(apps_dir: &Path, app: &str) -> Result<(), Box<dyn std::error::Error>> {
    let app_dir = apps_dir.join(app);
    if !app_dir.exists() {
        return Err(format!("app does not exist: {}", app_dir.display()).into());
    }
    Ok(())
}

fn app_migrations_dir(apps_dir: &Path, app: &str) -> PathBuf {
    apps_dir.join(app).join("migrations")
}

fn migration_files(migrations_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if !migrations_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(migrations_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "sql") {
            files.push(path);
        }
    }
    Ok(files)
}

fn next_migration_number(migrations_dir: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    let max = migration_files(migrations_dir)?
        .iter()
        .filter_map(|path| path.file_name()?.to_str()?.get(0..4)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(max + 1)
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    slug.trim_matches('_').to_string()
}

fn validate_app_name(name: &str) -> Result<(), Box<dyn std::error::Error>> {
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

fn update_apps_mod(apps_dir: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(apps_dir)?;
    let mod_path = apps_dir.join("mod.rs");
    let line = format!("pub mod {name};");
    let mut content = fs::read_to_string(&mod_path).unwrap_or_default();

    if !content.lines().any(|existing| existing.trim() == line) {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&line);
        content.push('\n');
        fs::write(mod_path, content)?;
    }

    Ok(())
}

fn mod_template(name: &str) -> String {
    let module_type = format!("{}Module", plural_camel(name));
    let model = singular_camel(name);
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
        "{name}"
    }}

    fn init(&self, ctx: &mut ModuleContext) {{
        ctx.create_table::<models::{model}>();
        ctx.route(views::routes());
    }}
}}
"#
    )
}

fn models_template(name: &str) -> String {
    let model = singular_camel(name);
    format!(
        r#"use che_orm::Model;

#[derive(Debug, Clone, Model)]
#[model(table = "{name}")]
pub struct {model} {{
    #[field(primary_key)]
    pub id: i64,

    pub name: String,
}}
"#
    )
}

fn serializers_template(name: &str) -> String {
    let model = singular_camel(name);
    let fn_name = singular_name(name);
    format!(
        r#"use che_rest::{{Field, ModelSerializer}};

use super::models::{model};

static {const_name}_FIELDS: &[Field] = &[
    Field::new("id").read_only(),
    Field::new("name"),
];

pub fn {fn_name}_serializer() -> ModelSerializer<{model}> {{
    ModelSerializer::new({const_name}_FIELDS)
}}
"#,
        const_name = name.to_ascii_uppercase()
    )
}

fn filters_template(name: &str) -> String {
    let model = singular_camel(name);
    let fn_name = singular_name(name);
    format!(
        r#"use che_rest::{{Filter, FilterSet}};

use super::models::{model};

static {const_name}_FILTERS: &[Filter] = &[
    Filter::exact("name"),
    Filter::contains("name"),
];

pub fn {fn_name}_filterset() -> FilterSet<{model}> {{
    FilterSet::new({const_name}_FILTERS)
}}
"#,
        const_name = name.to_ascii_uppercase()
    )
}

fn views_template(name: &str) -> String {
    let model = singular_camel(name);
    let viewset = format!("{}ViewSet", singular_camel(name));
    let fn_name = singular_name(name);
    format!(
        r#"use axum::Router;
use che_rest::ModelViewSet;

use super::{{filters::{fn_name}_filterset, models::{model}, serializers::{fn_name}_serializer}};

type {viewset} = ModelViewSet<{model}>;

pub fn routes() -> Router {{
    {viewset}::router("/{name}", {fn_name}_serializer(), {fn_name}_filterset())
}}
"#
    )
}

fn singular_name(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("ies") {
        format!("{stem}y")
    } else if name.ends_with("ss") {
        name.to_string()
    } else {
        name.strip_suffix('s').unwrap_or(name).to_string()
    }
}

fn singular_camel(name: &str) -> String {
    camel_case(&singular_name(name))
}

fn plural_camel(name: &str) -> String {
    camel_case(name)
}

fn camel_case(name: &str) -> String {
    name.split('_')
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
