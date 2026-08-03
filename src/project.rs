use std::{
    fs,
    path::{Path, PathBuf},
};

type ProjectResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone)]
pub struct StartProjectOptions {
    pub name: String,
    pub out: PathBuf,
    pub che_rest_path: String,
    pub che_orm_path: String,
    pub with_auth: bool,
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
    println!("  cargo run");
    println!("  cargo run --bin manage -- startapp users");

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

fn cargo_toml_template(name: &str, che_rest_path: &str, che_orm_path: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
default-run = "{name}"

[dependencies]
axum = "0.8"
che-orm = {{ path = "{che_orm_path}" }}
che-rest = {{ path = "{che_rest_path}" }}
tokio = {{ version = "1", features = ["macros", "net", "rt-multi-thread"] }}
"#
    )
}

fn app_toml_template() -> &'static str {
    r#"[database]
url = "sqlite://db.sqlite?mode=rwc"
"#
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
    let app = Server::new(state)
        .install({crate_name}::apps::installed_apps())
        .build()
        .await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
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
    Management::new({crate_name}::apps::installed_apps()).run().await
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
        r#"use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {{
    InstalledApps::new(){auth_module}
}}
"#
    )
}
