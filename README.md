# azdo

A small Rust CLI/TUI for **Azure DevOps Server 2020 Update 1** (on-prem).

The official `az devops` extension only works against Azure DevOps **Services**
(`dev.azure.com`). Server 2020 has no first-party CLI, so this tool talks to
the REST API directly. It is intentionally narrow: view a work item, read and
write comments, change state, trigger a pipeline, prepare a daily wiki page —
the daily-driver stuff, no portal required.

Cross-platform: one static binary for Windows and Linux. PAT is read from an
environment variable; nothing sensitive is ever written to disk.

> **Status:** under active development. See [ROADMAP.md](ROADMAP.md) for the
> stage-by-stage plan and what is already shipped.

## Installation

### From source

```sh
cargo install --path .
```

A pre-built `azdo` (Linux) / `azdo.exe` (Windows) will also be attached to
GitHub releases once the project hits its first tagged version.

## Configuration

Copy `config.example.toml` to:

- **Windows:** `%APPDATA%\azdo\config.toml`
- **Linux:**   `$XDG_CONFIG_HOME/azdo/config.toml` (fallback: `~/.config/azdo/config.toml`)

Then edit `server`, `collection`, `project` and any optional sections.

**PAT is never stored in the config file.** Set it via an environment variable
(default name: `AZDO_PAT`). The variable name is configurable under `[auth]`.

```sh
# Linux / WSL
export AZDO_PAT="<your-personal-access-token>"

# Windows PowerShell
$env:AZDO_PAT = "<your-personal-access-token>"
```

You can override key config fields per invocation:

```
--config <PATH>           Use an alternate config file
--server <URL>            Override server
--collection <NAME>       Override collection
--project <NAME>          Override project
```

## Usage

Verify the configuration and PAT:

```sh
azdo ping
# OK 200 (project "Customs" accessible)
```

Additional commands are added stage by stage; see [ROADMAP.md](ROADMAP.md) for
what is already available.

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | Generic error (IO, parsing) |
| 2    | Configuration error or missing PAT |
| 3    | Transport error (DNS, TCP, TLS, timeout) |
| 4    | HTTP 4xx (auth, not found, validation) |
| 5    | HTTP 5xx (server error) |

## Logging

Set `RUST_LOG=azdo=debug` to see internal logs (HTTP requests, parsing). By
default the tool is silent on stderr unless there is an error.

## Development

Requirements: Rust stable (pinned via `rust-toolchain.toml`).

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

## License

[WTFPL](LICENSE). Do whatever you want.
