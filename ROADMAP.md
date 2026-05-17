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

### [x] S7 — `azdo my` (WIQL)

`POST /_apis/wit/wiql` (`@Me`, state not Closed/Done/Removed, newest
change first) for the ids, then `POST /_apis/wit/workitemsbatch`
(chunked at the 200-id cap, request order preserved) for the fields.
Renders an aligned ID/Type/State/Title table reusing the `WorkItem`
accessors. `--pick` runs an interactive ratatui list (`j`/`k` move,
`Enter` open, `q`/`Esc` quit); the chosen item opens in the existing
viewer within the same terminal session (the item event loop was
factored into a `view` helper so the picker and `--tui` share one
terminal setup/guard/panic hook).

**Acceptance:** `azdo my` prints the table (or `No open work items.`);
`azdo my --pick` lets you select one and drops into the work-item TUI,
restoring the terminal on exit.

---

### [x] S8 — `azdo build start`

`azdo build start <product>` queues a build via
`POST /_apis/build/builds`. The build definition id is resolved from
`[products.<product>].build_definition_id` (a missing entry is a typed
`Config` error). `--work-item <id>` and repeated `--param key=value` are
folded into the Azure DevOps `parameters` string (a JSON-encoded object
with deterministically sorted keys; an empty key or a `--param` without
`=` is a typed `Input` error). `--wait` polls
`GET /_apis/build/builds/{id}` every 5s until `status == "completed"`
and prints the result and build number. Queue/report only — mapping a
failed build to a non-zero exit is deliberately left to the workflow
engine, not the raw command.

**Acceptance:** with `[products.declaration] build_definition_id = 42`,
`azdo build start declaration --work-item 12345` prints
`queued build #<n> (definition 42)`; adding `--wait` blocks and then
prints `build #<n> succeeded [<number>]`.

---

### [x] S9 — `azdo daily` (wiki)

Copy yesterday's daily wiki page to today's path (`prev_workday` aware:
Monday picks up Friday). Idempotent: re-running on the same day is a no-op.
Date substitution is restricted to `YYYY-MM-DD` literals — no global
text replacement.

**Design locked — S9 is the first real Rhai workflow** (it dogfoods the
spike surface and is what pulls the minimal engine into being; the prose
above is superseded where it conflicts):

- **Wiki shape.** `{root}/{YYYY}/Q{n}/{dd.MM.yyyy}`. The **target** path is
  a pure function of *today's date alone*, so a new quarter or year folder
  is never a special case — it falls out of `quarter(today)`/`today.year`.
  The **source** is `pick_latest`: the chronological max over *every*
  existing page, ordered by parsed date (never lexically — `01.01.2026`
  beats `31.12.2025`), so it crosses quarter/year rollovers by
  construction. Target and source are computed independently; there is no
  rollover branch. `prev_workday` is subsumed (the latest page is the
  latest page, weekends/gaps included).
- **Where the logic lives.** The invariant-bearing date/path core is pure
  Rust (`src/daily.rs`) precisely so property-based tests can pin it; the
  workflow *shape* (sequence, the don't-overwrite guard, messages) stays in
  the editable `.rhai`. "Policy in the script" means flow, not the gnarly
  date math — PBT lives in Rust, so the math must too.
- **Ops seam.** Only one genuinely new method — `wiki_list` (recursive page
  listing). `wiki_get`/`wiki_put` are already in the contract, currently
  `UNWIRED` in `RealOps`; S9 makes them real. Script-facing verbs:
  `wiki_list` / `wiki_read` / `wiki_create`.
- **Behaviour.** The tool *creates* the page (a real write, seeded with the
  latest page's content); the human does the final edit + save in the
  browser. Re-running is not a silent no-op: the script checks the listing
  and refuses to overwrite an existing page (a clean `Stop`/`fail`, not a
  clobber). UTC date is a deliberate spike simplification.
- **PBT scope.** Pure logic only: civil↔serial bijection, monotonicity,
  `fmt`/`parse` round-trip, target-path shape, and `pick_latest` global-max
  across quarter/year boundaries. Network and engine wiring get
  recording-fake example tests, as the spike did.

**Delivered.** `src/daily.rs` (pure core + 6 proptest properties),
`src/azdo/wiki.rs` (`get`/`list`/`put`), the `wiki_list` seam method,
`examples/daily.rhai`, and the minimal named-workflow engine:
`azdo run <name>` resolves `{[workflows].dir}/<name>.rhai` (a path-like
target — `.rhai` ext or a separator — is used verbatim), `azdo run
--list` enumerates them, and both need neither PAT nor client. The
shipped script is exercised verbatim by the integration tests. Still
out of scope (S10/S11): op/time limits, a fuller stdlib, and the
remaining `Ops` methods (`set_state`/`comment`/escape) — those land
with S10's composite workflow.

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

## Workflow engine — design locked

The scripting/workflow feature (the eventual home of S10's composite,
`--dry-run` aware flows) is built on the `Ops` seam in `src/ops.rs`:

- `ReadOps` / `WriteOps` / `EscapeOps` split the surface by capability.
  The split *is* the dry-run policy, expressed in types: reads/escape run
  verbatim, writes are stubbed.
- `DryRun` is generic only over `ReadOps + EscapeOps`, so it structurally
  cannot forward a real mutation — the dry-run safety property is a
  compile-time guarantee, not a convention.
- The contract, the dry-run wrapper and a recording test backend ship now;
  the network-backed `RealOps` is the body of S8/S9 (written against the
  same trait, which finally makes those paths unit-testable).

**Rhai spike landed (`azdo run <file.rhai>`).** A deliberately tiny
scripting surface — `task`, `build_start`, `print`, `fail`, `stop`, the
`DRY_RUN` constant — over the `Ops` seam, exercised against the real S8
backend to validate the surface *before* it is frozen. `RealOps` is real
for the spike surface and returns a typed "not wired" error elsewhere
(honest, not a stub success). Sync by design: `Ops` is synchronous
(Rhai is too); `RealOps` owns a dedicated runtime and `block_on`s per
call, and `azdo run` is dispatched outside the shared async runtime so
those `block_on`s are never nested. Deliberately **out** of the spike:
workflow discovery / `--list`, a full stdlib, op/time limits, the
remaining `Ops` methods, and any in-process concurrency (two workflows
at once = two processes, the OS already gives that). `rhai` adds
`smartstring` (MPL-2.0, file-level copyleft, unmodified upstream — no
obligation on our code) and `tiny-keccak` (CC0-1.0); both are narrow
per-crate `deny.toml` exceptions, the global allow-list stays
permissive-only. The engine proper, discovery, and limits are S10/S11.

### Deferred decisions (recorded, not scheduled)

- **Backend-agnostic vocabulary.** `Ops` is already the abstraction seam, so
  another backend (GitHub/Jira/...) is "another impl". Do *not* genericise
  the AzDO-shaped vocabulary on one implementation — revisit only when a
  second backend is a concrete need (two reference points, not a guess).
- **`cargo-mutants`**, scoped to the `Ops` impls and command layer (not the
  whole tree): validates that the seam-level tests actually catch
  regressions. Highest-ROI strictness add; next after `RealOps` exists.
- **TLA+/Alloy** only if work-item transition rules ever move host-side as a
  declarative table. Today they live in user scripts — nothing to specify.
- **Rejected, with reason:** Kani (no `unsafe`/algorithmic surface — verifies
  trivia at high cost); SMT deductive verification (brittle on I/O glue with
  trait objects); OpenAPI codegen (huge, partly inaccurate, fights the
  hand-curated minimal-verb design); making the script language
  non-Turing-complete (does not buy the property — even Nix is Turing
  complete; the real safeguard is the `Ops` capability boundary, already
  present, plus engine op/time limits).

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
