use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use che_orm::{
    Database, Migration, MigrationGraph, MigrationOperation, Model, SchemaSet, SqliteDialect,
    rusqlite::OptionalExtension,
};
use clap::{Parser, Subcommand};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::auth::{User, hash_password};
use crate::{AppConfig, AppModule, AppState, InstalledApps, StartAppOptions, StartProjectOptions};

type ManageResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Parser)]
#[command(name = "manage", about = "Project management commands")]
struct Cli {
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Debug, Subcommand)]
enum CommandKind {
    Startproject {
        name: String,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long, default_value = "")]
        che_rest_path: String,
        #[arg(long, default_value = "")]
        che_orm_path: String,
        #[arg(long, default_value_t = false)]
        with_auth: bool,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    Startapp {
        name: String,
        #[arg(long = "model")]
        models: Vec<String>,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    Schema,
    Makemigrations {
        name: Option<String>,
        #[arg(long, default_value = "migrations")]
        dir: PathBuf,
        #[arg(long, default_value_t = false)]
        empty: bool,
    },
    Migrate {
        #[command(subcommand)]
        action: Option<MigrateAction>,
        #[arg(long, default_value = "app.toml")]
        config: PathBuf,
        #[arg(long, default_value = "migrations")]
        dir: PathBuf,
    },
    Createsuperuser {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long, default_value = "app.toml")]
        config: PathBuf,
    },
    GenerateTs {
        #[arg(long, default_value = "frontend/client/src/generated")]
        out: PathBuf,
        #[arg(long, default_value = "app.toml")]
        config: PathBuf,
    },
    GenerateAdmin {
        #[arg(long, default_value = "frontend/admin")]
        out: PathBuf,
        #[arg(long, default_value = "app.toml")]
        config: PathBuf,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum MigrateAction {
    Apply,
    Status,
    OperationsApply,
    OperationsStatus,
    Verify,
    Lint,
    Diff { name: String },
}

pub struct Management {
    apps: InstalledApps,
    migrations: Vec<Migration>,
    project_root: PathBuf,
}

impl Management {
    pub fn new(apps: InstalledApps) -> Self {
        Self {
            apps,
            migrations: Vec::new(),
            project_root: PathBuf::from("."),
        }
    }

    pub fn project_root(mut self, project_root: impl Into<PathBuf>) -> Self {
        self.project_root = project_root.into();
        self
    }

    /// Registers compiled migrations used by `migrate operations-apply` and
    /// `migrate operations-status`.
    pub fn migrations(mut self, migrations: Vec<Migration>) -> Self {
        self.migrations = migrations;
        self
    }

    pub async fn run(self) -> ManageResult<()> {
        self.run_from(Cli::parse()).await
    }

    async fn run_from(self, cli: Cli) -> ManageResult<()> {
        match cli.command {
            CommandKind::Startproject {
                name,
                out,
                che_rest_path,
                che_orm_path,
                with_auth,
                force,
            } => crate::startproject(StartProjectOptions {
                name,
                out,
                che_rest_path,
                che_orm_path,
                with_auth,
                force,
            })?,
            CommandKind::Startapp {
                name,
                models,
                root,
                force,
            } => crate::startapp(StartAppOptions {
                name,
                models,
                root: root.unwrap_or(self.project_root),
                force,
            })?,
            CommandKind::Schema => print_schema(&self.apps),
            CommandKind::Makemigrations { name, dir, empty } => {
                if !self.migrations.is_empty() {
                    return Err(
                        "compiled operation migration generation is not implemented yet; do not use the legacy Atlas makemigrations workflow"
                            .into(),
                    );
                }
                let name = name.unwrap_or_else(|| {
                    let seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |duration| duration.as_secs());
                    format!("auto_{seconds}")
                });
                if empty {
                    write_empty_migration(&dir, &name)?;
                } else {
                    atlas_diff(&self.apps, &dir, &name)?;
                }
            }
            CommandKind::Migrate {
                action,
                config,
                dir,
            } => {
                let config = AppConfig::from_file(config)?;
                let action = action.unwrap_or(if self.migrations.is_empty() {
                    MigrateAction::Apply
                } else {
                    MigrateAction::OperationsApply
                });
                match action {
                    MigrateAction::Apply => {
                        if self.migrations.is_empty() {
                            apply_migrations(&config, dir).await?;
                        } else {
                            apply_operation_migrations(&config, self.migrations).await?;
                        }
                    }
                    MigrateAction::Status => {
                        if self.migrations.is_empty() {
                            migration_status(&config, dir).await?;
                        } else {
                            operation_migration_status(&config, self.migrations).await?;
                        }
                    }
                    MigrateAction::OperationsApply => {
                        apply_operation_migrations(&config, self.migrations).await?;
                    }
                    MigrateAction::OperationsStatus => {
                        operation_migration_status(&config, self.migrations).await?;
                    }
                    MigrateAction::Verify => {
                        if self.migrations.is_empty() {
                            verify_migrations(&config, dir).await?;
                        } else {
                            verify_operation_migrations(&config, self.migrations).await?;
                        }
                    }
                    MigrateAction::Lint | MigrateAction::Diff { .. }
                        if !self.migrations.is_empty() =>
                    {
                        return Err(
                            "Atlas commands are unavailable for compiled operation migrations"
                                .into(),
                        );
                    }
                    MigrateAction::Lint => atlas_lint(&dir)?,
                    MigrateAction::Diff { name } => atlas_diff(&self.apps, &dir, &name)?,
                }
            }
            CommandKind::Createsuperuser {
                username,
                password,
                config,
            } => create_superuser(username, password, config).await?,
            CommandKind::GenerateTs { out, config } => {
                AppConfig::from_file(config)?;
                let state = AppState::from_database(Database::connect_in_memory()?);
                let endpoints = self.apps.api_endpoints(state.clone());
                let signals = self.apps.api_signals(state);
                let files = crate::generate_ts::generate(
                    &out,
                    &endpoints,
                    &signals,
                    self.apps.find("auth").is_some(),
                )?;
                println!("generated {} files in {}", files.len(), out.display());
            }
            CommandKind::GenerateAdmin { out, config, force } => {
                AppConfig::from_file(config)?;
                let state = AppState::from_database(Database::connect_in_memory()?);
                let endpoints = self.apps.api_endpoints(state.clone());
                let signals = self.apps.api_signals(state);
                let generated = crate::generate_ts::generate(
                    &out.join("src/generated"),
                    &endpoints,
                    &signals,
                    self.apps.find("auth").is_some(),
                )?;
                let admin = crate::generate_admin::generate(&out, &endpoints, force)?;
                println!(
                    "generated {} files in {}",
                    generated.len() + admin.len(),
                    out.display()
                );
            }
        }
        Ok(())
    }
}

async fn create_superuser(username: String, password: String, config: PathBuf) -> ManageResult<()> {
    let state = AppState::from_config_file(config).await?;
    if state
        .database()
        .fetch_one(User::query().filter(User::USERNAME.eq(username.clone())))
        .await?
        .is_some()
    {
        return Err(format!("user `{username}` already exists").into());
    }

    let password_hash =
        hash_password(&password).map_err(|error| format!("could not hash password: {error}"))?;
    state
        .database()
        .create::<User>()
        .set(User::USERNAME, username.as_str())
        .set(User::PASSWORD_HASH, password_hash)
        .set(User::IS_ACTIVE, true)
        .set(User::IS_STAFF, true)
        .set(User::IS_ADMIN, true)
        .set(User::IS_SUPERUSER, true)
        .execute()
        .await?;
    println!("Superuser created.");
    Ok(())
}

fn schema(apps: &InstalledApps) -> SchemaSet {
    apps.iter()
        .map(AppModule::schema)
        .fold(SchemaSet::new(), SchemaSet::merge)
}

fn print_schema(apps: &InstalledApps) {
    print!("{}", schema(apps).to_sql::<SqliteDialect>());
}

fn write_empty_migration(dir: &PathBuf, name: &str) -> ManageResult<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(
            "migration name must contain only ascii letters, digits, and underscores".into(),
        );
    }

    let version = OffsetDateTime::now_utc()
        .format(&Rfc3339)?
        .chars()
        .filter(|character| character.is_ascii_digit())
        .take(14)
        .collect::<String>();
    let path = create_empty_migration(dir, name, &version)?;
    if let Err(error) = atlas_command(&["migrate", "hash", "--dir", &file_url(dir)]) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    println!("wrote empty migration to {}", path.display());
    Ok(())
}

fn create_empty_migration(dir: &PathBuf, name: &str, version: &str) -> ManageResult<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{version}_{name}.sql"));
    if path.exists() {
        return Err(format!("migration already exists: {}", path.display()).into());
    }
    fs::write(
        &path,
        "-- Write the required SQLite DDL, data backfill, or trigger changes here.\n",
    )?;
    Ok(path)
}

async fn apply_migrations(config: &AppConfig, dir: PathBuf) -> ManageResult<()> {
    let migrations = load_atlas_migrations(&dir)?;
    let applied_count = run_sqlite_migrations(config, migrations).await?;

    if applied_count == 0 {
        println!("No pending migrations.");
    } else {
        println!("Applied {applied_count} migration(s).");
    }
    Ok(())
}

async fn run_sqlite_migrations(
    config: &AppConfig,
    migrations: Vec<refinery::Migration>,
) -> ManageResult<usize> {
    let database = Database::connect_with_pool_size(
        sqlite_path(&config.database.url),
        config.database.max_connections as usize,
    )?;
    let pool = database.pool().clone();
    let result = pool
        .get()
        .await?
        .interact(move |connection| apply_sqlite_migrations(connection, migrations))
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

fn apply_sqlite_migrations(
    connection: &mut che_orm::rusqlite::Connection,
    migrations: Vec<refinery::Migration>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let applied = applied_migrations(connection)?;
    let filesystem = migrations
        .iter()
        .map(|migration| (migration.version(), migration.clone()))
        .collect::<HashMap<_, _>>();
    for migration in &applied {
        match filesystem.get(&migration.version()) {
            Some(expected)
                if expected.name() == migration.name()
                    && expected.checksum() == migration.checksum() => {}
            Some(expected) => {
                return Err(format!(
                    "divergent V{}__{} (filesystem: V{}__{})",
                    migration.version(),
                    migration.name(),
                    expected.version(),
                    expected.name()
                )
                .into());
            }
            None => {
                return Err(
                    format!("missing V{}__{}", migration.version(), migration.name()).into(),
                );
            }
        }
    }

    let applied_versions = applied
        .iter()
        .map(|migration| migration.version())
        .collect::<HashSet<_>>();
    let pending = migrations
        .into_iter()
        .filter(|migration| !applied_versions.contains(&migration.version()))
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(0);
    }

    connection
        .execute_batch("PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE;")
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
    let result = (|| {
        connection.execute_batch(MIGRATION_HISTORY_SQL)?;
        for migration in &pending {
            connection.execute_batch(
                migration
                    .sql()
                    .ok_or("pending migration is missing SQL content")?,
            )?;
            connection.execute(
                "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) VALUES (?1, ?2, ?3, ?4)",
                che_orm::rusqlite::params![
                    migration.version(),
                    migration.name(),
                    OffsetDateTime::now_utc().format(&Rfc3339)?,
                    migration.checksum().to_string(),
                ],
            )?;
        }
        assert_foreign_keys_valid(connection)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })();

    match result {
        Ok(()) => {
            connection
                .execute_batch("COMMIT; PRAGMA foreign_keys = ON;")
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(pending.len())
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK; PRAGMA foreign_keys = ON;");
            Err(error)
        }
    }
}

const MIGRATION_HISTORY_SQL: &str = "CREATE TABLE IF NOT EXISTS refinery_schema_history(\n             version int8 PRIMARY KEY,\n             name VARCHAR(255),\n             applied_on VARCHAR(255),\n             checksum VARCHAR(255));";

fn applied_migrations(
    connection: &che_orm::rusqlite::Connection,
) -> Result<Vec<refinery::Migration>, Box<dyn std::error::Error + Send + Sync>> {
    let has_history = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'refinery_schema_history'",
            [],
            |_| Ok(()),
        )
        .optional()?;
    if has_history.is_none() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT version, name, applied_on, checksum FROM refinery_schema_history ORDER BY version ASC",
    )?;
    let mut rows = statement.query([])?;
    let mut migrations = Vec::new();
    while let Some(row) = rows.next()? {
        let applied_on: String = row.get(2)?;
        migrations.push(refinery::Migration::applied(
            row.get(0)?,
            row.get(1)?,
            OffsetDateTime::parse(&applied_on, &Rfc3339)?,
            row.get::<_, String>(3)?.parse()?,
        ));
    }
    Ok(migrations)
}

async fn migration_status(config: &AppConfig, dir: PathBuf) -> ManageResult<usize> {
    let migrations = load_atlas_migrations(&dir)?;
    let filesystem = migrations
        .iter()
        .map(|migration| (migration.version(), migration.clone()))
        .collect::<BTreeMap<_, _>>();
    let applied = run_with_refinery(config, migrations, |runner, connection| {
        let has_history = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'refinery_schema_history'",
                [],
                |_| Ok(()),
            )
            .optional()?;
        if has_history.is_none() {
            return Ok(Vec::new());
        }
        runner.get_applied_migrations(connection).map_err(Into::into)
    })
    .await?;
    let applied_versions = applied
        .iter()
        .map(|migration| migration.version())
        .collect::<HashSet<_>>();
    let mut has_status_error = false;

    for migration in &applied {
        match filesystem.get(&migration.version()) {
            Some(expected)
                if expected.name() == migration.name()
                    && expected.checksum() == migration.checksum() =>
            {
                println!("applied V{}__{}", migration.version(), migration.name());
            }
            Some(expected) => {
                has_status_error = true;
                println!(
                    "divergent V{}__{} (filesystem: V{}__{})",
                    migration.version(),
                    migration.name(),
                    expected.version(),
                    expected.name()
                );
            }
            None => {
                has_status_error = true;
                println!("missing V{}__{}", migration.version(), migration.name());
            }
        }
    }

    let mut pending_count = 0;
    for migration in filesystem.values() {
        if !applied_versions.contains(&migration.version()) {
            pending_count += 1;
            println!("pending V{}__{}", migration.version(), migration.name());
        }
    }

    if has_status_error {
        Err("migration history contains missing or divergent migrations".into())
    } else {
        Ok(pending_count)
    }
}

/// Applies compiled operation migrations. This is primarily used by the
/// management command and migration integration tests.
pub async fn apply_operation_migrations(
    config: &AppConfig,
    migrations: Vec<Migration>,
) -> ManageResult<()> {
    let applied_count = run_operation_migrations(config, migrations).await?;
    if applied_count == 0 {
        println!("No pending operation migrations.");
    } else {
        println!("Applied {applied_count} operation migration(s).");
    }
    Ok(())
}

async fn run_operation_migrations(
    config: &AppConfig,
    migrations: Vec<Migration>,
) -> ManageResult<usize> {
    let graph = MigrationGraph::new(migrations)
        .map_err(|error| format!("invalid operation migration graph: {error}"))?;
    let database = Database::connect_with_pool_size(
        sqlite_path(&config.database.url),
        config.database.max_connections as usize,
    )?;
    let pool = database.pool().clone();
    let result = pool
        .get()
        .await?
        .interact(move |connection| apply_operation_migrations_on_connection(connection, graph))
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

fn apply_operation_migrations_on_connection(
    connection: &mut che_orm::rusqlite::Connection,
    graph: MigrationGraph,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let applied = applied_operation_migrations(connection)?;
    let pending = pending_operation_migrations(&graph, &applied)?;
    if pending.is_empty() {
        return Ok(0);
    }

    for migration in &pending {
        for operation in &migration.operations {
            if !matches!(operation, MigrationOperation::RunSql { .. }) {
                return Err(format!(
                    "operation migration {}:{} uses an unsupported operation; only RunSql is supported until a SQLite SQL renderer is available",
                    migration.id.app, migration.id.name
                )
                .into());
            }
        }
    }

    connection
        .execute_batch("PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE;")
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
    let result = (|| {
        connection.execute_batch(OPERATION_MIGRATION_HISTORY_SQL)?;
        for migration in &pending {
            for operation in &migration.operations {
                let MigrationOperation::RunSql { forward, .. } = operation else {
                    unreachable!("operations were validated before starting the transaction");
                };
                connection.execute_batch(forward)?;
            }
            connection.execute(
                "INSERT INTO che_migration_history (app, name, checksum, applied_on) VALUES (?1, ?2, ?3, ?4)",
                che_orm::rusqlite::params![
                    migration.id.app,
                    migration.id.name,
                    migration.checksum,
                    OffsetDateTime::now_utc().format(&Rfc3339)?,
                ],
            )?;
        }
        assert_foreign_keys_valid(connection)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })();

    match result {
        Ok(()) => {
            connection
                .execute_batch("COMMIT; PRAGMA foreign_keys = ON;")
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(pending.len())
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK; PRAGMA foreign_keys = ON;");
            Err(error)
        }
    }
}

async fn operation_migration_status(
    config: &AppConfig,
    migrations: Vec<Migration>,
) -> ManageResult<usize> {
    let graph = MigrationGraph::new(migrations)
        .map_err(|error| format!("invalid operation migration graph: {error}"))?;
    let database = Database::connect_with_pool_size(
        sqlite_path(&config.database.url),
        config.database.max_connections as usize,
    )?;
    let pool = database.pool().clone();
    let result = pool
        .get()
        .await?
        .interact(move |connection| operation_migration_status_for_connection(connection, graph))
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

async fn verify_operation_migrations(
    config: &AppConfig,
    migrations: Vec<Migration>,
) -> ManageResult<()> {
    let pending_count = operation_migration_status(config, migrations).await?;
    if pending_count != 0 {
        return Err(format!("{pending_count} operation migration(s) are pending").into());
    }
    verify_foreign_keys(config).await?;
    println!("Operation migration verification passed.");
    Ok(())
}

fn operation_migration_status_for_connection(
    connection: &che_orm::rusqlite::Connection,
    graph: MigrationGraph,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let applied = applied_operation_migrations(connection)?;
    let pending = pending_operation_migrations(&graph, &applied)?;
    for migration in graph.ordered() {
        if applied.contains_key(&(migration.id.app.clone(), migration.id.name.clone())) {
            println!("applied {}:{}", migration.id.app, migration.id.name);
        } else {
            println!("pending {}:{}", migration.id.app, migration.id.name);
        }
    }
    Ok(pending.len())
}

fn pending_operation_migrations<'a>(
    graph: &'a MigrationGraph,
    applied: &HashMap<(String, String), String>,
) -> Result<Vec<&'a Migration>, Box<dyn std::error::Error + Send + Sync>> {
    let registered = graph
        .ordered()
        .map(|migration| {
            (
                (migration.id.app.clone(), migration.id.name.clone()),
                migration,
            )
        })
        .collect::<HashMap<_, _>>();
    for ((app, name), checksum) in applied {
        match registered.get(&(app.clone(), name.clone())) {
            Some(migration) if migration.checksum == *checksum => {}
            Some(_) => return Err(format!("divergent operation migration {app}:{name}").into()),
            None => return Err(format!("missing operation migration {app}:{name}").into()),
        }
    }
    Ok(graph
        .ordered()
        .filter(|migration| {
            !applied.contains_key(&(migration.id.app.clone(), migration.id.name.clone()))
        })
        .collect())
}

const OPERATION_MIGRATION_HISTORY_SQL: &str = "CREATE TABLE IF NOT EXISTS che_migration_history (app TEXT NOT NULL, name TEXT NOT NULL, checksum TEXT NOT NULL, applied_on TEXT NOT NULL, PRIMARY KEY (app, name));";

fn applied_operation_migrations(
    connection: &che_orm::rusqlite::Connection,
) -> Result<HashMap<(String, String), String>, Box<dyn std::error::Error + Send + Sync>> {
    let has_history = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'che_migration_history'",
            [],
            |_| Ok(()),
        )
        .optional()?;
    if has_history.is_none() {
        return Ok(HashMap::new());
    }
    let mut statement =
        connection.prepare("SELECT app, name, checksum FROM che_migration_history")?;
    let rows = statement.query_map([], |row| {
        Ok((
            (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
            row.get(2)?,
        ))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

async fn verify_migrations(config: &AppConfig, dir: PathBuf) -> ManageResult<()> {
    let pending_count = migration_status(config, dir.clone()).await?;
    if pending_count != 0 {
        return Err(format!("{pending_count} migration(s) are pending").into());
    }
    verify_foreign_keys(config).await?;
    atlas_lint(&dir)?;
    println!("Migration verification passed.");
    Ok(())
}

async fn verify_foreign_keys(config: &AppConfig) -> ManageResult<()> {
    let database = Database::connect_with_pool_size(
        sqlite_path(&config.database.url),
        config.database.max_connections as usize,
    )?;
    let pool = database.pool().clone();
    let result = pool
        .get()
        .await?
        .interact(|connection| assert_foreign_keys_valid(connection))
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

async fn run_with_refinery<T, F>(
    config: &AppConfig,
    migrations: Vec<refinery::Migration>,
    action: F,
) -> ManageResult<T>
where
    T: Send + 'static,
    F: FnOnce(
            refinery::Runner,
            &mut che_orm::rusqlite::Connection,
        ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
        + Send
        + 'static,
{
    let database = Database::connect_with_pool_size(
        sqlite_path(&config.database.url),
        config.database.max_connections as usize,
    )?;
    let pool = database.pool().clone();
    let result = pool
        .get()
        .await?
        .interact(move |connection| {
            let runner = refinery::Runner::new(&migrations)
                .set_abort_divergent(true)
                .set_abort_missing(true);
            action(runner, connection)
        })
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

fn assert_foreign_keys_valid(
    connection: &che_orm::rusqlite::Connection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check;")
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
    let mut rows = statement
        .query([])
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
    if rows
        .next()
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?
        .is_some()
    {
        return Err("migration left foreign key violations".into());
    }
    Ok(())
}

fn load_atlas_migrations(dir: &PathBuf) -> ManageResult<Vec<refinery::Migration>> {
    let mut migrations = Vec::new();
    let mut versions = HashSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("sql") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("invalid migration filename: {}", path.display()))?;
        let Some((version, name)) = stem.split_once('_') else {
            return Err(format!(
                "invalid Atlas migration filename `{}`; expected <version>_<name>.sql",
                path.display()
            )
            .into());
        };
        if version.len() != 14 || !version.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(format!(
                "invalid Atlas migration version `{version}` in {}; expected 14 digits",
                path.display()
            )
            .into());
        }
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(format!(
                "invalid Atlas migration name `{name}` in {}; use only ascii letters, digits, and underscores",
                path.display()
            )
            .into());
        }
        if !versions.insert(version.to_string()) {
            return Err(format!("duplicate migration version `{version}`").into());
        }

        let sql = fs::read_to_string(&path)?;
        let refinery_name = format!("V{version}__{name}");
        migrations.push(refinery::Migration::unapplied(&refinery_name, &sql)?);
    }
    migrations.sort();
    Ok(migrations)
}

fn sqlite_path(url: &str) -> String {
    url.strip_prefix("sqlite://")
        .and_then(|value| value.split('?').next())
        .filter(|value| !value.is_empty())
        .unwrap_or(url)
        .to_string()
}

fn atlas_diff(apps: &InstalledApps, dir: &PathBuf, name: &str) -> ManageResult<()> {
    let temp_path = env::temp_dir().join(format!("che-rest-schema-{}.sql", std::process::id()));
    fs::write(&temp_path, schema(apps).to_sql::<SqliteDialect>())?;
    let to = file_url(&temp_path);
    let result = atlas_command(&[
        "migrate",
        "diff",
        name,
        "--dir",
        &file_url(dir),
        "--to",
        &to,
        "--dev-url",
        "sqlite://dev?mode=memory",
    ]);
    let _ = fs::remove_file(temp_path);
    result
}

fn atlas_lint(dir: &PathBuf) -> ManageResult<()> {
    atlas_command(&[
        "migrate",
        "lint",
        "--dir",
        &file_url(dir),
        "--dev-url",
        "sqlite://dev?mode=memory",
        "--latest",
        "1",
    ])
}

fn atlas_command(args: &[&str]) -> ManageResult<()> {
    let binary = env::var_os("ATLAS_BIN").unwrap_or_else(|| "atlas".into());
    let status = Command::new(binary).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("atlas exited with status {status}").into())
    }
}

fn file_url(path: &PathBuf) -> String {
    let path = if path.is_absolute() {
        path.clone()
    } else {
        env::current_dir().unwrap_or_default().join(path)
    };
    format!("file://{}", path.display())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use che_orm::{
        Database, Migration, MigrationId, MigrationOperation, Model, rusqlite::OptionalExtension,
    };
    use clap::Parser;

    use super::{
        Cli, Management, apply_migrations, apply_operation_migrations, create_empty_migration,
        migration_status, operation_migration_status,
    };
    use crate::InstalledApps;
    use crate::auth::{User, verify_password};
    use crate::{AppConfig, DatabaseConfig};

    fn temp_path(name: &str, extension: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("che_rest_{name}_{suffix}.{extension}"))
    }

    fn sqlite_config(database_path: &std::path::Path) -> AppConfig {
        AppConfig {
            database: DatabaseConfig {
                url: format!("sqlite://{}?mode=rwc", database_path.display()),
                max_connections: 1,
            },
            ..AppConfig::default()
        }
    }

    #[test]
    fn parses_project_generation_commands() {
        Cli::try_parse_from([
            "manage",
            "startproject",
            "todo_api",
            "--out",
            "..",
            "--with-auth",
        ])
        .unwrap();
        Cli::try_parse_from([
            "manage", "startapp", "tasks", "--model", "Task", "--model", "Comment",
        ])
        .unwrap();
        Cli::try_parse_from(["manage", "makemigrations", "backfill_tasks", "--empty"]).unwrap();
        Cli::try_parse_from(["manage", "migrate", "verify"]).unwrap();
        Cli::try_parse_from(["manage", "migrate", "operations-apply"]).unwrap();
        Cli::try_parse_from(["manage", "migrate", "operations-status"]).unwrap();
    }

    #[test]
    fn empty_migration_contains_manual_sql_prompt() {
        let directory = temp_path("empty_migration", "dir");
        fs::create_dir(&directory).unwrap();
        let path = create_empty_migration(&directory, "backfill_tasks", "20260914120000").unwrap();
        assert_eq!(
            path.file_name().unwrap(),
            "20260914120000_backfill_tasks.sql"
        );
        assert!(fs::read_to_string(&path).unwrap().contains("data backfill"));
        assert!(create_empty_migration(&directory, "backfill_tasks", "20260914120000").is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn creates_superuser_with_hashed_password() {
        let database_path = temp_path("superuser", "sqlite");
        let config_path = temp_path("superuser", "toml");
        fs::write(
            &config_path,
            format!(
                "[database]\nurl = \"sqlite://{}?mode=rwc\"\n",
                database_path.display()
            ),
        )
        .unwrap();
        Database::connect(database_path.to_string_lossy())
            .unwrap()
            .create_table::<User>()
            .await
            .unwrap();

        let cli = Cli::try_parse_from([
            "manage",
            "createsuperuser",
            "--username",
            "admin",
            "--password",
            "secret",
            "--config",
            config_path.to_str().unwrap(),
        ])
        .unwrap();
        Management::new(InstalledApps::new())
            .run_from(cli)
            .await
            .unwrap();

        let database = Database::connect(database_path.to_string_lossy()).unwrap();
        let user = database
            .fetch_one(User::query().filter(User::USERNAME.eq("admin")))
            .await
            .unwrap()
            .unwrap();
        assert!(verify_password("secret", &user.password_hash));
        assert!(user.is_active && user.is_staff && user.is_admin && user.is_superuser);

        let _ = fs::remove_file(database_path);
        let _ = fs::remove_file(config_path);
    }

    #[tokio::test]
    async fn generate_admin_cli_creates_project_files() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!("che_rest_admin_{suffix}"));
        let config = output.join("app.toml");
        fs::create_dir_all(&output).unwrap();
        fs::write(&config, "[database]\nurl = 'sqlite://unused.sqlite'\n").unwrap();
        let cli = Cli::try_parse_from([
            "manage",
            "generate-admin",
            "--out",
            output.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ])
        .unwrap();
        Management::new(InstalledApps::new())
            .run_from(cli)
            .await
            .unwrap();
        assert!(output.join("package.json").exists());
        assert!(output.join("src/admin/generated/adminSchema.ts").exists());
        assert!(output.join("src/router.ts").exists());
        let _ = fs::remove_dir_all(output);
    }

    #[tokio::test]
    async fn migrate_apply_runs_refinery_without_atlas() {
        let migrations = temp_path("migrations", "dir");
        fs::create_dir_all(&migrations).unwrap();
        fs::write(
            migrations.join("20260817000000_initial.sql"),
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        )
        .unwrap();
        fs::write(
            migrations.join("20260817000001_add_rows.sql"),
            "INSERT INTO items (name) VALUES ('one');",
        )
        .unwrap();
        let database_path = temp_path("migrate_apply", "sqlite");
        let config = sqlite_config(&database_path);

        apply_migrations(&config, migrations.clone()).await.unwrap();
        apply_migrations(&config, migrations.clone()).await.unwrap();

        let connection = che_orm::rusqlite::Connection::open(&database_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let history_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(history_count, 2);

        let _ = fs::remove_dir_all(migrations);
        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn migrate_apply_rejects_changed_migration() {
        let migrations = temp_path("migrations_changed", "dir");
        fs::create_dir_all(&migrations).unwrap();
        let migration = migrations.join("20260817000000_initial.sql");
        fs::write(&migration, "CREATE TABLE items (id INTEGER PRIMARY KEY);").unwrap();
        let database_path = temp_path("migrate_changed", "sqlite");
        let config = sqlite_config(&database_path);

        apply_migrations(&config, migrations.clone()).await.unwrap();
        fs::write(&migration, "CREATE TABLE changed (id INTEGER PRIMARY KEY);").unwrap();

        let error = apply_migrations(&config, migrations.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.is_empty());

        let _ = fs::remove_dir_all(migrations);
        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn migrate_status_reports_pending_on_fresh_database() {
        let migrations = temp_path("migrations_status", "dir");
        fs::create_dir_all(&migrations).unwrap();
        fs::write(
            migrations.join("20260817000000_initial.sql"),
            "CREATE TABLE items (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        let database_path = temp_path("migrate_status", "sqlite");
        let config = sqlite_config(&database_path);

        migration_status(&config, migrations.clone()).await.unwrap();

        let _ = fs::remove_dir_all(migrations);
        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn operation_migrations_apply_and_record_history() {
        let database_path = temp_path("operation_migrate", "sqlite");
        let config = sqlite_config(&database_path);
        let migrations = vec![Migration {
            id: MigrationId {
                app: "tasks".into(),
                name: "0001_initial".into(),
            },
            dependencies: vec![],
            operations: vec![MigrationOperation::RunSql {
                forward: "CREATE TABLE operation_items (id INTEGER PRIMARY KEY);".into(),
                reverse: None,
                state_operations: vec![],
            }],
            checksum: "operation-items-v1".into(),
        }];

        apply_operation_migrations(&config, migrations.clone())
            .await
            .unwrap();
        apply_operation_migrations(&config, migrations.clone())
            .await
            .unwrap();
        assert_eq!(
            operation_migration_status(&config, migrations)
                .await
                .unwrap(),
            0
        );

        let connection = che_orm::rusqlite::Connection::open(&database_path).unwrap();
        let table: String = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'operation_items'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table, "operation_items");
        let history: (String, String, String) = connection
            .query_row(
                "SELECT app, name, checksum FROM che_migration_history",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            history,
            (
                "tasks".into(),
                "0001_initial".into(),
                "operation-items-v1".into()
            )
        );

        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn migrate_apply_rejects_foreign_key_violations() {
        let migrations = temp_path("migrations_fk", "dir");
        fs::create_dir_all(&migrations).unwrap();
        fs::write(
            migrations.join("20260817000000_initial.sql"),
            "CREATE TABLE parents (id INTEGER PRIMARY KEY);\nCREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parents(id));\nINSERT INTO children (id, parent_id) VALUES (1, 999);",
        )
        .unwrap();
        let database_path = temp_path("migrate_fk", "sqlite");
        let config = sqlite_config(&database_path);

        let error = apply_migrations(&config, migrations.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("foreign key violations"));

        let connection = che_orm::rusqlite::Connection::open(&database_path).unwrap();
        let children_table: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'children'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(children_table.is_none());
        let history_table: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'refinery_schema_history'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(history_table.is_none());

        let _ = fs::remove_dir_all(migrations);
        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn migrate_apply_runs_checked_in_atlas_migrations() {
        let database_path = temp_path("cli_fullstack_migrate", "sqlite");
        let config = sqlite_config(&database_path);
        let migrations =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/cli_fullstack/migrations");

        apply_migrations(&config, migrations).await.unwrap();

        let connection = che_orm::rusqlite::Connection::open(&database_path).unwrap();
        let task_table: String = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'tasks_task'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(task_table, "tasks_task");
        let history_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(history_count, 3);

        let _ = fs::remove_file(database_path);
    }
}
