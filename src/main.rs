mod azdo;
mod config;
mod error;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::azdo::AzdoClient;
use crate::config::Config;
use crate::error::AzdoError;

#[derive(Parser, Debug)]
#[command(name = "azdo", version, about = "CLI/TUI for Azure DevOps Server 2020")]
struct Cli {
    /// Path to config file (default: platform config dir / azdo / config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Override server URL (e.g. https://azdo.company.local/tfs)
    #[arg(long, global = true)]
    server: Option<String>,

    /// Override collection name
    #[arg(long, global = true)]
    collection: Option<String>,

    /// Override project name
    #[arg(long, global = true)]
    project: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Verify connectivity and PAT by fetching configured project.
    Ping,
}

fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot init tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

async fn run(cli: Cli) -> Result<(), AzdoError> {
    let mut cfg = Config::load(cli.config.as_deref())?;
    apply_overrides(&mut cfg, &cli);
    cfg.validate()?;

    let pat = cfg.read_pat()?;
    let client = AzdoClient::new(&cfg, &pat)?;

    match cli.command {
        Command::Ping => cmd_ping(&client).await,
    }
}

fn apply_overrides(cfg: &mut Config, cli: &Cli) {
    if let Some(s) = &cli.server {
        cfg.server = s.clone();
    }
    if let Some(c) = &cli.collection {
        cfg.collection = c.clone();
    }
    if let Some(p) = &cli.project {
        cfg.project = p.clone();
    }
}

async fn cmd_ping(client: &AzdoClient) -> Result<(), AzdoError> {
    let status = client.ping().await?;
    println!(
        "OK {} (project \"{}\" accessible)",
        status,
        client.project_name()
    );
    Ok(())
}
