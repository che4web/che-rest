use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env, fs,
    io::Write,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use che_orm::{
    Database, Migration, MigrationGraph, MigrationId, MigrationOperation, Model, SchemaSet,
    SqliteDialect, operations_from_changes, render_migration_rust, rusqlite::OptionalExtension,
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
        app: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "migrations")]
        dir: PathBuf,
        #[arg(long, default_value_t = false)]
        empty: bool,
        #[arg(long, default_value_t = false, conflicts_with = "empty")]
        check: bool,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Generate an empty migration that merges two compatible heads.
        #[arg(long, default_value_t = false, conflicts_with_all = ["empty", "check"])]
        merge: bool,
    },
    Migrate {
        #[command(subcommand)]
        action: Option<MigrateAction>,
        /// Optional target: application and migration name, or `latest`.
        #[arg(value_names = ["APP", "NAME"], num_args = 0..=2)]
        target: Vec<String>,
        /// Show pending compiled migrations without applying them.
        #[arg(long)]
        plan: bool,
        #[arg(long, default_value = "app.toml")]
        config: PathBuf,
        #[arg(long, default_value = "migrations")]
        dir: PathBuf,
    },
    /// Print SQL for one compiled migration without executing it.
    Sqlmigrate {
        app: String,
        name: String,
    },
    /// List compiled migrations, their dependencies and database status.
    Showmigrations {
        app: Option<String>,
        #[arg(long, default_value = "app.toml")]
        config: PathBuf,
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
    compiled_migrations: bool,
    project_root: PathBuf,
}

impl Management {
    pub fn new(apps: InstalledApps) -> Self {
        Self {
            apps,
            migrations: Vec::new(),
            compiled_migrations: false,
            project_root: PathBuf::from("."),
        }
    }

    pub fn project_root(mut self, project_root: impl Into<PathBuf>) -> Self {
        self.project_root = project_root.into();
        self
    }

    /// Selects compiled migrations, including when the registry is empty.
    /// Pass `migrations::all()` from the generated registry; after adding a
    /// file, rebuild the management binary to register it.
    pub fn migrations(mut self, migrations: Vec<Migration>) -> Self {
        self.migrations = migrations;
        self.compiled_migrations = true;
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
            CommandKind::Makemigrations {
                app,
                name,
                dir,
                empty,
                check,
                dry_run,
                merge,
            } => {
                if self.compiled_migrations {
                    if merge {
                        let Some(app) = app else {
                            return Err("compiled --merge requires an application name".into());
                        };
                        if self.apps.find(&app).is_none() {
                            return Err(format!("unknown installed application `{app}`").into());
                        }
                        ensure_compiled_registry_matches(&dir, &self.migrations, &app)?;
                        let label = name.unwrap_or_else(|| "merge".into());
                        let migration =
                            build_operation_merge_migration(&self.migrations, &app, &label)?;
                        if dry_run {
                            print!("{}", render_migration_rust(&migration));
                            return Ok(());
                        }
                        let path = write_operation_migration_and_register(&dir, &migration)?;
                        println!("wrote merge migration to {}", path.display());
                        return Ok(());
                    }
                    if app.is_none() {
                        if empty {
                            return Err("compiled --empty requires an application name".into());
                        }
                        if !self.migrations.is_empty() {
                            return Err("batch migration generation is only supported for an empty compiled history".into());
                        }
                        let label = name.unwrap_or_else(|| "initial".into());
                        let migrations = build_initial_cycle_batch(&self.apps, &label)?;
                        if migrations.is_empty() {
                            println!("No model changes detected.");
                            return Ok(());
                        }
                        if check {
                            return Err("model changes need initial migrations".into());
                        }
                        if dry_run {
                            for migration in &migrations {
                                print!("{}", render_migration_rust(migration));
                            }
                            return Ok(());
                        }
                        for migration in &migrations {
                            let path = dir.join(&migration.id.app);
                            write_operation_migration_and_register(&path, migration)?;
                        }
                        println!(
                            "wrote {} initial migration(s) under {}",
                            migrations.len(),
                            dir.display()
                        );
                        return Ok(());
                    }
                    let app = app.unwrap();
                    ensure_compiled_registry_matches(&dir, &self.migrations, &app)?;
                    let name = name.unwrap_or_else(|| generated_migration_label());
                    let migration = build_operation_migration(
                        &self.apps,
                        &self.migrations,
                        &app,
                        &name,
                        empty,
                    )?;
                    let Some(migration) = migration else {
                        println!("No model changes detected for {app}.");
                        return Ok(());
                    };
                    if check {
                        return Err(format!(
                            "model changes for {app} need migration {}",
                            migration.id.name
                        )
                        .into());
                    }
                    if dry_run {
                        print!("{}", render_migration_rust(&migration));
                        return Ok(());
                    }
                    let path = write_operation_migration_and_register(&dir, &migration)?;
                    println!("wrote operation migration to {}", path.display());
                    return Ok(());
                }
                let name = name.or(app).unwrap_or_else(generated_migration_label);
                if empty {
                    write_empty_migration(&dir, &name)?;
                } else {
                    atlas_diff(&self.apps, &dir, &name)?;
                }
            }
            CommandKind::Migrate {
                action,
                target,
                plan,
                config,
                dir,
            } => {
                if target.len() > 2 {
                    return Err("migrate accepts at most an app and migration name".into());
                }
                if !target.is_empty() && (plan || action.is_some()) {
                    return Err(
                        "a migration target cannot be combined with --plan or a migrate subcommand"
                            .into(),
                    );
                }
                if plan && action.is_some() {
                    return Err(
                        "migrate --plan cannot be combined with a migrate subcommand".into(),
                    );
                }
                if plan && !self.compiled_migrations {
                    return Err("migrate --plan requires compiled operation migrations".into());
                }
                let config = AppConfig::from_file(config)?;
                if plan {
                    let result = tokio::task::spawn_blocking(move || {
                        operation_migration_plan(&config, self.migrations)
                    })
                    .await?
                    .map_err(|error| error as Box<dyn std::error::Error>)?;
                    print!("{result}");
                    return Ok(());
                }
                if let Some(app) = target.first() {
                    if !self.compiled_migrations {
                        return Err(
                            "migrate <app> [name|latest] requires compiled operation migrations"
                                .into(),
                        );
                    }
                    let target = resolve_operation_migration_target(
                        &self.migrations,
                        app,
                        target.get(1).map(String::as_str),
                    )?;
                    apply_operation_migrations_to(&config, self.migrations, target).await?;
                    return Ok(());
                }
                let action = action.unwrap_or(if !self.compiled_migrations {
                    MigrateAction::Apply
                } else {
                    MigrateAction::OperationsApply
                });
                match action {
                    MigrateAction::Apply => {
                        if !self.compiled_migrations {
                            apply_migrations(&config, dir).await?;
                        } else {
                            apply_operation_migrations(&config, self.migrations).await?;
                        }
                    }
                    MigrateAction::Status => {
                        if !self.compiled_migrations {
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
                        if !self.compiled_migrations {
                            verify_migrations(&config, dir).await?;
                        } else {
                            verify_operation_migrations(&config, self.migrations).await?;
                        }
                    }
                    MigrateAction::Lint | MigrateAction::Diff { .. }
                        if self.compiled_migrations =>
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
            CommandKind::Sqlmigrate { app, name } => {
                if !self.compiled_migrations {
                    return Err("sqlmigrate requires compiled operation migrations".into());
                }
                print!(
                    "{}",
                    operation_migration_sql(&self.migrations, &app, &name)?
                );
            }
            CommandKind::Showmigrations { app, config } => {
                if !self.compiled_migrations {
                    return Err("showmigrations requires compiled operation migrations".into());
                }
                let config = AppConfig::from_file(config)?;
                let result = tokio::task::spawn_blocking(move || {
                    operation_migration_list(&config, self.migrations, app.as_deref())
                })
                .await?
                .map_err(|error| error as Box<dyn std::error::Error>)?;
                print!("{result}");
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

/// Builds a set of initial migrations for all installed apps. Cross-app foreign
/// keys are deferred to a second migration per source app, so mutually
/// referencing models never form a dependency cycle.
pub fn build_initial_cycle_batch(
    apps: &InstalledApps,
    label: &str,
) -> ManageResult<Vec<Migration>> {
    validate_operation_migration_name(label)?;
    let desired = apps.migration_state()?;
    let models = desired.models();
    let owners: BTreeMap<_, _> = models
        .iter()
        .map(|model| (model.table.name.clone(), model.key.app.clone()))
        .collect();
    let mut by_app: BTreeMap<String, Vec<che_orm::ModelState>> = BTreeMap::new();
    let mut deferred: BTreeMap<
        String,
        Vec<(String, che_orm::ColumnState, che_orm::ColumnState, String)>,
    > = BTreeMap::new();
    for mut model in models {
        for column in &mut model.table.columns {
            let original = column.clone();
            let Some(reference) = &column.references else {
                continue;
            };
            let Some((target, _)) = reference.target.split_once('(') else {
                continue;
            };
            let Some(target_app) = owners.get(target) else {
                continue;
            };
            if target_app != &model.key.app {
                column.references = None;
                deferred.entry(model.key.app.clone()).or_default().push((
                    model.table.name.clone(),
                    column.clone(),
                    original,
                    target_app.clone(),
                ));
            }
        }
        by_app.entry(model.key.app.clone()).or_default().push(model);
    }
    let mut initial_ids = BTreeMap::new();
    let mut migrations = Vec::new();
    for (app, models) in &by_app {
        let id = MigrationId {
            app: app.clone(),
            name: format!("0001_{label}"),
        };
        initial_ids.insert(app.clone(), id.clone());
        migrations.push(Migration::new(
            id,
            vec![],
            models
                .iter()
                .cloned()
                .map(|model| MigrationOperation::CreateModel { model })
                .collect(),
        ));
    }
    for (app, columns) in deferred {
        let mut dependencies = BTreeSet::from([initial_ids[&app].clone()]);
        for (_, _, _, target_app) in &columns {
            dependencies.insert(initial_ids[target_app].clone());
        }
        migrations.push(Migration::new(
            MigrationId {
                app: app.clone(),
                name: "0002_relationships".into(),
            },
            dependencies.into_iter().collect(),
            columns
                .into_iter()
                .map(
                    |(table, old_column, new_column, _)| MigrationOperation::AlterColumn {
                        table,
                        old_column,
                        new_column,
                    },
                )
                .collect(),
        ));
    }
    MigrationGraph::new(migrations.clone())?.replay(Default::default())?;
    Ok(migrations)
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

/// Writes a generated, compiled migration without ever replacing an existing
/// file. Updating the owning `mod.rs` is intentionally a separate operation:
/// callers can inspect this source before it becomes part of the binary.
pub fn write_operation_migration(dir: &PathBuf, migration: &Migration) -> ManageResult<PathBuf> {
    validate_operation_migration_name(&migration.id.name)?;
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("m{}.rs", migration.id.name));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    if let Err(error) = file
        .write_all(render_migration_rust(migration).as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }
    Ok(path)
}

/// Creates an operation migration for one installed application by comparing
/// the compiled historical state with the currently compiled model metadata.
/// It intentionally never opens the project database.
pub fn build_operation_migration(
    apps: &InstalledApps,
    migrations: &[Migration],
    app: &str,
    label: &str,
    empty: bool,
) -> ManageResult<Option<Migration>> {
    validate_operation_migration_name(label)?;
    if apps.find(app).is_none() {
        return Err(format!("unknown installed application `{app}`").into());
    }
    let graph = MigrationGraph::new(migrations.to_vec())?;
    graph.plan_forward(&BTreeSet::new(), None)?;
    let history = graph.replay(Default::default())?;
    let desired = apps.migration_state()?;
    let changes: Vec<_> = if empty {
        vec![]
    } else {
        history
            .diff(&desired)?
            .into_iter()
            .filter(|change| change.app() == app)
            .collect()
    };
    if changes.is_empty() && !empty {
        return Ok(None);
    }
    let number = next_operation_migration_number(&graph, app)?;
    let mut dependencies: BTreeSet<_> = graph
        .app_heads(app)
        .into_iter()
        .map(|migration| migration.id.clone())
        .collect();
    // Generation uses the complete replayed state, including models introduced
    // by a shared baseline under a different app label. Depend on the heads of
    // that history so replay of this migration's dependency closure has the
    // same starting state as the generator.
    for owner in migrations
        .iter()
        .map(|migration| &migration.id.app)
        .collect::<BTreeSet<_>>()
    {
        dependencies.extend(
            graph
                .app_heads(owner)
                .into_iter()
                .map(|migration| migration.id.clone()),
        );
    }
    let table_apps: BTreeMap<_, _> = desired
        .models()
        .into_iter()
        .map(|model| (model.table.name, model.key.app))
        .collect();
    validate_generated_references(&changes, &history, &desired, &table_apps, app)?;
    for referenced_app in referenced_apps(&changes, &table_apps, app) {
        dependencies.extend(
            graph
                .app_heads(&referenced_app)
                .into_iter()
                .map(|migration| migration.id.clone()),
        );
    }
    let operations = order_generated_operations(operations_from_changes(changes), &history)?;
    if !empty {
        let mut resulting = history.clone();
        for operation in &operations {
            operation.state_forwards(&mut resulting)?;
        }
        if resulting
            .diff(&desired)?
            .iter()
            .any(|change| change.app() == app)
        {
            return Err("generated operations do not reproduce the desired model state; explicit migration required".into());
        }
    }
    Ok(Some(Migration::new(
        MigrationId {
            app: app.into(),
            name: format!("{number:04}_{label}"),
        },
        dependencies.into_iter().collect(),
        operations,
    )))
}

/// Builds an empty merge migration for exactly two branches whose declarative
/// state transitions commute. `RunSql` branches require an explicit manual
/// merge because their database effects cannot be proven from state metadata.
pub fn build_operation_merge_migration(
    migrations: &[Migration],
    app: &str,
    label: &str,
) -> ManageResult<Migration> {
    validate_operation_migration_name(label)?;
    let graph = MigrationGraph::new(migrations.to_vec())?;
    let heads = graph.app_heads(app);
    if heads.len() < 2 {
        return Err(format!("{app} has no conflicting migration heads to merge").into());
    }
    if heads.len() > 2 {
        return Err(format!(
            "{app} has {} migration heads; merge them manually in smaller reviewed steps",
            heads.len()
        )
        .into());
    }
    let heads = heads
        .into_iter()
        .map(|migration| migration.id.clone())
        .collect::<Vec<_>>();
    let left = migration_closure(&graph, &heads[0]);
    let right = migration_closure(&graph, &heads[1]);
    let common = left.intersection(&right).cloned().collect::<BTreeSet<_>>();
    let left_branch = left.difference(&common).cloned().collect::<BTreeSet<_>>();
    let right_branch = right.difference(&common).cloned().collect::<BTreeSet<_>>();

    for migration in graph.ordered().filter(|migration| {
        left_branch.contains(&migration.id) || right_branch.contains(&migration.id)
    }) {
        if migration
            .operations
            .iter()
            .any(|operation| matches!(operation, che_orm::MigrationOperation::RunSql { .. }))
        {
            return Err(format!(
                "cannot prove merge compatibility for {}:{} because it contains RunSql; write an explicit reviewed merge migration",
                migration.id.app, migration.id.name
            )
            .into());
        }
    }

    let base = graph.replay_selected(Default::default(), &common)?;
    let left_then_right = replay_migration_branch(&graph, base.clone(), &left_branch)
        .and_then(|state| replay_migration_branch(&graph, state, &right_branch));
    let right_then_left = replay_migration_branch(&graph, base, &right_branch)
        .and_then(|state| replay_migration_branch(&graph, state, &left_branch));
    match (left_then_right, right_then_left) {
        (Ok(left), Ok(right)) if migration_states_equivalent(&left, &right) => {}
        (Ok(_), Ok(_)) => {
            return Err("migration branches produce different historical states; write an explicit reviewed merge migration".into());
        }
        (Err(left), Err(right)) => {
            return Err(format!(
                "migration branches are not compatible ({left}; {right}); write an explicit reviewed merge migration"
            )
            .into());
        }
        (Err(error), _) | (_, Err(error)) => {
            return Err(format!(
                "migration branches are not compatible ({error}); write an explicit reviewed merge migration"
            )
            .into());
        }
    }
    let number = next_operation_migration_number(&graph, app)?;
    Ok(Migration::new(
        MigrationId {
            app: app.into(),
            name: format!("{number:04}_{label}"),
        },
        heads,
        vec![],
    ))
}

fn migration_states_equivalent(
    left: &che_orm::ProjectState,
    right: &che_orm::ProjectState,
) -> bool {
    fn normalize_table(table: &mut che_orm::TableState) {
        table
            .columns
            .sort_by(|left, right| left.name.cmp(&right.name));
        table.indexes.sort();
        table.unique_constraints.sort();
    }
    let mut left_tables = left.tables().to_vec();
    let mut right_tables = right.tables().to_vec();
    for table in &mut left_tables {
        normalize_table(table);
    }
    for table in &mut right_tables {
        normalize_table(table);
    }
    left_tables.sort_by(|left, right| left.name.cmp(&right.name));
    right_tables.sort_by(|left, right| left.name.cmp(&right.name));
    if left_tables != right_tables {
        return false;
    }
    let mut left_models = left.models();
    let mut right_models = right.models();
    for model in &mut left_models {
        normalize_table(&mut model.table);
    }
    for model in &mut right_models {
        normalize_table(&mut model.table);
    }
    left_models.sort_by(|left, right| left.key.cmp(&right.key));
    right_models.sort_by(|left, right| left.key.cmp(&right.key));
    left_models == right_models
}

fn migration_closure(graph: &MigrationGraph, target: &MigrationId) -> BTreeSet<MigrationId> {
    let migrations = graph
        .ordered()
        .map(|migration| (migration.id.clone(), migration))
        .collect::<BTreeMap<_, _>>();
    let mut closure = BTreeSet::new();
    let mut pending = vec![target.clone()];
    while let Some(id) = pending.pop() {
        if closure.insert(id.clone()) {
            pending.extend(migrations[&id].dependencies.iter().cloned());
        }
    }
    closure
}

fn replay_migration_branch(
    graph: &MigrationGraph,
    mut state: che_orm::ProjectState,
    branch: &BTreeSet<MigrationId>,
) -> Result<che_orm::ProjectState, che_orm::MigrationError> {
    for migration in graph
        .ordered()
        .filter(|migration| branch.contains(&migration.id))
    {
        for operation in &migration.operations {
            operation.state_forwards(&mut state)?;
            state.validate()?;
        }
    }
    Ok(state)
}

fn order_generated_operations(
    mut pending: Vec<che_orm::MigrationOperation>,
    history: &che_orm::ProjectState,
) -> ManageResult<Vec<che_orm::MigrationOperation>> {
    let mut state = history.clone();
    let mut ordered = Vec::new();
    while !pending.is_empty() {
        let mut ready = None;
        let mut last_error = String::new();
        for (index, operation) in pending.iter().enumerate() {
            let mut candidate = state.clone();
            match operation
                .state_forwards(&mut candidate)
                .and_then(|_| candidate.validate())
            {
                Ok(()) => {
                    ready = Some((index, candidate));
                    break;
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        let Some((index, candidate)) = ready else {
            return Err(format!(
                "cannot order generated operations; explicit migration required: {last_error}"
            )
            .into());
        };
        state = candidate;
        ordered.push(pending.remove(index));
    }
    Ok(ordered)
}

fn validate_generated_references(
    changes: &[che_orm::StateChange],
    history: &che_orm::ProjectState,
    desired: &che_orm::ProjectState,
    table_apps: &BTreeMap<String, String>,
    app: &str,
) -> ManageResult<()> {
    for change in changes {
        let columns = match change {
            che_orm::StateChange::CreateModel(model) => {
                model.table.columns.iter().collect::<Vec<_>>()
            }
            che_orm::StateChange::AddColumn { column, .. } => vec![column],
            che_orm::StateChange::AlterColumn { new_column, .. } => vec![new_column],
            _ => vec![],
        };
        for reference in columns
            .into_iter()
            .filter_map(|column| column.references.as_ref())
        {
            let (table, field) = reference
                .target
                .split_once('(')
                .ok_or("invalid foreign key")?;
            let field = field.strip_suffix(')').ok_or("invalid foreign key")?;
            let Some(owner) = table_apps.get(table) else {
                continue;
            };
            let source = if owner == app { desired } else { history };
            let target_exists = source.models().iter().any(|model| {
                model.key.app == *owner
                    && model.table.name == table
                    && model
                        .table
                        .columns
                        .iter()
                        .any(|column| column.name == field)
            });
            if !target_exists {
                return Err(format!("foreign key {} needs a migration for application {owner}; generate and register it first", reference.target).into());
            }
        }
    }
    Ok(())
}

fn referenced_apps(
    changes: &[che_orm::StateChange],
    table_apps: &BTreeMap<String, String>,
    current_app: &str,
) -> BTreeSet<String> {
    let columns = changes.iter().flat_map(|change| match change {
        che_orm::StateChange::CreateModel(model) => model.table.columns.iter().collect::<Vec<_>>(),
        che_orm::StateChange::AddColumn { column, .. } => vec![column],
        che_orm::StateChange::AlterColumn { new_column, .. } => vec![new_column],
        _ => vec![],
    });
    columns
        .filter_map(|column| column.references.as_ref())
        .filter_map(|reference| reference.target.split_once('(').map(|(table, _)| table))
        .filter_map(|table| table_apps.get(table))
        .filter(|app| app.as_str() != current_app)
        .cloned()
        .collect()
}

/// Writes a new migration then adds its module to an explicit managed section
/// of `mod.rs`. Existing handwritten content outside the markers is preserved.
pub fn write_operation_migration_and_register(
    dir: &PathBuf,
    migration: &Migration,
) -> ManageResult<PathBuf> {
    validate_operation_migration_name(&migration.id.name)?;
    fs::create_dir_all(dir)?;
    let lock_path = dir.join(".che-migrations.lock");
    let _lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot lock migration directory {}: {error}; another writer may be active",
                dir.display()
            )
        })?;
    let _lock = RemoveOnDrop(lock_path);
    let module = format!("m{}", migration.id.name);
    let registry = rendered_registry_update(&dir.join("mod.rs"), &module)?;
    let path = write_operation_migration(dir, migration)?;
    let temporary = dir.join(".che-mod.rs.tmp");
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
    };
    let _temporary = RemoveOnDrop(temporary.clone());
    if let Err(error) = file
        .write_all(registry.as_bytes())
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temporary, dir.join("mod.rs")))
    {
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }
    Ok(path)
}

struct RemoveOnDrop(PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

const REGISTRY_BEGIN: &str = "// che-orm: migrations begin";
const REGISTRY_END: &str = "// che-orm: migrations end";

fn rendered_registry_update(path: &std::path::Path, module: &str) -> ManageResult<String> {
    let current = if path.exists() {
        fs::read_to_string(path)?
    } else {
        format!("{REGISTRY_BEGIN}\n{REGISTRY_END}\n")
    };
    let start = current
        .find(REGISTRY_BEGIN)
        .ok_or("migration registry is missing its managed begin marker")?;
    let end = current
        .find(REGISTRY_END)
        .ok_or("migration registry is missing its managed end marker")?;
    if end < start {
        return Err("migration registry markers are in the wrong order".into());
    }
    let prefix_end = start + REGISTRY_BEGIN.len();
    let modules = current[prefix_end..end]
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub mod "))
        .filter_map(|line| line.strip_suffix(';'))
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let mut modules = modules;
    if !modules.insert(module.into()) {
        return Err(format!("migration module `{module}` is already registered").into());
    }
    let calls = modules
        .iter()
        .map(|module| format!("{module}::migration()"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = modules
        .into_iter()
        .map(|module| format!("pub mod {module};"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "{}\n{}\npub fn all() -> Vec<che_orm::Migration> {{ vec![{}] }}\n{}{}",
        &current[..prefix_end],
        body,
        calls,
        REGISTRY_END,
        &current[end + REGISTRY_END.len()..]
    ))
}

fn next_operation_migration_number(graph: &MigrationGraph, app: &str) -> ManageResult<u32> {
    graph
        .ordered()
        .filter(|migration| migration.id.app == app)
        .map(|migration| {
            migration
                .id
                .name
                .split_once('_')
                .map(|(number, _)| number)
                .unwrap_or(&migration.id.name)
                .parse::<u32>()
                .map_err(|_| {
                    format!("migration {} has no numeric prefix", migration.id.name).into()
                })
        })
        .collect::<ManageResult<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| "migration number overflow".into())
}

fn generated_migration_label() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("auto_{seconds}")
}

/// Ensures the source files in one migration directory describe exactly the
/// migrations compiled into this management binary. This prevents a binary
/// built before the last `makemigrations` call from generating the same diff
/// again and overwriting history with a parallel migration.
fn ensure_compiled_registry_matches(
    dir: &PathBuf,
    migrations: &[Migration],
    app: &str,
) -> ManageResult<()> {
    let expected: BTreeSet<_> = migrations
        .iter()
        .filter(|migration| migration.id.app == app)
        .map(|migration| format!("m{}.rs", migration.id.name))
        .collect();
    let actual = if dir.exists() {
        fs::read_dir(dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                let name = entry.file_name().into_string().ok()?;
                (kind.is_file()
                    && name.strip_prefix('m').is_some_and(|suffix| {
                        suffix.as_bytes().first().is_some_and(u8::is_ascii_digit)
                    })
                    && name.ends_with(".rs"))
                .then_some(name)
            })
            .collect()
    } else {
        BTreeSet::new()
    };
    if actual != expected {
        return Err(format!(
            "compiled migration registry for {app} differs from {}. Rebuild the management binary after makemigrations before generating another migration",
            dir.display()
        )
        .into());
    }
    Ok(())
}

fn validate_operation_migration_name(name: &str) -> ManageResult<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(
            "migration name must contain only ascii letters, digits, and underscores".into(),
        );
    }
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

async fn apply_operation_migrations_to(
    config: &AppConfig,
    migrations: Vec<Migration>,
    target: MigrationId,
) -> ManageResult<()> {
    let applied_count = run_operation_migrations_to(config, migrations, target.clone()).await?;
    if applied_count == 0 {
        println!("Target {}:{} is already applied.", target.app, target.name);
    } else {
        println!(
            "Applied {applied_count} operation migration(s) through {}:{}.",
            target.app, target.name
        );
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

async fn run_operation_migrations_to(
    config: &AppConfig,
    migrations: Vec<Migration>,
    target: MigrationId,
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
        .interact(move |connection| {
            apply_operation_migrations_to_on_connection(connection, graph, Some(target))
        })
        .await
        .map_err(|error| format!("database interaction error: {error}"))?;
    result.map_err(|error| error as Box<dyn std::error::Error>)
}

use che_orm::migration::sqlite_executor::{
    applied_operation_migrations, apply_operation_migrations_on_connection,
    apply_operation_migrations_to_on_connection, pending_operation_migrations,
};

fn resolve_operation_migration_target(
    migrations: &[Migration],
    app: &str,
    name: Option<&str>,
) -> ManageResult<MigrationId> {
    let graph = MigrationGraph::new(migrations.to_vec())
        .map_err(|error| format!("invalid operation migration graph: {error}"))?;
    let name = name.unwrap_or("latest");
    if name == "latest" {
        return graph
            .app_heads(app)
            .into_iter()
            .next()
            .map(|migration| migration.id.clone())
            .ok_or_else(|| format!("no registered operation migrations for {app}").into());
    }
    let target = MigrationId {
        app: app.to_owned(),
        name: name.to_owned(),
    };
    if graph.ordered().any(|migration| migration.id == target) {
        Ok(target)
    } else {
        Err(format!("unknown operation migration {app}:{name}").into())
    }
}

fn operation_migration_plan(
    config: &AppConfig,
    migrations: Vec<Migration>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let graph = MigrationGraph::new(migrations)?;
    let applied = read_operation_migration_history_read_only(config, "migrate --plan")?;
    let pending = pending_operation_migrations(&graph, &applied)?;
    let applied_ids = applied
        .keys()
        .map(|(app, name)| MigrationId {
            app: app.clone(),
            name: name.clone(),
        })
        .collect();
    // Replay historical state as well as checking checksums and dependencies.
    graph.plan_forward(&applied_ids, None)?;
    let mut output = format!(
        "Forward migration plan: {} pending migration(s).\n",
        pending.len()
    );
    for migration in pending {
        output.push_str(&format!(
            "\npending {}:{}\n",
            migration.id.app, migration.id.name
        ));
        let dependencies = migration
            .dependencies
            .iter()
            .map(|id| format!("{}:{}", id.app, id.name))
            .collect::<Vec<_>>();
        output.push_str(&format!(
            "  Dependencies: {}\n",
            if dependencies.is_empty() {
                "none".to_string()
            } else {
                dependencies.join(", ")
            }
        ));
        if migration.operations.is_empty() {
            output.push_str("  (no operations)\n");
        }
        for operation in &migration.operations {
            output.push_str(&format!("  {}\n", describe_migration_operation(operation)));
        }
    }
    output.push_str("\nPreview only: no migrations applied. SQL and live-schema compatibility are checked on apply.\n");
    Ok(output)
}

fn read_operation_migration_history_read_only(
    config: &AppConfig,
    command: &str,
) -> Result<HashMap<(String, String), String>, Box<dyn std::error::Error + Send + Sync>> {
    use che_orm::rusqlite::{Connection, OpenFlags};

    let path = sqlite_path(&config.database.url);
    if path.is_empty()
        || path == ":memory:"
        || path.starts_with("file:")
        || path.contains("://")
        || config
            .database
            .url
            .split_once('?')
            .is_some_and(|(_, query)| query.split('&').any(|item| item == "mode=memory"))
    {
        return Err(format!("{command} requires a SQLite file path (plain or sqlite://)").into());
    }
    // Never use the regular pool here: it opens with CREATE and may initialize SQLite.
    match fs::metadata(&path) {
        Ok(_) => {
            let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            connection.execute_batch("BEGIN")?;
            let applied = applied_operation_migrations(&connection)?;
            connection.execute_batch("ROLLBACK")?;
            Ok(applied)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn operation_migration_list(
    config: &AppConfig,
    migrations: Vec<Migration>,
    app_filter: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let graph = MigrationGraph::new(migrations)?;
    let applied = read_operation_migration_history_read_only(config, "showmigrations")?;
    pending_operation_migrations(&graph, &applied)?;
    let migrations = graph
        .ordered()
        .filter(|migration| app_filter.is_none_or(|app| migration.id.app == app))
        .collect::<Vec<_>>();
    if migrations.is_empty() {
        return Ok(match app_filter {
            Some(app) => format!("No registered migrations for {app}.\n"),
            None => "No registered migrations.\n".to_string(),
        });
    }
    let mut output = String::new();
    for migration in migrations {
        let marker = if applied.contains_key(&(migration.id.app.clone(), migration.id.name.clone()))
        {
            "[X]"
        } else {
            "[ ]"
        };
        output.push_str(&format!(
            "{marker} {}:{}\n",
            migration.id.app, migration.id.name
        ));
        if !migration.dependencies.is_empty() {
            output.push_str(&format!(
                "    depends on: {}\n",
                migration
                    .dependencies
                    .iter()
                    .map(|id| format!("{}:{}", id.app, id.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(output)
}

fn describe_migration_operation(operation: &MigrationOperation) -> String {
    use MigrationOperation::*;
    match operation {
        CreateModel { model } => format!(
            "CreateModel {}:{} (table {})",
            model.key.app, model.key.name, model.table.name
        ),
        CreateTable { table } => format!("CreateTable {}", table.name),
        DropTable { name } => format!("DropTable {name} — WARNING: deletes all table data"),
        AddColumn { table, column } => format!("AddColumn {table}.{}", column.name),
        RemoveColumn { table, column } => {
            format!("RemoveColumn {table}.{column} — WARNING: deletes all column data")
        }
        RenameTable { old_name, new_name } => format!("RenameTable {old_name} -> {new_name}"),
        RenameColumn {
            table,
            old_name,
            new_name,
        } => format!("RenameColumn {table}.{old_name} -> {new_name}"),
        AlterColumn {
            table,
            old_column,
            new_column,
        } => format!(
            "AlterColumn {table}.{} -> {} — WARNING: review type, constraints and data compatibility",
            old_column.name, new_column.name
        ),
        AddIndex { table, columns } => format!("AddIndex {table} ({})", columns.join(", ")),
        RemoveIndex { table, columns } => format!("RemoveIndex {table} ({})", columns.join(", ")),
        AddUniqueConstraint { table, columns } => {
            format!("AddUniqueConstraint {table} ({})", columns.join(", "))
        }
        RemoveUniqueConstraint { table, columns } => {
            format!("RemoveUniqueConstraint {table} ({})", columns.join(", "))
        }
        RunSql {
            state_operations, ..
        } => format!(
            "RunSql ({} state operation(s)) — WARNING: manual SQL may change or delete data; inspect migration source",
            state_operations.len()
        ),
    }
}

fn operation_migration_sql(
    migrations: &[Migration],
    app: &str,
    name: &str,
) -> ManageResult<String> {
    let graph = MigrationGraph::new(migrations.to_vec())
        .map_err(|error| format!("invalid operation migration graph: {error}"))?;
    let target_id = MigrationId {
        app: app.to_owned(),
        name: name.to_owned(),
    };
    let migrations_by_id = graph
        .ordered()
        .map(|migration| (migration.id.clone(), migration))
        .collect::<HashMap<_, _>>();
    let target = migrations_by_id.get(&target_id).ok_or_else(|| {
        format!("unknown operation migration {app}:{name}; use its registered app and name")
    })?;
    let mut required = BTreeSet::new();
    let mut pending = target.dependencies.clone();
    while let Some(id) = pending.pop() {
        if required.insert(id.clone()) {
            pending.extend(
                migrations_by_id
                    .get(&id)
                    .expect("graph dependencies were validated")
                    .dependencies
                    .iter()
                    .cloned(),
            );
        }
    }

    let mut state = che_orm::ProjectState::default();
    for migration in graph
        .ordered()
        .filter(|migration| required.contains(&migration.id))
    {
        for operation in &migration.operations {
            operation.state_forwards(&mut state)?;
        }
    }

    let mut output = format!("-- {}:{}\n", target.id.app, target.id.name);
    if target.operations.is_empty() {
        output.push_str("-- This migration has no SQL operations.\n");
    }
    for operation in &target.operations {
        let rendered = che_orm::SqliteSchemaEditor::plan(operation, &state)?;
        for statement in rendered.statements {
            output.push_str(&statement);
            if !statement.trim_end().ends_with(';') {
                output.push(';');
            }
            output.push('\n');
        }
        state = rendered.resulting_state;
    }
    Ok(output)
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
        collections::HashMap,
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use che_orm::{
        Database, Migration, MigrationId, MigrationOperation, Model, rusqlite::OptionalExtension,
    };
    use clap::Parser;

    use super::{
        Cli, Management, apply_migrations, apply_operation_migrations, build_operation_migration,
        create_empty_migration, migration_status, operation_migration_status,
        pending_operation_migrations, write_operation_migration,
        write_operation_migration_and_register,
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

    fn plan_test_migrations() -> Vec<Migration> {
        let initial = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![MigrationOperation::CreateTable {
                table: che_orm::TableState {
                    name: "items".into(),
                    columns: vec![che_orm::ColumnState {
                        field_name: "id".into(),
                        name: "id".into(),
                        column_type: che_orm::ColumnType::Integer,
                        nullable: false,
                        primary_key: true,
                        unique: false,
                        default: None,
                        check: None,
                        choices: None,
                        references: None,
                        auto_now_add: false,
                        auto_now: false,
                    }],
                    indexes: vec![],
                    unique_constraints: vec![],
                },
            }],
        );
        let remove = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0002_remove".into(),
            },
            vec![initial.id.clone()],
            vec![MigrationOperation::DropTable {
                name: "items".into(),
            }],
        );
        vec![initial, remove]
    }

    fn merge_test_migrations() -> Vec<Migration> {
        let initial = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![MigrationOperation::CreateTable {
                table: che_orm::TableState {
                    name: "items".into(),
                    columns: vec![che_orm::ColumnState {
                        field_name: "id".into(),
                        name: "id".into(),
                        column_type: che_orm::ColumnType::Integer,
                        nullable: false,
                        primary_key: true,
                        unique: false,
                        default: None,
                        check: None,
                        choices: None,
                        references: None,
                        auto_now_add: false,
                        auto_now: false,
                    }],
                    indexes: vec![],
                    unique_constraints: vec![],
                },
            }],
        );
        let initial_id = initial.id.clone();
        let branch = |name: &str, column: &str| {
            Migration::new(
                MigrationId {
                    app: "tasks".into(),
                    name: name.into(),
                },
                vec![initial_id.clone()],
                vec![MigrationOperation::AddColumn {
                    table: "items".into(),
                    column: che_orm::ColumnState {
                        field_name: column.into(),
                        name: column.into(),
                        column_type: che_orm::ColumnType::Text,
                        nullable: true,
                        primary_key: false,
                        unique: false,
                        default: None,
                        check: None,
                        choices: None,
                        references: None,
                        auto_now_add: false,
                        auto_now: false,
                    },
                }],
            )
        };
        vec![
            initial,
            branch("0002_add_description", "description"),
            branch("0002_add_title", "title"),
        ]
    }

    #[test]
    fn merge_generation_requires_two_compatible_declarative_heads() {
        Cli::try_parse_from(["manage", "makemigrations", "tasks", "--merge"]).unwrap();
        let migrations = merge_test_migrations();
        let merge = super::build_operation_merge_migration(&migrations, "tasks", "merge").unwrap();
        assert_eq!(merge.id.name, "0003_merge");
        assert!(merge.operations.is_empty());
        assert_eq!(
            merge
                .dependencies
                .iter()
                .map(|id| id.name.as_str())
                .collect::<Vec<_>>(),
            ["0002_add_description", "0002_add_title"]
        );

        let mut conflicting = merge_test_migrations();
        conflicting[2].operations = conflicting[1].operations.clone();
        conflicting[2].checksum = conflicting[2].calculated_checksum();
        assert!(
            super::build_operation_merge_migration(&conflicting, "tasks", "merge")
                .unwrap_err()
                .to_string()
                .contains("not compatible")
        );
    }

    #[test]
    fn merge_generation_rejects_manual_sql_and_non_branches() {
        let mut migrations = merge_test_migrations();
        migrations[1].operations = vec![MigrationOperation::RunSql {
            forward: "SELECT 1".into(),
            reverse: None,
            state_operations: migrations[1].operations.clone(),
        }];
        migrations[1].checksum = migrations[1].calculated_checksum();
        assert!(
            super::build_operation_merge_migration(&migrations, "tasks", "merge")
                .unwrap_err()
                .to_string()
                .contains("contains RunSql")
        );
        assert!(
            super::build_operation_merge_migration(&plan_test_migrations(), "tasks", "merge")
                .unwrap_err()
                .to_string()
                .contains("no conflicting")
        );
    }

    #[test]
    fn migration_plan_does_not_create_database() {
        let path = temp_path("plan_missing", "sqlite");
        let plan =
            super::operation_migration_plan(&sqlite_config(&path), plan_test_migrations()).unwrap();
        assert!(plan.contains("2 pending migration(s)"));
        assert!(plan.contains("Dependencies: tasks:0001_initial"));
        assert!(plan.contains("DropTable items — WARNING: deletes all table data"));
        assert!(
            plan.find("pending tasks:0001").unwrap() < plan.find("pending tasks:0002").unwrap()
        );
        assert!(!path.exists());
    }

    #[test]
    fn migration_plan_reads_history_without_changing_database() {
        let path = temp_path("plan_history", "sqlite");
        let migrations = plan_test_migrations();
        let connection = che_orm::rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE items (id INTEGER); INSERT INTO items VALUES (42);
            CREATE TABLE che_migration_history (app TEXT, name TEXT, checksum TEXT);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO che_migration_history VALUES (?1, ?2, ?3)",
                [
                    &migrations[0].id.app,
                    &migrations[0].id.name,
                    &migrations[0].checksum,
                ],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        let plan =
            super::operation_migration_plan(&sqlite_config(&path), migrations.clone()).unwrap();
        assert!(plan.contains("1 pending migration(s)"));
        assert!(!plan.contains("pending tasks:0001_initial"));
        assert!(plan.contains("pending tasks:0002_remove"));
        assert_eq!(before, fs::read(&path).unwrap());

        let connection = che_orm::rusqlite::Connection::open(&path).unwrap();
        connection
            .execute("UPDATE che_migration_history SET checksum = 'wrong'", [])
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(
            super::operation_migration_plan(&sqlite_config(&path), migrations)
                .unwrap_err()
                .to_string()
                .contains("divergent")
        );
        assert_eq!(before, fs::read(&path).unwrap());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_plan_leaves_database_without_history_unchanged() {
        let path = temp_path("plan_no_history", "sqlite");
        let connection = che_orm::rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE existing (id INTEGER);")
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        let plan = super::operation_migration_plan(&sqlite_config(&path), vec![]).unwrap();
        assert!(plan.contains("0 pending migration(s)"));
        assert_eq!(before, fs::read(&path).unwrap());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_plan_warns_about_column_removal_and_manual_sql() {
        let removal = super::describe_migration_operation(&MigrationOperation::RemoveColumn {
            table: "items".into(),
            column: "name".into(),
        });
        assert!(removal.contains("items.name — WARNING: deletes all column data"));
        let sql = super::describe_migration_operation(&MigrationOperation::RunSql {
            forward: "DELETE FROM items".into(),
            reverse: None,
            state_operations: vec![],
        });
        assert!(sql.contains("WARNING: manual SQL may change or delete data"));
    }

    #[test]
    fn migration_plan_rejects_inconsistent_and_unknown_history() {
        let path = temp_path("plan_invalid_history", "sqlite");
        let migrations = plan_test_migrations();
        let connection = che_orm::rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE che_migration_history (app TEXT, name TEXT, checksum TEXT);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO che_migration_history VALUES (?1, ?2, ?3)",
                [
                    &migrations[1].id.app,
                    &migrations[1].id.name,
                    &migrations[1].checksum,
                ],
            )
            .unwrap();
        let error =
            super::operation_migration_plan(&sqlite_config(&path), migrations.clone()).unwrap_err();
        assert!(error.to_string().contains("missing dependency"));
        connection
            .execute("UPDATE che_migration_history SET name = 'unknown'", [])
            .unwrap();
        let error = super::operation_migration_plan(&sqlite_config(&path), migrations).unwrap_err();
        assert!(error.to_string().contains("missing operation migration"));
        drop(connection);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_plan_rejects_conflicting_heads_and_invalid_state() {
        let path = temp_path("plan_invalid_graph", "sqlite");
        let mut migrations = plan_test_migrations();
        migrations.push(Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0002_other".into(),
            },
            vec![migrations[0].id.clone()],
            vec![],
        ));
        assert!(
            super::operation_migration_plan(&sqlite_config(&path), migrations)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        let invalid = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0001_invalid".into(),
            },
            vec![],
            vec![MigrationOperation::DropTable {
                name: "missing".into(),
            }],
        );
        assert!(super::operation_migration_plan(&sqlite_config(&path), vec![invalid]).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn migration_plan_propagates_invalid_database_errors() {
        let path = temp_path("plan_corrupt", "sqlite");
        fs::write(&path, b"This is not SQLite").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(super::operation_migration_plan(&sqlite_config(&path), vec![]).is_err());
        assert_eq!(before, fs::read(&path).unwrap());
        fs::remove_file(&path).unwrap();
        for url in [
            ":memory:",
            "sqlite://dev?mode=memory",
            "file:test.sqlite",
            "postgres://localhost/db",
        ] {
            let mut config = sqlite_config(&path);
            config.database.url = url.into();
            assert!(
                super::operation_migration_plan(&config, vec![])
                    .unwrap_err()
                    .to_string()
                    .contains("requires a SQLite file path")
            );
        }
    }

    #[test]
    fn showmigrations_lists_status_dependencies_and_does_not_create_database() {
        let path = temp_path("showmigrations_missing", "sqlite");
        let migrations = plan_test_migrations();
        let output =
            super::operation_migration_list(&sqlite_config(&path), migrations, None).unwrap();
        assert_eq!(
            output,
            "[ ] tasks:0001_initial\n[ ] tasks:0002_remove\n    depends on: tasks:0001_initial\n"
        );
        assert!(!path.exists());
    }

    #[test]
    fn showmigrations_filters_app_and_reads_history_without_writing() {
        let path = temp_path("showmigrations_history", "sqlite");
        let mut migrations = plan_test_migrations();
        let audit = Migration::new(
            MigrationId {
                app: "audit".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![],
        );
        let initial = migrations[0].clone();
        migrations.push(audit);
        let connection = che_orm::rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE che_migration_history (app TEXT, name TEXT, checksum TEXT);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO che_migration_history VALUES (?1, ?2, ?3)",
                [&initial.id.app, &initial.id.name, &initial.checksum],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        let output = super::operation_migration_list(
            &sqlite_config(&path),
            migrations.clone(),
            Some("tasks"),
        )
        .unwrap();
        assert_eq!(
            output,
            "[X] tasks:0001_initial\n[ ] tasks:0002_remove\n    depends on: tasks:0001_initial\n"
        );
        assert_eq!(before, fs::read(&path).unwrap());
        assert_eq!(
            super::operation_migration_list(&sqlite_config(&path), migrations, Some("missing"))
                .unwrap(),
            "No registered migrations for missing.\n"
        );
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn migrate_target_applies_closure_and_rejects_a_passed_target() {
        Cli::try_parse_from(["manage", "migrate", "tasks", "0002_remove"]).unwrap();
        Cli::try_parse_from(["manage", "migrate", "tasks", "latest"]).unwrap();
        let path = temp_path("migrate_target", "sqlite");
        let config = sqlite_config(&path);
        let migrations = plan_test_migrations();
        let first =
            super::resolve_operation_migration_target(&migrations, "tasks", Some("0001_initial"))
                .unwrap();
        assert_eq!(
            super::run_operation_migrations_to(&config, migrations.clone(), first)
                .await
                .unwrap(),
            1
        );
        let latest =
            super::resolve_operation_migration_target(&migrations, "tasks", Some("latest"))
                .unwrap();
        assert_eq!(latest.name, "0002_remove");
        assert_eq!(
            super::run_operation_migrations_to(&config, migrations.clone(), latest)
                .await
                .unwrap(),
            1
        );
        let rollback =
            super::resolve_operation_migration_target(&migrations, "tasks", Some("0001_initial"))
                .unwrap();
        assert!(
            super::run_operation_migrations_to(&config, migrations.clone(), rollback)
                .await
                .unwrap_err()
                .to_string()
                .contains("rollback is not supported")
        );
        assert!(
            super::resolve_operation_migration_target(&migrations, "tasks", Some("missing"))
                .unwrap_err()
                .to_string()
                .contains("unknown operation migration")
        );
        assert!(
            super::resolve_operation_migration_target(&migrations, "missing", None)
                .unwrap_err()
                .to_string()
                .contains("no registered operation migrations")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn sqlmigrate_renders_target_against_its_historical_dependencies() {
        let initial = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![MigrationOperation::CreateTable {
                table: che_orm::TableState {
                    name: "items".into(),
                    columns: vec![che_orm::ColumnState {
                        field_name: "id".into(),
                        name: "id".into(),
                        column_type: che_orm::ColumnType::Integer,
                        nullable: false,
                        primary_key: true,
                        unique: false,
                        default: None,
                        check: None,
                        choices: None,
                        references: None,
                        auto_now_add: false,
                        auto_now: false,
                    }],
                    indexes: vec![],
                    unique_constraints: vec![],
                },
            }],
        );
        let add_title = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0002_add_title".into(),
            },
            vec![initial.id.clone()],
            vec![MigrationOperation::AddColumn {
                table: "items".into(),
                column: che_orm::ColumnState {
                    field_name: "title".into(),
                    name: "title".into(),
                    column_type: che_orm::ColumnType::Text,
                    nullable: true,
                    primary_key: false,
                    unique: false,
                    default: None,
                    check: None,
                    choices: None,
                    references: None,
                    auto_now_add: false,
                    auto_now: false,
                },
            }],
        );
        let sql = super::operation_migration_sql(
            &[initial.clone(), add_title],
            "tasks",
            "0002_add_title",
        )
        .unwrap();
        assert!(sql.starts_with("-- tasks:0002_add_title\n"));
        assert!(sql.contains("CREATE TABLE \"__che_migration_new_items\""));
        assert!(sql.contains("\"id\" INTEGER PRIMARY KEY"));
        assert!(sql.contains("\"title\" TEXT"));
        assert!(sql.contains("INSERT INTO \"__che_migration_new_items\""));
        assert!(sql.ends_with(";\n"));

        let initial_sql =
            super::operation_migration_sql(&[initial], "tasks", "0001_initial").unwrap();
        assert!(initial_sql.contains("CREATE TABLE \"items\""));
        assert!(!initial_sql.contains("__che_migration"));
    }

    #[test]
    fn sqlmigrate_preserves_manual_sql_and_rejects_unknown_migration() {
        let migration = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0001_manual".into(),
            },
            vec![],
            vec![MigrationOperation::RunSql {
                forward: "CREATE TABLE notes (id INTEGER PRIMARY KEY)".into(),
                reverse: None,
                state_operations: vec![MigrationOperation::CreateTable {
                    table: che_orm::TableState {
                        name: "notes".into(),
                        columns: vec![],
                        indexes: vec![],
                        unique_constraints: vec![],
                    },
                }],
            }],
        );
        let sql = super::operation_migration_sql(&[migration], "tasks", "0001_manual").unwrap();
        assert!(sql.contains("CREATE TABLE notes (id INTEGER PRIMARY KEY);"));
        assert!(
            super::operation_migration_sql(&[], "tasks", "missing")
                .unwrap_err()
                .to_string()
                .contains("unknown operation migration tasks:missing")
        );
    }

    #[tokio::test]
    async fn migration_plan_cli_rejects_legacy_and_conflicting_actions() {
        let cli = Cli::try_parse_from(["manage", "migrate", "--plan"]).unwrap();
        let error = Management::new(InstalledApps::new())
            .run_from(cli)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires compiled"));
        let cli = Cli::try_parse_from(["manage", "migrate", "--plan", "apply"]).unwrap();
        let error = Management::new(InstalledApps::new())
            .migrations(vec![])
            .run_from(cli)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot be combined"));

        let cli = Cli::try_parse_from(["manage", "sqlmigrate", "tasks", "0001_initial"]).unwrap();
        let error = Management::new(InstalledApps::new())
            .run_from(cli)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires compiled"));

        let cli = Cli::try_parse_from(["manage", "showmigrations"]).unwrap();
        let error = Management::new(InstalledApps::new())
            .run_from(cli)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires compiled"));
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

    #[test]
    fn operation_writer_creates_inspectable_rust_source_without_overwriting() {
        let directory = temp_path("operation_writer", "dir");
        let migration = Migration::new(
            MigrationId {
                app: "tasks".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![MigrationOperation::RunSql {
                forward: "CREATE TABLE tasks (id INTEGER PRIMARY KEY);".into(),
                reverse: None,
                state_operations: vec![],
            }],
        );
        let path = write_operation_migration(&directory, &migration).unwrap();
        assert_eq!(path.file_name().unwrap(), "m0001_initial.rs");
        let source = fs::read_to_string(&path).unwrap();
        assert!(source.contains("Migration::new("));
        assert!(write_operation_migration(&directory, &migration).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn compiled_makemigrations_creates_the_next_number_and_registry_entry() {
        #[derive(Default)]
        struct EmptyApp;
        impl crate::AppModule for EmptyApp {
            fn name(&self) -> &'static str {
                "tasks"
            }

            fn schema(&self) -> che_orm::SchemaSet {
                che_orm::SchemaSet::new()
            }

            fn init(&self, _context: &mut crate::ModuleContext) {}
        }

        let apps = InstalledApps::new().add(EmptyApp);
        let first = build_operation_migration(&apps, &[], "tasks", "initial", true)
            .unwrap()
            .unwrap();
        assert_eq!(first.id.name, "0001_initial");
        let directory = temp_path("operation_registry", "dir");
        let path = write_operation_migration_and_register(&directory, &first).unwrap();
        assert!(path.exists());
        assert_eq!(
            fs::read_to_string(directory.join("mod.rs")).unwrap(),
            "// che-orm: migrations begin\npub mod m0001_initial;\npub fn all() -> Vec<che_orm::Migration> { vec![m0001_initial::migration()] }\n// che-orm: migrations end\n"
        );
        let second = build_operation_migration(&apps, &[first], "tasks", "manual", true)
            .unwrap()
            .unwrap();
        assert_eq!(second.id.name, "0002_manual");
        let _ = fs::remove_dir_all(directory);
    }

    #[derive(Debug, Model)]
    #[orm(table = "review_items")]
    struct ReviewItem {
        #[orm(primary_key)]
        id: i64,
    }

    struct ReviewApp;
    impl crate::AppModule for ReviewApp {
        fn name(&self) -> &'static str {
            "review"
        }
        fn schema(&self) -> che_orm::SchemaSet {
            che_orm::SchemaSet::new().model::<ReviewItem>()
        }
        fn init(&self, _: &mut crate::ModuleContext) {}
    }

    #[tokio::test]
    async fn generator_review_initial_cli_and_empty_and_branches() {
        let apps = || InstalledApps::new().add(ReviewApp);
        let empty = build_operation_migration(&apps(), &[], "review", "manual", true)
            .unwrap()
            .unwrap();
        assert!(empty.operations.is_empty());
        let directory = temp_path("review_cli", "dir");
        let cli = Cli::try_parse_from([
            "manage",
            "makemigrations",
            "review",
            "--name",
            "initial",
            "--dir",
            directory.to_str().unwrap(),
        ])
        .unwrap();
        Management::new(apps())
            .migrations(vec![])
            .run_from(cli)
            .await
            .unwrap();
        assert!(directory.join("m0001_initial.rs").exists());
        let first = build_operation_migration(&apps(), &[], "review", "initial", false)
            .unwrap()
            .unwrap();
        let branch = |name: &str| {
            Migration::new(
                MigrationId {
                    app: "review".into(),
                    name: name.into(),
                },
                vec![first.id.clone()],
                vec![],
            )
        };
        let history = vec![first.clone(), branch("0002_left"), branch("0002_right")];
        assert!(build_operation_migration(&apps(), &history, "review", "manual", true).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn compiled_makemigrations_check_dry_run_and_stale_registry_are_safe() {
        let apps = || InstalledApps::new().add(ReviewApp);
        let directory = temp_path("review_check", "dir");
        let check = Cli::try_parse_from([
            "manage",
            "makemigrations",
            "review",
            "--check",
            "--dir",
            directory.to_str().unwrap(),
        ])
        .unwrap();
        let error = Management::new(apps())
            .migrations(vec![])
            .run_from(check)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("need migration 0001_"));
        assert!(!directory.exists());

        let dry_run = Cli::try_parse_from([
            "manage",
            "makemigrations",
            "review",
            "--dry-run",
            "--dir",
            directory.to_str().unwrap(),
        ])
        .unwrap();
        Management::new(apps())
            .migrations(vec![])
            .run_from(dry_run)
            .await
            .unwrap();
        assert!(!directory.exists());

        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("m0001_initial.rs"), "stale source").unwrap();
        let stale = Cli::try_parse_from([
            "manage",
            "makemigrations",
            "review",
            "--name",
            "again",
            "--dir",
            directory.to_str().unwrap(),
        ])
        .unwrap();
        let error = Management::new(apps())
            .migrations(vec![])
            .run_from(stale)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Rebuild the management binary"));
        assert_eq!(
            fs::read_to_string(directory.join("m0001_initial.rs")).unwrap(),
            "stale source"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generator_review_removes_constraints_before_column() {
        let apps = InstalledApps::new().add(ReviewApp);
        let desired = apps.migration_state().unwrap();
        let mut model = desired.models().remove(0);
        let mut column = model.table.columns[0].clone();
        column.name = "obsolete".into();
        column.primary_key = false;
        model.table.columns.push(column);
        model.table.indexes.push(vec!["obsolete".into()]);
        model.table.unique_constraints.push(vec!["obsolete".into()]);
        let first = Migration::new(
            MigrationId {
                app: "review".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![MigrationOperation::CreateModel { model }],
        );
        let next = build_operation_migration(&apps, &[first.clone()], "review", "remove", false)
            .unwrap()
            .unwrap();
        let graph = che_orm::MigrationGraph::new(vec![first, next]).unwrap();
        let actual = graph.replay(Default::default()).unwrap();
        assert!(actual.diff(&desired).unwrap().is_empty());
    }

    #[derive(Debug, Model)]
    #[orm(table = "cycle_left")]
    struct CycleLeft {
        #[orm(primary_key)]
        id: i64,
        #[orm(references = "cycle_right(id)")]
        right_id: i64,
    }

    #[derive(Debug, Model)]
    #[orm(table = "cycle_right")]
    struct CycleRight {
        #[orm(primary_key)]
        id: i64,
        #[orm(references = "cycle_left(id)")]
        left_id: i64,
    }

    struct CycleLeftApp;
    impl crate::AppModule for CycleLeftApp {
        fn name(&self) -> &'static str {
            "left"
        }
        fn schema(&self) -> che_orm::SchemaSet {
            che_orm::SchemaSet::new().model::<CycleLeft>()
        }
        fn init(&self, _: &mut crate::ModuleContext) {}
    }
    struct CycleRightApp;
    impl crate::AppModule for CycleRightApp {
        fn name(&self) -> &'static str {
            "right"
        }
        fn schema(&self) -> che_orm::SchemaSet {
            che_orm::SchemaSet::new().model::<CycleRight>()
        }
        fn init(&self, _: &mut crate::ModuleContext) {}
    }

    #[test]
    fn initial_batch_defers_cyclic_cross_app_foreign_keys() {
        let apps = InstalledApps::new().add(CycleLeftApp).add(CycleRightApp);
        let migrations = super::build_initial_cycle_batch(&apps, "initial").unwrap();
        assert_eq!(migrations.len(), 4);
        let graph = che_orm::MigrationGraph::new(migrations).unwrap();
        let state = graph.replay(Default::default()).unwrap();
        assert_eq!(state, apps.migration_state().unwrap());
        let mut connection = che_orm::rusqlite::Connection::open_in_memory().unwrap();
        che_orm::migration::sqlite_executor::apply_operation_migrations_on_connection(
            &mut connection,
            graph,
        )
        .unwrap();
        let fk_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('cycle_left')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fk_count, 1);
    }

    #[test]
    fn generator_review_requires_historical_cross_app_target() {
        let mut desired = InstalledApps::new()
            .add(ReviewApp)
            .migration_state()
            .unwrap();
        let mut source = desired.models().remove(0);
        source.key.app = "source".into();
        source.key.name = "Source".into();
        source.table.name = "sources".into();
        source.table.columns[0].references = Some(che_orm::ForeignKeyState {
            target: "review_items(id)".into(),
            on_delete: None,
        });
        MigrationOperation::CreateModel {
            model: source.clone(),
        }
        .state_forwards(&mut desired)
        .unwrap();
        let changes = vec![che_orm::StateChange::CreateModel(source)];
        let owners = std::collections::BTreeMap::from([("review_items".into(), "review".into())]);
        assert!(
            super::validate_generated_references(
                &changes,
                &Default::default(),
                &desired,
                &owners,
                "source"
            )
            .is_err()
        );
        assert!(
            super::validate_generated_references(&changes, &desired, &desired, &owners, "source")
                .is_ok()
        );
    }

    #[test]
    fn generator_review_writer_lock_and_failure_preserve_files() {
        let directory = temp_path("review_lock", "dir");
        fs::create_dir_all(&directory).unwrap();
        let migration = Migration::new(
            MigrationId {
                app: "review".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![],
        );
        fs::write(directory.join(".che-migrations.lock"), "held").unwrap();
        assert!(write_operation_migration_and_register(&directory, &migration).is_err());
        assert!(!directory.join("m0001_initial.rs").exists());
        fs::remove_file(directory.join(".che-migrations.lock")).unwrap();
        fs::write(directory.join(".che-mod.rs.tmp"), "preexisting").unwrap();
        assert!(write_operation_migration_and_register(&directory, &migration).is_err());
        assert!(!directory.join("m0001_initial.rs").exists());
        assert_eq!(
            fs::read_to_string(directory.join(".che-mod.rs.tmp")).unwrap(),
            "preexisting"
        );
        fs::remove_file(directory.join(".che-mod.rs.tmp")).unwrap();
        write_operation_migration_and_register(&directory, &migration).unwrap();
        let before = fs::read(directory.join("mod.rs")).unwrap();
        assert!(write_operation_migration_and_register(&directory, &migration).is_err());
        assert_eq!(fs::read(directory.join("mod.rs")).unwrap(), before);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_registry_compiles_and_applies_in_downstream_project() {
        let directory = temp_path("compiled_registry", "dir");
        let migrations_dir = directory.join("src/migrations");
        let apps = InstalledApps::new().add(ReviewApp);
        let first = build_operation_migration(&apps, &[], "review", "initial", false)
            .unwrap()
            .unwrap();
        write_operation_migration_and_register(&migrations_dir, &first).unwrap();
        let second = build_operation_migration(&apps, &[first], "review", "manual", true)
            .unwrap()
            .unwrap();
        write_operation_migration_and_register(&migrations_dir, &second).unwrap();
        let rest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let orm = rest.join("../che-orm");
        fs::write(directory.join("Cargo.toml"), format!(
            "[package]\nname = \"registry-compile-test\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nche-rest = {{ path = {:?} }}\nche-orm = {{ path = {:?} }}\n", rest, orm
        )).unwrap();
        fs::write(directory.join("src/main.rs"), r#"
mod migrations;
fn main() {
    let registered = migrations::all();
    assert_eq!(registered.len(), 2);
    assert!(registered[1].operations.is_empty());
    let _management = che_rest::Management::new(che_rest::InstalledApps::new()).migrations(registered.clone());
    let graph = che_orm::MigrationGraph::new(registered).unwrap();
    let mut connection = che_orm::rusqlite::Connection::open_in_memory().unwrap();
    use che_orm::migration::sqlite_executor::apply_operation_migrations_on_connection;
    apply_operation_migrations_on_connection(&mut connection, graph.clone()).unwrap();
    apply_operation_migrations_on_connection(&mut connection, graph).unwrap();
    connection.execute("INSERT INTO review_items (id) VALUES (7)", []).unwrap();
}
"#).unwrap();
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| rest.join("target"));
        let output = std::process::Command::new(env!("CARGO"))
            .args(["run", "--offline", "--quiet", "--manifest-path"])
            .arg(directory.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
    async fn operation_migrations_apply_declarative_sqlite_operations() {
        let database_path = temp_path("declarative_operation_migrate", "sqlite");
        let config = sqlite_config(&database_path);
        let table = che_orm::TableState {
            name: "operation_items".into(),
            columns: vec![che_orm::ColumnState {
                field_name: "id".into(),
                name: "id".into(),
                column_type: che_orm::ColumnType::Integer,
                nullable: false,
                primary_key: true,
                unique: false,
                default: None,
                check: None,
                choices: None,
                references: None,
                auto_now_add: false,
                auto_now: false,
            }],
            unique_constraints: vec![],
            indexes: vec![],
        };
        let name = che_orm::ColumnState {
            field_name: "name".into(),
            name: "name".into(),
            column_type: che_orm::ColumnType::Text,
            nullable: false,
            primary_key: false,
            unique: false,
            default: Some("'untitled'".into()),
            check: None,
            choices: None,
            references: None,
            auto_now_add: false,
            auto_now: false,
        };
        let initial = Migration::new(
            MigrationId {
                app: "items".into(),
                name: "0001_initial".into(),
            },
            vec![],
            vec![MigrationOperation::CreateTable { table }],
        );
        let add_name = Migration::new(
            MigrationId {
                app: "items".into(),
                name: "0002_add_name".into(),
            },
            vec![initial.id.clone()],
            vec![MigrationOperation::AddColumn {
                table: "operation_items".into(),
                column: name,
            }],
        );

        apply_operation_migrations(&config, vec![initial.clone(), add_name.clone()])
            .await
            .unwrap();
        let connection = che_orm::rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute("INSERT INTO operation_items (id) VALUES (1)", [])
            .unwrap();
        let name: String = connection
            .query_row("SELECT name FROM operation_items WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "untitled");
        connection
            .execute_batch("CREATE UNIQUE INDEX manual_unique ON operation_items(name)")
            .unwrap();
        let extend = Migration::new(
            MigrationId {
                app: "items".into(),
                name: "0003_index".into(),
            },
            vec![add_name.id.clone()],
            vec![MigrationOperation::AddUniqueConstraint {
                table: "operation_items".into(),
                columns: vec!["id".into(), "name".into()],
            }],
        );
        let error = apply_operation_migrations(&config, vec![initial, add_name, extend])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unmanaged index manual_unique"));
        assert!(
            connection
                .execute("INSERT INTO operation_items(id) VALUES(2)", [])
                .is_err()
        );
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM che_migration_history", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
        let _ = fs::remove_file(database_path);
    }

    #[test]
    fn operation_history_rejects_an_applied_migration_without_its_dependency() {
        let initial = Migration {
            id: MigrationId {
                app: "tasks".into(),
                name: "0001_initial".into(),
            },
            dependencies: vec![],
            operations: vec![],
            checksum: "initial".into(),
        };
        let follow_up = Migration {
            id: MigrationId {
                app: "tasks".into(),
                name: "0002_follow_up".into(),
            },
            dependencies: vec![initial.id.clone()],
            operations: vec![],
            checksum: "follow-up".into(),
        };
        let graph = che_orm::MigrationGraph::new(vec![initial, follow_up.clone()]).unwrap();
        let applied = HashMap::from([(
            (follow_up.id.app.clone(), follow_up.id.name.clone()),
            follow_up.checksum.clone(),
        )]);

        let error = pending_operation_migrations(&graph, &applied)
            .unwrap_err()
            .to_string();
        assert!(error.contains("inconsistent operation migration history"));
        assert!(error.contains("0001_initial"));
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
