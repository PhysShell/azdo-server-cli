# Roadmap

This is the staged delivery plan for `azdo`. Each stage is a vertical slice:
it ships independently and is closed only when its acceptance criterion can
be run against a real Azure DevOps Server 2020 instance.

The full specification lives in the original design doc. This file tracks
**what is implemented right now**.

Legend: `[x]` shipped &middot; `[ ]` not yet &middot; `[~]` in progress

---

## Stages

### [x] S0 — Project skeleton

- `cargo` project bootstrapped with the production dependency set
  (`reqwest` + `rustls`, `clap`, `serde`, `tokio`, `tracing`, `thiserror`,
  `base64`).
- `Config` loader (TOML + env), validation, `Pat` newtype with a redacted
  `Debug` impl.
- `AzdoClient` with PAT-based Basic auth, connect/request timeouts,
  collection/project URL builders.
- `azdo ping` verifies the configured project is reachable.
- `cargo clippy --all-targets -- -D warnings` and `cargo test` are green.

**Acceptance:** `AZDO_PAT=... azdo ping` prints `OK 200 (project "<name>" accessible)`.

---

### [x] S1 — `azdo task <id>` (plain-text view)

Read a work item via `GET /_apis/wit/workitems/{id}` and render it as a
readable block in stdout: id, type, state, title, assignee, changed date,
tags, description (HTML stripped via `html2text`). Missing optional fields
are omitted; `System.AssignedTo` accepts both the modern identity object
and a bare-string fallback.

**Acceptance:** `AZDO_PAT=... azdo task 12345` prints the work item block;
unknown ids surface as `HTTP 404` and exit code 4.

---

### [x] S2 — Comments (read)

Add `--comments N` to `azdo task <id>`. Fetches
`GET /_apis/wit/workItems/{id}/comments` (`api-version=6.0-preview.3`,
project-scoped, `$top=N`) and appends a chronologically ordered list
(oldest first, ties broken by comment id) under the work item. Comment
text is HTML-flattened; a missing author renders as `Unknown`.

**Acceptance:** `AZDO_PAT=... azdo task 12345 --comments 5` prints the
work item block followed by a `Comments (n):` section.

---

### [x] S3 — TUI mode

Ratatui + crossterm interactive view, opened with `azdo task <id> --tui`
(plain text stays the default). Layout: bordered header/meta block, a
scrollable description pane and a scrollable comments pane, plus a
footer hint/status line. Hotkeys: `q`/`Esc` quit, `r` refresh (re-fetch;
failures surface in the status line, old data kept), `o` open in browser,
`Tab`/`Shift+Tab` switch the active pane, `j`/`k` (and arrows) scroll.
Terminal state is always restored: a `TerminalGuard` covers normal and
`?`-propagation exits, and a chained panic hook restores the screen
before the default hook prints.

**Acceptance:** `AZDO_PAT=... azdo task 12345 --tui` opens the viewer;
`q` exits with the terminal fully restored.

---

### [x] S4 — `azdo task <id> comment "..."`

`POST .../comments` (`6.0-preview.3`, JSON `{"text": ...}`). `comment -`
reads the whole body from stdin; surrounding whitespace is trimmed and an
empty body is rejected (`AzdoError::Input`) so a blank comment is never
sent. In the TUI: hotkey `c` opens a one-line prompt on the footer row
(`Enter` posts then re-fetches so the new comment shows, `Esc` cancels);
view hotkeys are suppressed while typing.

**Acceptance:** `azdo task 12345 comment "looks good"` prints a
confirmation; `echo body | azdo task 12345 comment -` posts stdin; in
`--tui`, `c` then text then `Enter` adds the comment and refreshes.

---

### [x] S5 — `azdo task <id> set-state <name>`

`PATCH .../workitems/{id}` with a single-op `application/json-patch+json`
document (`add /fields/System.State`). The Content-Type is set before
`.json()` so reqwest keeps the patch media type. `name` is resolved
through the `[states]` alias table, falling back to the literal name
(e.g. `set-state test` → `Ready for Test`, `set-state Active` →
`Active`). The server's stored state is echoed back on success.

**Acceptance:** `azdo task 12345 set-state test` patches the item and
prints `work item #12345 state set to "Ready for Test"`; an unknown
state surfaces the server's HTTP error.

---

### [x] S6 — `--open` (browser)

Opens the work item URL
(`{server}/{collection}/{project}/_workitems/edit/{id}`, built by
`AzdoClient::web_item_url`) in the system browser. The launch logic
lives in a shared `browser` module (`xdg-open`, or `cmd /C start` on
Windows; the child is reaped so there is no zombie). Used by the
`azdo task <id> --open` flag (no network call; precedes `--tui`/plain)
and the existing TUI `o` hotkey, which now delegates to the same code.
A failed launch is a typed `AzdoError::Browser` (exit 1) on the CLI and
a non-fatal status line in the TUI.

**Acceptance:** `azdo task 12345 --open` launches the browser at the
edit URL and prints `opening <url>`; `o` in `--tui` does the same
without leaving the viewer.

---

### [ ] S7 — `azdo my` (WIQL)

List the current user's open work items via WIQL +
`workitemsbatch`. Renders as a table; optional `--pick` opens the chosen
item in the TUI.

---

### [ ] S8 — `azdo build start`

Queue a build via `POST /_apis/build/builds` with parameter passthrough
(`{ "WorkItemId": "12345" }`). Resolves build definition ID through
`[products.<name>]`. `--wait` polls until the build reaches a terminal state.

---

### [ ] S9 — `azdo daily` (wiki)

Copy yesterday's daily wiki page to today's path (`prev_workday` aware:
Monday picks up Friday). Idempotent: re-running on the same day is a no-op.
Date substitution is restricted to `YYYY-MM-DD` literals — no global
text replacement.

---

### [ ] S10 — `azdo send-test <id>` (composite)

End-to-end "hand off to QA" workflow: change state, post a templated
comment with `@mentions` resolved through `[profiles.<role>]`, optionally
trigger a build. `--dry-run` prints the plan without making any changes.

---

### [ ] S11 — Distribution

Release builds for `x86_64-pc-windows-msvc` and
`x86_64-unknown-linux-musl`. Packaged as `.zip` / `.tar.gz` with the
example config and a copy of this README.

---

## Cross-cutting requirements

These apply to every stage and are checked at PR time:

- `cargo clippy --all-targets -- -D warnings` must be clean.
- `cargo fmt --check` must be clean.
- Every new `azdo::*` module ships with a fixture-based snapshot test.
- No `unwrap()` in production paths; `expect("...")` only with an
  explanation.
- PAT must never be logged. The `Pat` newtype enforces a redacted `Debug`.
- HTTP errors print status + URL + a 500-byte body excerpt, then exit with
  the code documented in `README.md`.
- All HTML-bearing fields (`System.Description`, comment text) pass through
  the HTML-to-terminal helper before rendering.
