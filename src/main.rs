mod azdo;
mod browser;
mod config;
mod error;
mod ops;
mod tui;
mod workflow;

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tokio::runtime::Builder as RuntimeBuilder;
use tracing_subscriber::EnvFilter;

use crate::azdo::build;
use crate::azdo::query;
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

    /// List your open work items (assigned to you, not Closed/Done/Removed).
    My {
        /// Pick one interactively and open it in the TUI viewer.
        #[arg(long)]
        pick: bool,
    },

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

        /// Open the work item in the system browser (takes precedence
        /// over `--tui` and plain rendering; no network call).
        #[arg(long)]
        open: bool,

        #[command(subcommand)]
        action: Option<TaskAction>,
    },

    /// Queue a build for a configured product.
    Build {
        #[command(subcommand)]
        action: BuildAction,
    },

    /// Run a workflow script (Rhai spike: minimal host surface).
    Run {
        /// Path to the `.rhai` workflow file.
        file: PathBuf,

        /// Preview: reads and the escape hatch run for real, every AzDO
        /// mutation is stubbed.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum TaskAction {
    /// Post a comment to the work item (`-` reads the body from stdin).
    Comment {
        /// Comment text, or `-` to read the whole body from stdin.
        text: String,
    },

    /// Set the work item state (accepts `[states]` aliases from config).
    SetState {
        /// State name, or a `[states]` alias (e.g. `test`).
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum BuildAction {
    /// Queue a build for `[products.<product>]` and optionally wait for it.
    Start {
        /// Product key from `[products.<name>]` in the config.
        product: String,

        /// Pass `WorkItemId=<id>` to the build.
        #[arg(long = "work-item", value_name = "ID")]
        work_item: Option<u64>,

        /// Extra build parameter as `key=value` (repeatable).
        #[arg(long = "param", value_name = "K=V")]
        params: Vec<String>,

        /// Block until the build reaches a terminal state.
        #[arg(long)]
        wait: bool,
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

    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

/// Sync setup, then either run the workflow spike synchronously (it owns its
/// own runtime — see `workflow`) or drive a network command on a shared
/// current-thread runtime. The two paths are kept separate so a per-op
/// `block_on` inside `RealOps` is never nested in an outer `block_on`.
fn dispatch(cli: Cli) -> Result<ExitCode, AzdoError> {
    let mut cfg = Config::load(cli.config.as_deref())?;
    apply_overrides(&mut cfg, &cli);
    cfg.validate()?;

    let pat = cfg.read_pat()?;
    let client = AzdoClient::new(&cfg, &pat)?;

    match cli.command {
        Command::Run { file, dry_run } => Ok(ExitCode::from(workflow::run_file(
            &client, &cfg, &file, dry_run,
        )?)),
        command => {
            let runtime = RuntimeBuilder::new_current_thread().enable_all().build()?;
            runtime.block_on(run_net(&client, &cfg, command))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn run_net(client: &AzdoClient, cfg: &Config, command: Command) -> Result<(), AzdoError> {
    match command {
        Command::Ping => cmd_ping(client).await,
        Command::My { pick } => cmd_my(client, pick).await,
        Command::Task {
            id,
            comments,
            tui,
            open,
            action,
        } => match action {
            Some(TaskAction::Comment { text }) => cmd_comment(client, id, &text).await,
            Some(TaskAction::SetState { name }) => {
                cmd_set_state(client, id, resolve_state(&name, &cfg.states)).await
            }
            None => cmd_task(client, id, comments, tui, open).await,
        },
        Command::Build { action } => match action {
            BuildAction::Start {
                product,
                work_item,
                params,
                wait,
            } => {
                cmd_build_start(
                    client,
                    cfg,
                    &product,
                    BuildStartOpts {
                        work_item,
                        params,
                        wait,
                    },
                )
                .await
            }
        },
        // `run` is dispatched synchronously in `dispatch`; never reaches here.
        Command::Run { .. } => Ok(()),
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

async fn cmd_my(client: &AzdoClient, pick: bool) -> Result<(), AzdoError> {
    let items = query::my_open_items(client).await?;
    if pick {
        if items.is_empty() {
            println!("No open work items.");
            return Ok(());
        }
        return tui::pick(client, items).await;
    }
    println!("{}", query::render_table(&items));
    Ok(())
}

async fn cmd_task(
    client: &AzdoClient,
    id: u64,
    comments: Option<u32>,
    tui: bool,
    open: bool,
) -> Result<(), AzdoError> {
    if open {
        let url = client.web_item_url(id);
        browser::open(&url)?;
        println!("opening {url}");
        return Ok(());
    }
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

/// Resolve the comment body: `-` pulls from `read_stdin`, anything else is
/// the literal argument. Surrounding whitespace is trimmed and an empty
/// result is rejected so we never POST a blank comment.
fn resolve_comment_body(
    arg: &str,
    read_stdin: impl FnOnce() -> io::Result<String>,
) -> Result<String, AzdoError> {
    let raw = if arg == "-" {
        read_stdin()?
    } else {
        arg.to_owned()
    };
    let body = raw.trim();
    if body.is_empty() {
        return Err(AzdoError::Input("comment body is empty".to_owned()));
    }
    Ok(body.to_owned())
}

async fn cmd_comment(client: &AzdoClient, id: u64, text: &str) -> Result<(), AzdoError> {
    let body = resolve_comment_body(text, || {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        Ok(buf)
    })?;
    workitem::post_comment(client, id, &body).await?;
    println!("comment posted on work item #{id}");
    Ok(())
}

/// Map `name` through the `[states]` alias table, falling back to the
/// literal name when it is not an alias.
fn resolve_state<'a>(name: &'a str, states: &'a BTreeMap<String, String>) -> &'a str {
    states.get(name).map_or(name, String::as_str)
}

async fn cmd_set_state(client: &AzdoClient, id: u64, state: &str) -> Result<(), AzdoError> {
    let item = workitem::set_state(client, id, state).await?;
    println!("work item #{id} state set to \"{}\"", item.state());
    Ok(())
}

#[derive(Debug)]
struct BuildStartOpts {
    work_item: Option<u64>,
    params: Vec<String>,
    wait: bool,
}

/// Assemble the Azure DevOps `parameters` value (itself a JSON-encoded
/// object) from `--work-item` and repeated `--param key=value`. Returns
/// `None` when there is nothing to send. Keys must be non-empty and each
/// `--param` must contain a `=`.
fn build_parameters(
    work_item: Option<u64>,
    params: &[String],
) -> Result<Option<String>, AzdoError> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Some(id) = work_item {
        drop(map.insert("WorkItemId".to_owned(), id.to_string()));
    }
    for kv in params {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| AzdoError::Input(format!("--param must be key=value, got `{kv}`")))?;
        if k.is_empty() {
            return Err(AzdoError::Input(format!(
                "--param has an empty key: `{kv}`"
            )));
        }
        drop(map.insert(k.to_owned(), v.to_owned()));
    }
    if map.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&map)
        .map(Some)
        .map_err(|e| AzdoError::Render(e.to_string()))
}

async fn cmd_build_start(
    client: &AzdoClient,
    cfg: &Config,
    product: &str,
    opts: BuildStartOpts,
) -> Result<(), AzdoError> {
    let def_id = cfg
        .products
        .get(product)
        .and_then(|p| p.build_definition_id)
        .ok_or_else(|| {
            AzdoError::Config(format!(
                "no `build_definition_id` for product `{product}` (expected [products.{product}])"
            ))
        })?;
    let parameters = build_parameters(opts.work_item, &opts.params)?;
    let queued = build::queue_build(client, def_id, parameters).await?;
    println!("queued build #{} (definition {def_id})", queued.id);
    if opts.wait {
        let done = build::wait_build(client, queued.id).await?;
        let result = done.result.as_deref().unwrap_or("(no result)");
        let number = done.number.as_deref().unwrap_or("-");
        println!("build #{} {result} [{number}]", done.id);
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{self, Error, ErrorKind};

    use super::{build_parameters, resolve_comment_body, resolve_state, AzdoError};

    fn never_called() -> io::Result<String> {
        panic!("stdin must not be read for a literal argument")
    }

    #[test]
    fn literal_argument_is_trimmed_and_kept() {
        let body =
            resolve_comment_body("  hello world  ", never_called).expect("literal must resolve");
        assert_eq!(body, "hello world", "surrounding whitespace is trimmed");
    }

    #[test]
    fn dash_reads_and_trims_stdin() {
        let body = resolve_comment_body("-", || Ok("from stdin\n".to_owned()))
            .expect("stdin must resolve");
        assert_eq!(body, "from stdin", "trailing newline from stdin is trimmed");
    }

    #[test]
    fn empty_literal_is_rejected() {
        let err = resolve_comment_body("   ", never_called).expect_err("empty must fail");
        assert!(
            matches!(err, AzdoError::Input(_)),
            "expected Input error, got {err:?}",
        );
    }

    #[test]
    fn blank_stdin_is_rejected() {
        let err = resolve_comment_body("-", || Ok("  \n\t ".to_owned()))
            .expect_err("blank stdin must fail");
        assert!(
            matches!(err, AzdoError::Input(_)),
            "expected Input error, got {err:?}",
        );
    }

    #[test]
    fn state_alias_resolves_else_passes_through() {
        let mut states = BTreeMap::new();
        drop(states.insert("test".to_owned(), "Ready for Test".to_owned()));

        assert_eq!(
            resolve_state("test", &states),
            "Ready for Test",
            "a configured alias maps to its full state name",
        );
        assert_eq!(
            resolve_state("Active", &states),
            "Active",
            "a non-alias is treated as a literal state name",
        );
        assert_eq!(
            resolve_state("anything", &BTreeMap::new()),
            "anything",
            "with no aliases every name passes through verbatim",
        );
    }

    #[test]
    fn stdin_read_error_propagates() {
        let err = resolve_comment_body("-", || Err(Error::new(ErrorKind::BrokenPipe, "boom")))
            .expect_err("io error must propagate");
        assert!(
            matches!(err, AzdoError::Io(_)),
            "expected Io error, got {err:?}",
        );
    }

    #[test]
    fn build_parameters_emits_sorted_json_object() {
        let out = build_parameters(Some(12345), &["b=2".to_owned(), "a=1".to_owned()])
            .expect("valid params")
            .expect("non-empty -> Some");
        assert_eq!(
            out, r#"{"WorkItemId":"12345","a":"1","b":"2"}"#,
            "parameters must be a JSON object with deterministically sorted keys",
        );
    }

    #[test]
    fn build_parameters_is_none_when_empty() {
        let out = build_parameters(None, &[]).expect("no params is valid");
        assert!(out.is_none(), "nothing to send must be None, got {out:?}");
    }

    #[test]
    fn build_parameters_rejects_missing_equals_and_empty_key() {
        let no_eq = build_parameters(None, &["novalue".to_owned()])
            .expect_err("a param without `=` must fail");
        assert!(
            matches!(no_eq, AzdoError::Input(_)),
            "expected Input error, got {no_eq:?}",
        );
        let empty_key =
            build_parameters(None, &["=v".to_owned()]).expect_err("an empty key must fail");
        assert!(
            matches!(empty_key, AzdoError::Input(_)),
            "expected Input error, got {empty_key:?}",
        );
    }
}
