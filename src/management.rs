use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use che_orm2::{SchemaSet, SqliteDialect};
use clap::{Parser, Subcommand};

use crate::{AppConfig, AppModule, InstalledApps};

type ManageResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Parser)]
#[command(name = "manage", about = "Project management commands")]
struct Cli {
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Debug, Subcommand)]
enum CommandKind {
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
}

impl Management {
    pub fn new(apps: InstalledApps) -> Self {
        Self { apps }
    }

    pub async fn run(self) -> ManageResult<()> {
        self.run_from(Cli::parse()).await
    }

    async fn run_from(self, cli: Cli) -> ManageResult<()> {
        match cli.command {
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
                    MigrateAction::Apply => atlas_command(&[
                        "migrate",
                        "apply",
                        "--dir",
                        &file_url(&dir),
                        "--url",
                        &config.database.url,
                    ])?,
                    MigrateAction::Status => atlas_command(&[
                        "migrate",
                        "status",
                        "--dir",
                        &file_url(&dir),
                        "--url",
                        &config.database.url,
                    ])?,
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
        }
        Ok(())
    }
}

fn schema(apps: &InstalledApps) -> SchemaSet {
    apps.iter()
        .map(AppModule::schema)
        .fold(SchemaSet::new(), SchemaSet::merge)
}

fn print_schema(apps: &InstalledApps) {
    print!("{}", schema(apps).to_sql::<SqliteDialect>());
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
