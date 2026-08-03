use std::path::PathBuf;

use che_rest::project::{StartProjectOptions, startproject};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "che-rest")]
#[command(about = "Project utilities for che-rest applications")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Startproject {
        name: String,

        #[arg(long, default_value = ".")]
        out: PathBuf,

        #[arg(long, default_value = "../che-rest")]
        che_rest_path: String,

        #[arg(long, default_value = "../che-orm/crates/che-orm")]
        che_orm_path: String,

        #[arg(long)]
        with_auth: bool,

        #[arg(long)]
        force: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Startproject {
            name,
            out,
            che_rest_path,
            che_orm_path,
            with_auth,
            force,
        } => startproject(StartProjectOptions {
            name,
            out,
            che_rest_path,
            che_orm_path,
            with_auth,
            force,
        })?,
    }

    Ok(())
}
