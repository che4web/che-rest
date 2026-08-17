use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use che_orm2::{Database, Model, SchemaSet, SqliteDialect, rusqlite::OptionalExtension};
use clap::{Parser, Subcommand};

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
        #[arg(long, default_value = "../che-rest")]
        che_rest_path: String,
        #[arg(long, default_value = "../che-orm2")]
        che_orm2_path: String,
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
    Lint,
    Diff { name: String },
}

pub struct Management {
    apps: InstalledApps,
    project_root: PathBuf,
}

impl Management {
    pub fn new(apps: InstalledApps) -> Self {
        Self {
            apps,
            project_root: PathBuf::from("."),
        }
    }

    pub fn project_root(mut self, project_root: impl Into<PathBuf>) -> Self {
        self.project_root = project_root.into();
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
                che_orm2_path,
                with_auth,
                force,
            } => crate::startproject(StartProjectOptions {
                name,
                out,
                che_rest_path,
                che_orm2_path,
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
            CommandKind::Makemigrations { name, dir } => {
                let name = name.unwrap_or_else(|| {
                    let seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |duration| duration.as_secs());
                    format!("auto_{seconds}")
                });
                atlas_diff(&self.apps, &dir, &name)?;
            }
            CommandKind::Migrate {
                action,
                config,
                dir,
            } => {
                let config = AppConfig::from_file(config)?;
                let action = action.unwrap_or(MigrateAction::Apply);
                match action {
                    MigrateAction::Apply => apply_migrations(&config, dir).await?,
                    MigrateAction::Status => migration_status(&config, dir).await?,
                    MigrateAction::Lint => atlas_command(&[
                        "migrate",
                        "lint",
                        "--dir",
                        &file_url(&dir),
                        "--dev-url",
                        "sqlite://dev?mode=memory",
                    ])?,
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

async fn apply_migrations(config: &AppConfig, dir: PathBuf) -> ManageResult<()> {
    let migrations = load_atlas_migrations(&dir)?;
    let applied_count = run_with_refinery(config, migrations, true, |runner, connection| {
        let report = runner.run(connection)?;
        Ok(report.applied_migrations().len())
    })
    .await?;

    if applied_count == 0 {
        println!("No pending migrations.");
    } else {
        println!("Applied {applied_count} migration(s).");
    }
    Ok(())
}

async fn migration_status(config: &AppConfig, dir: PathBuf) -> ManageResult<()> {
    let migrations = load_atlas_migrations(&dir)?;
    let filesystem = migrations
        .iter()
        .map(|migration| (migration.version(), migration.clone()))
        .collect::<BTreeMap<_, _>>();
    let applied = run_with_refinery(config, migrations, false, |runner, connection| {
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

    for migration in filesystem.values() {
        if !applied_versions.contains(&migration.version()) {
            println!("pending V{}__{}", migration.version(), migration.name());
        }
    }

    if has_status_error {
        Err("migration history contains missing or divergent migrations".into())
    } else {
        Ok(())
    }
}

async fn run_with_refinery<T, F>(
    config: &AppConfig,
    migrations: Vec<refinery::Migration>,
    validate_foreign_keys: bool,
    action: F,
) -> ManageResult<T>
where
    T: Send + 'static,
    F: FnOnce(
            refinery::Runner,
            &mut che_orm2::rusqlite::Connection,
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
            if validate_foreign_keys {
                connection
                    .execute_batch("PRAGMA foreign_keys = OFF;")
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            }
            let result = action(runner, connection);
            let restore = if validate_foreign_keys {
                connection.execute_batch("PRAGMA foreign_keys = ON;")
            } else {
                Ok(())
            };
            match (result, restore) {
                (Ok(value), Ok(())) => {
                    if validate_foreign_keys {
                        assert_foreign_keys_valid(connection)?;
                    }
                    Ok(value)
                }
                (Ok(_), Err(error)) => {
                    Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
                }
                (Err(error), _) => Err(error),
            }
        })
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

fn assert_foreign_keys_valid(
    connection: &che_orm2::rusqlite::Connection,
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

    use che_orm2::{Database, Model};
    use clap::Parser;

    use super::{Cli, Management, apply_migrations, migration_status};
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

        let connection = che_orm2::rusqlite::Connection::open(&database_path).unwrap();
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

        let connection = che_orm2::rusqlite::Connection::open(&database_path).unwrap();
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
