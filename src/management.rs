use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use che_orm2::{Database, Model, SchemaSet, SqliteDialect};
use clap::{Parser, Subcommand};

use crate::auth::{User, hash_password};
use crate::{AppConfig, AppModule, AppState, InstalledApps};

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
            CommandKind::Createsuperuser {
                username,
                password,
                config,
            } => create_superuser(username, password, config).await?,
            CommandKind::GenerateTs { out, config } => {
                AppConfig::from_file(config)?;
                let state = AppState::from_database(Database::connect_in_memory()?);
                let endpoints = self.apps.api_endpoints(state);
                let files = crate::generate_ts::generate(
                    &out,
                    &endpoints,
                    self.apps.find("auth").is_some(),
                )?;
                println!("generated {} files in {}", files.len(), out.display());
            }
            CommandKind::GenerateAdmin { out, config, force } => {
                AppConfig::from_file(config)?;
                let state = AppState::from_database(Database::connect_in_memory()?);
                let endpoints = self.apps.api_endpoints(state);
                let generated = crate::generate_ts::generate(
                    &out.join("src/generated"),
                    &endpoints,
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
        time::{SystemTime, UNIX_EPOCH},
    };

    use che_orm2::{Database, Model};
    use clap::Parser;

    use super::{Cli, Management};
    use crate::InstalledApps;
    use crate::auth::{User, verify_password};

    #[tokio::test]
    async fn creates_superuser_with_hashed_password() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let database_path =
            std::env::temp_dir().join(format!("che_rest_superuser_{suffix}.sqlite"));
        let config_path = std::env::temp_dir().join(format!("che_rest_superuser_{suffix}.toml"));
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
}
