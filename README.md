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

Show a work item as a readable plain-text block:

```sh
azdo task 12345
# #12345  [Bug]  State: Active
# Title:    Customs declaration form rejects valid TIN
# Assignee: Ivan Petrov
# Changed:  2026-05-14T10:23:00Z
# Tags:     customs, urgent
#
# Description:
# The form rejects a valid TIN.
```

The HTML description is flattened to terminal text; optional fields
(assignee, changed date, tags) are omitted when the server returns none.

Open the interactive TUI viewer instead of plain text:

```sh
azdo task 12345 --tui
```

Hotkeys: `q`/`Esc` quit, `r` refresh, `c` comment, `o` open in browser,
`Tab`/`Shift+Tab` switch pane, `j`/`k` (or arrows) scroll the active
pane. The terminal is always restored on exit or panic.

Open the work item in the system browser instead of rendering it
(`xdg-open`, or `cmd /C start` on Windows; no network call):

```sh
azdo task 12345 --open
```

`--open` takes precedence over `--tui` and plain rendering; the same
action is bound to `o` inside the TUI.

Append recent comments with `--comments N` (oldest first):

```sh
azdo task 12345 --comments 5
# ...work item block...
#
# Comments (2):
#
# [2026-05-13T09:00:00Z] Ivan Petrov
# First comment.
#
# [2026-05-14T11:30:00Z] Maria Ivanova
# Second comment.
```

Post a comment (the `comment` argument may be `-` to read the whole body
from stdin):

```sh
azdo task 12345 comment "Reproduced on staging; raising priority."
git log -1 --format=%B | azdo task 12345 comment -
```

A blank body is rejected rather than posted. In `--tui`, press `c`, type
the comment, and `Enter` to post (the view re-fetches so it appears);
`Esc` cancels.

Change the work item state:

```sh
azdo task 12345 set-state "Ready for Test"
azdo task 12345 set-state test          # via a [states] alias
```

`set-state` accepts a literal state name or an alias from the `[states]`
table in the config, e.g.:

```toml
[states]
test = "Ready for Test"
active = "Active"
```

Anything not listed there is sent verbatim, so configuring aliases is
optional. The server's stored state is printed back on success.

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
