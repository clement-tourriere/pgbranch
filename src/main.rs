use anyhow::Result;
use clap::{CommandFactory, Parser};

mod config;
mod database;
mod git;
mod cli;
mod docker;
mod post_commands;
mod local_state;

use cli::Commands;

#[derive(Parser)]
#[command(name = "pgbranch")]
#[command(about = "A tool for creating PostgreSQL database branches that sync with Git branches")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::init();
    
    let cli = Cli::parse();
    
    match cli.command {
        Some(cmd) => cli::handle_command(cmd).await?,
        None => {
            // Print help when no command is provided
            let mut cmd = Cli::command();
            cmd.print_help()?;
        }
    }
    
    Ok(())
}