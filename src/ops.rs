//! The `Ops` seam: the single contract between *what a workflow wants to do*
//! and *how it actually talks to Azure DevOps and the user*.
//!
//! The seam is split by capability into three traits so that the dry-run
//! safety property is enforced by the type system rather than by discipline:
//!
//! - [`ReadOps`] — pure reads and user IO. Never mutates AzDO state. Always
//!   executes, even under `--dry-run` (otherwise a previewed plan would be a
//!   lie).
//! - [`WriteOps`] — the *only* AzDO-mutating surface. Under `--dry-run` the
//!   [`DryRun`] wrapper replaces every method with "log the intent and return
//!   a fake success", so a preview never changes anything.
//! - [`EscapeOps`] — the deliberately *unclassified* escape hatch (`http`,
//!   `sh`). See its own documentation for why it is a separate trait.
//!
//! The compile-time guarantee lives in [`DryRun`]: it is generic only over
//! `ReadOps + EscapeOps` and holds no [`WriteOps`] value, so its `WriteOps`
//! implementation *cannot* forward a real mutation — there is nothing to
//! forward to, and the code would not compile if it tried.
//!
//! The concrete network-backed implementation (`RealOps`) is intentionally
//! out of scope here: it is the body of the later build/wiki/workflow stages.
//! This module is the contract those stages — and the eventual scripting
//! engine — are written against, plus the dry-run wrapper, both of which are
//! complete and testable today.

#![allow(
    dead_code,
    reason = "the Ops contract lands ahead of its S8-S10 consumers"
)]

/// Numeric work item id, kept as a newtype so it cannot be confused with any
/// other integer that flows through a workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkItemId(pub(crate) u64);

/// A read-only view of a work item, exactly the fields a workflow may observe.
#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub(crate) id: WorkItemId,
    pub(crate) kind: String,
    pub(crate) state: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) tags: Vec<String>,
    pub(crate) assigned_to: Option<String>,
    pub(crate) url: String,
}

/// A request for the [`EscapeOps::http`] hatch.
#[derive(Debug, Clone)]
pub(crate) struct HttpReq {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<String>,
}

/// The outcome of an [`EscapeOps::http`] call.
#[derive(Debug, Clone)]
pub(crate) struct HttpResp {
    pub(crate) status: u16,
    pub(crate) body: String,
}

/// The outcome of an [`EscapeOps::sh`] call.
#[derive(Debug, Clone)]
pub(crate) struct ShOut {
    pub(crate) code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Build parameters passed through to a queued build, as ordered key/value
/// pairs (order is preserved so callers can rely on it).
#[derive(Debug, Clone)]
pub(crate) struct BuildParams(pub(crate) Vec<(String, String)>);

/// A reference to a queued build.
#[derive(Debug, Clone)]
pub(crate) struct BuildRef {
    pub(crate) id: u64,
    pub(crate) url: String,
}

/// Errors any `Ops` method may report. Conversion into the crate-wide
/// `AzdoError` happens at the engine boundary, once that boundary exists.
#[derive(Debug)]
pub(crate) enum OpsError {
    Network(String),
    Http { status: u16, body: String },
    Auth,
    NotFound(WorkItemId),
    Spawn(String),
    Io(String),
}

/// The three — and only three — ways a workflow can end.
///
/// `Done` and `Stop` both exit 0; `Stop` exists solely to carry a reason
/// string for the log. This split is the minimum needed to keep an
/// informative early return ("a task for this release already exists, decide
/// yourself") distinct from a real failure — it is deliberately not richer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The workflow ran to completion. Exit 0.
    Done,
    /// A controlled, informative early return. Exit 0; the reason is logged.
    Stop(String),
    /// The workflow aborted. Maps to a future `AzdoError::Workflow` and the
    /// exit code below.
    Fail(String),
}

impl Outcome {
    /// Process exit code for this outcome: `Done`/`Stop` succeed, `Fail` uses
    /// a dedicated code (6) that slots in after the existing `error::ExitCode`
    /// range so a workflow abort is distinguishable from transport/HTTP
    /// failures. Wiring this into `AzdoError` happens with the engine.
    pub(crate) const fn exit_code(&self) -> u8 {
        match *self {
            Self::Done | Self::Stop(_) => 0,
            Self::Fail(_) => 6,
        }
    }
}

/// Pure reads and user IO. Nothing here can mutate AzDO state, so every
/// method is safe to run verbatim under `--dry-run` (and must be, or a
/// previewed plan would not reflect reality — `prompt`/`confirm` included,
/// since the plan should show the choices the user actually made).
pub(crate) trait ReadOps {
    /// Fetch a single work item as a read-only [`Task`].
    fn task(&self, id: WorkItemId) -> Result<Task, OpsError>;
    /// The caller's open work items.
    fn my_open(&self) -> Result<Vec<Task>, OpsError>;
    /// Read a wiki page's content by path.
    fn wiki_get(&self, path: &str) -> Result<String, OpsError>;

    /// Print a line to the user.
    fn print(&self, msg: &str);
    /// Ask the user for a line of text.
    fn prompt(&self, msg: &str) -> Result<String, OpsError>;
    /// Ask the user a yes/no question.
    fn confirm(&self, msg: &str) -> Result<bool, OpsError>;
    /// Ask the user to pick one of `opts`; returns the chosen index.
    fn choose(&self, msg: &str, opts: &[String]) -> Result<usize, OpsError>;
}

/// The only AzDO-mutating surface. Under `--dry-run` the [`DryRun`] wrapper
/// substitutes every method with a logged no-op, so previews never write.
///
/// `[states]` alias resolution is a host concern and happens inside the real
/// [`WriteOps::set_state`] implementation — scripts pass the alias, the host
/// resolves it, exactly as the existing `set-state` command already does.
pub(crate) trait WriteOps {
    /// Set the work item state (accepts a raw state name or a `[states]`
    /// alias; the alias is resolved host-side).
    fn set_state(&self, id: WorkItemId, state: &str) -> Result<(), OpsError>;
    /// Post a comment to the work item.
    fn comment(&self, id: WorkItemId, body: &str) -> Result<(), OpsError>;
    /// Queue a build and return a reference to it.
    fn build_start(&self, def: &str, params: BuildParams) -> Result<BuildRef, OpsError>;
    /// Create or overwrite a wiki page.
    fn wiki_put(&self, path: &str, content: &str) -> Result<(), OpsError>;
}

/// The escape hatch: arbitrary HTTP (`http`) and subprocess (`sh`) calls,
/// used by a workflow to talk to *other* services and to express
/// pipeline-specific checks that `azdo` itself knows nothing about.
///
/// Why this is a separate trait, and not folded into [`ReadOps`]:
///
/// 1. **Honest types.** An `http` POST is plainly not a read. Keeping it out
///    of [`ReadOps`] preserves that trait's invariant — "executing this can
///    never mutate external state" — as something a reader can trust. A
///    distinct, documented trait names the hazard at every bound and impl.
/// 2. **Unclassifiable by the host.** The host cannot tell whether a given
///    `http`/`sh` call reads or mutates a foreign system. We deliberately do
///    *not* guess. Under `--dry-run` these run for real (so a plan that
///    branches on, say, a pipeline-stage query stays faithful); guarding a
///    *mutating* escape call is the script's responsibility, via the
///    `DRY_RUN` constant the engine exposes — the same trust level as a
///    `$(...)` in a Makefile.
/// 3. **An extension point at zero present cost.** Because escape is its own
///    trait, a future "block mutating escape under dry-run" policy can be a
///    wrapper over `EscapeOps` alone, without entangling reads.
pub(crate) trait EscapeOps {
    /// Perform an arbitrary HTTP request.
    fn http(&self, req: HttpReq) -> Result<HttpResp, OpsError>;
    /// Run a subprocess with explicit arguments (no shell interpolation).
    fn sh(&self, cmd: &str, args: &[String]) -> Result<ShOut, OpsError>;
}

/// The full backend surface a workflow engine holds. The blanket impl makes
/// every `ReadOps + WriteOps + EscapeOps` value an `Ops` — including
/// `DryRun<RealOps>` — so the engine can keep a single `dyn Ops` and stay
/// agnostic to whether it is previewing or executing.
pub(crate) trait Ops: ReadOps + WriteOps + EscapeOps {}
impl<T: ReadOps + WriteOps + EscapeOps> Ops for T {}

/// `--dry-run` wrapper.
///
/// Parameterised over `ReadOps + EscapeOps` *only* — never [`WriteOps`].
/// Reads, user IO and the escape hatch are delegated unchanged; the
/// [`WriteOps`] implementation logs the intended mutation and returns a fake
/// success. See the `WriteOps for DryRun` impl for the compile-time argument
/// that it cannot forward a real write.
#[derive(Debug)]
pub(crate) struct DryRun<R: ReadOps + EscapeOps> {
    inner: R,
}

impl<R: ReadOps + EscapeOps> DryRun<R> {
    pub(crate) const fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Unwrap the dry-run shell, returning the wrapped backend.
    pub(crate) fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: ReadOps + EscapeOps> ReadOps for DryRun<R> {
    fn task(&self, id: WorkItemId) -> Result<Task, OpsError> {
        self.inner.task(id)
    }
    fn my_open(&self) -> Result<Vec<Task>, OpsError> {
        self.inner.my_open()
    }
    fn wiki_get(&self, path: &str) -> Result<String, OpsError> {
        self.inner.wiki_get(path)
    }
    fn print(&self, msg: &str) {
        self.inner.print(msg);
    }
    fn prompt(&self, msg: &str) -> Result<String, OpsError> {
        self.inner.prompt(msg)
    }
    fn confirm(&self, msg: &str) -> Result<bool, OpsError> {
        self.inner.confirm(msg)
    }
    fn choose(&self, msg: &str, opts: &[String]) -> Result<usize, OpsError> {
        self.inner.choose(msg, opts)
    }
}

impl<R: ReadOps + EscapeOps> EscapeOps for DryRun<R> {
    // By design: escape calls run for real even under dry-run, so a plan that
    // branches on a foreign-service query stays faithful. Mutation safety on
    // these calls is the script's responsibility (the `DRY_RUN` constant).
    fn http(&self, req: HttpReq) -> Result<HttpResp, OpsError> {
        self.inner.http(req)
    }
    fn sh(&self, cmd: &str, args: &[String]) -> Result<ShOut, OpsError> {
        self.inner.sh(cmd, args)
    }
}

impl<R: ReadOps + EscapeOps> WriteOps for DryRun<R> {
    // COMPILE-TIME GUARANTEE: the only value in scope is `self.inner: R`, and
    // `R: ReadOps + EscapeOps` — never `WriteOps`. There is no write method to
    // forward to; `self.inner.set_state(..)` would not type-check. Dry-run
    // therefore *cannot* reach a real mutation, enforced by the compiler, not
    // by convention.
    fn set_state(&self, id: WorkItemId, state: &str) -> Result<(), OpsError> {
        self.inner
            .print(&format!("[dry-run] set_state #{} -> {state}", id.0));
        Ok(())
    }
    fn comment(&self, id: WorkItemId, _body: &str) -> Result<(), OpsError> {
        self.inner.print(&format!("[dry-run] comment #{}", id.0));
        Ok(())
    }
    fn build_start(&self, def: &str, _params: BuildParams) -> Result<BuildRef, OpsError> {
        self.inner.print(&format!("[dry-run] build_start {def}"));
        Ok(BuildRef {
            id: 0,
            url: "dry-run://build".to_owned(),
        })
    }
    fn wiki_put(&self, path: &str, _content: &str) -> Result<(), OpsError> {
        self.inner.print(&format!("[dry-run] wiki_put {path}"));
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use std::cell::RefCell;

    use super::{
        BuildParams, EscapeOps, HttpReq, HttpResp, Outcome, ReadOps, ShOut, Task, WorkItemId,
        WriteOps,
    };

    /// A `ReadOps + EscapeOps + WriteOps` backend that records every call as a
    /// string. The recorder makes the dry-run behaviour observable: reads and
    /// escape must reach it, writes must not.
    #[derive(Debug, Default)]
    struct Recording {
        calls: RefCell<Vec<String>>,
    }

    impl Recording {
        fn log(&self, s: &str) {
            self.calls.borrow_mut().push(s.to_owned());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl ReadOps for Recording {
        fn task(&self, id: WorkItemId) -> Result<Task, super::OpsError> {
            self.log(&format!("task {}", id.0));
            Ok(Task {
                id,
                kind: "Bug".to_owned(),
                state: "Active".to_owned(),
                title: "t".to_owned(),
                description: String::new(),
                tags: vec![],
                assigned_to: None,
                url: "u".to_owned(),
            })
        }
        fn my_open(&self) -> Result<Vec<Task>, super::OpsError> {
            self.log("my_open");
            Ok(vec![])
        }
        fn wiki_get(&self, path: &str) -> Result<String, super::OpsError> {
            self.log(&format!("wiki_get {path}"));
            Ok(String::new())
        }
        fn print(&self, msg: &str) {
            self.log(&format!("print {msg}"));
        }
        fn prompt(&self, _msg: &str) -> Result<String, super::OpsError> {
            self.log("prompt");
            Ok(String::new())
        }
        fn confirm(&self, _msg: &str) -> Result<bool, super::OpsError> {
            self.log("confirm");
            Ok(true)
        }
        fn choose(&self, _msg: &str, _opts: &[String]) -> Result<usize, super::OpsError> {
            self.log("choose");
            Ok(0)
        }
    }

    impl EscapeOps for Recording {
        fn http(&self, req: HttpReq) -> Result<HttpResp, super::OpsError> {
            self.log(&format!("http {} {}", req.method, req.url));
            Ok(HttpResp {
                status: 200,
                body: String::new(),
            })
        }
        fn sh(&self, cmd: &str, _args: &[String]) -> Result<ShOut, super::OpsError> {
            self.log(&format!("sh {cmd}"));
            Ok(ShOut {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    impl WriteOps for Recording {
        fn set_state(&self, id: WorkItemId, state: &str) -> Result<(), super::OpsError> {
            self.log(&format!("set_state {} {state}", id.0));
            Ok(())
        }
        fn comment(&self, id: WorkItemId, body: &str) -> Result<(), super::OpsError> {
            self.log(&format!("comment {} {body}", id.0));
            Ok(())
        }
        fn build_start(
            &self,
            def: &str,
            _params: BuildParams,
        ) -> Result<super::BuildRef, super::OpsError> {
            self.log(&format!("build_start {def}"));
            Ok(super::BuildRef {
                id: 1,
                url: "real".to_owned(),
            })
        }
        fn wiki_put(&self, path: &str, _content: &str) -> Result<(), super::OpsError> {
            self.log(&format!("wiki_put {path}"));
            Ok(())
        }
    }

    #[test]
    fn outcome_exit_codes() {
        assert_eq!(Outcome::Done.exit_code(), 0, "Done succeeds");
        assert_eq!(
            Outcome::Stop("x".to_owned()).exit_code(),
            0,
            "an informative Stop is not a failure",
        );
        assert_eq!(
            Outcome::Fail("x".to_owned()).exit_code(),
            6,
            "a workflow abort uses the dedicated code 6",
        );
    }

    #[test]
    fn dry_run_passes_reads_and_escape_through_to_backend() {
        let dry = super::DryRun::new(Recording::default());
        dry.task(WorkItemId(7)).expect("read ok");
        drop(dry.my_open().expect("read ok"));
        drop(
            dry.http(HttpReq {
                method: "GET".to_owned(),
                url: "svc".to_owned(),
                headers: vec![],
                body: None,
            })
            .expect("escape ok"),
        );
        assert_eq!(
            dry.into_inner().calls(),
            vec![
                "task 7".to_owned(),
                "my_open".to_owned(),
                "http GET svc".to_owned(),
            ],
            "reads and escape reach the backend unchanged",
        );
    }

    #[test]
    fn dry_run_stubs_writes_without_touching_backend() {
        let dry = super::DryRun::new(Recording::default());
        dry.set_state(WorkItemId(42), "Done").expect("stubbed");
        dry.comment(WorkItemId(42), "hi").expect("stubbed");
        drop(
            dry.build_start("Rel", BuildParams(vec![]))
                .expect("stubbed"),
        );
        dry.wiki_put("/p", "c").expect("stubbed");
        // Every write became a `[dry-run] ...` print on the inner backend;
        // not one mutating call reached it. The compiler already guarantees
        // no real write could be forwarded — this locks the logged-intent
        // behaviour and the exact wording.
        let logged = dry.into_inner().calls();
        assert!(
            logged.iter().all(|c| c.starts_with("print [dry-run] ")),
            "writes must only produce dry-run prints, got {logged:?}",
        );
        assert!(
            !logged.iter().any(|c| c.starts_with("set_state ")
                || c.starts_with("comment ")
                || c.starts_with("build_start ")
                || c.starts_with("wiki_put ")),
            "no real mutating call may reach the backend, got {logged:?}",
        );
    }
}
