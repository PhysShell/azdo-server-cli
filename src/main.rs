mod azdo;
mod config;
mod error;
mod tui;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tokio::runtime::Builder as RuntimeBuilder;
use tracing_subscriber::EnvFilter;

use crate::azdo::workitem;
use crate::azdo::AzdoClient;
use crate::config::Config;
use crate::error::AzdoError;
use crate::tui::run as run_tui;

#[derive(Parser, Debug)]
#[command(name = "azdo", version, about = "CLI/TUI for Azure DevOps Server 2020")]
struct Cli {
    /// Path to config file (default: platform config dir / azdo / config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Override server URL (e.g. `https://azdo.company.local/tfs`).
    #[arg(long, global = true)]
    server: Option<String>,

    /// Override collection name.
    #[arg(long, global = true)]
    collection: Option<String>,

    /// Override project name.
    #[arg(long, global = true)]
    project: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Verify connectivity and PAT by fetching configured project.
    Ping,

    /// Show a work item as a readable plain-text block.
    Task {
        /// Numeric work item id.
        id: u64,

        /// Also append up to N comments (oldest first).
        #[arg(long, value_name = "N")]
        comments: Option<u32>,

        /// Open the interactive TUI viewer instead of plain text.
        #[arg(long)]
        tui: bool,
    },
}

fn main() -> ExitCode {
    drop(
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
            )
            .with_writer(io::stderr)
            .try_init(),
    );

    let cli = Cli::parse();

    let runtime = match RuntimeBuilder::new_current_thread().enable_all().build() {
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
        Command::Task { id, comments, tui } => cmd_task(&client, id, comments, tui).await,
    }
}

fn apply_overrides(cfg: &mut Config, cli: &Cli) {
    if let Some(s) = cli.server.as_ref() {
        cfg.server.clone_from(s);
    }
    if let Some(c) = cli.collection.as_ref() {
        cfg.collection.clone_from(c);
    }
    if let Some(p) = cli.project.as_ref() {
        cfg.project.clone_from(p);
    }
}

async fn cmd_ping(client: &AzdoClient) -> Result<(), AzdoError> {
    let status = client.ping().await?;
    println!(
        "OK {} (project \"{}\" accessible)",
        status,
        client.project_name(),
    );
    Ok(())
}

async fn cmd_task(
    client: &AzdoClient,
    id: u64,
    comments: Option<u32>,
    tui: bool,
) -> Result<(), AzdoError> {
    if tui {
        return run_tui(client, id).await;
    }

    let item = workitem::fetch(client, id).await?;
    let mut out = workitem::render(&item)?;

    if let Some(top) = comments.filter(|n| *n > 0) {
        let list = workitem::fetch_comments(client, id, top).await?;
        out.push_str("\n\n");
        out.push_str(&workitem::render_comments(&list)?);
    }

    println!("{out}");
    Ok(())
}
