//! Rhai workflow spike: a deliberately tiny scripting surface over the
//! `Ops` seam, validated against one real backend (S8 `build_start`).
//!
//! Scope is intentionally minimal — `task`, `build_start`, `print`, `fail`,
//! `stop`, and the `DRY_RUN` constant. It exists to pressure-test the
//! scripting surface *before* it is frozen, not to be the engine. Workflow
//! discovery, a full stdlib, op/time limits, and the remaining `Ops`
//! methods are later stages; methods outside the spike surface return a
//! typed "not wired" error rather than a stub success, so a script that
//! strays off the surface fails honestly instead of lying.
//!
//! Sync by design: `Ops` is synchronous (Rhai is too), and `RealOps`
//! bridges to the async client by owning a dedicated runtime and blocking
//! on it per call. `azdo run` is dispatched outside the shared async
//! runtime (see `main::dispatch`), so these `block_on`s are never nested.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position, Scope};
use tokio::runtime::{Builder, Runtime};

use crate::azdo::{build, wiki, workitem, AzdoClient};
use crate::config::Config;
use crate::daily;
use crate::error::AzdoError;
use crate::ops::{
    BuildParams, BuildRef, DryRun, EscapeOps, HttpReq, HttpResp, Ops, OpsError, Outcome, ReadOps,
    ShOut, Task, WorkItemId, WriteOps,
};

/// Marker for methods deliberately outside the spike surface.
const UNWIRED: &str = "not wired in the S8 + Rhai spike stage";

/// Network-backed `Ops`. Real for the spike surface (`task`, `build_start`,
/// `print`); honest typed errors elsewhere.
pub(crate) struct RealOps {
    client: AzdoClient,
    builds: BTreeMap<String, u32>,
    wiki_id: Option<String>,
    rt: Runtime,
}

impl RealOps {
    pub(crate) fn new(client: &AzdoClient, cfg: &Config) -> Result<Self, AzdoError> {
        let builds = cfg
            .products
            .iter()
            .filter_map(|(name, p)| p.build_definition_id.map(|id| (name.clone(), id)))
            .collect();
        let wiki_id = cfg.wiki.as_ref().map(|w| w.id.clone());
        let rt = Builder::new_current_thread().enable_all().build()?;
        Ok(Self {
            client: client.clone(),
            builds,
            wiki_id,
            rt,
        })
    }

    /// The configured wiki identifier, or a typed error naming the missing
    /// `[wiki]` config section so a script fails honestly off-surface.
    fn wiki_id(&self) -> Result<&str, OpsError> {
        self.wiki_id
            .as_deref()
            .ok_or_else(|| OpsError::Io("no `[wiki]` section in config".to_owned()))
    }
}

fn to_ops_err(e: AzdoError) -> OpsError {
    match e {
        AzdoError::Http { status, body, .. } => OpsError::Http { status, body },
        AzdoError::Transport(t) => OpsError::Network(t.to_string()),
        AzdoError::PatMissing(_) => OpsError::Auth,
        other => OpsError::Io(other.to_string()),
    }
}

impl ReadOps for RealOps {
    fn task(&self, id: WorkItemId) -> Result<Task, OpsError> {
        let item = self
            .rt
            .block_on(workitem::fetch(&self.client, id.0))
            .map_err(to_ops_err)?;
        let description = workitem::description_text(&item).map_err(to_ops_err)?;
        Ok(Task {
            id,
            kind: item.work_item_type().to_owned(),
            state: item.state().to_owned(),
            title: item.title().to_owned(),
            description,
            tags: Vec::new(),
            assigned_to: None,
            url: self.client.web_item_url(item.id),
        })
    }
    fn my_open(&self) -> Result<Vec<Task>, OpsError> {
        Err(OpsError::Io(format!("my_open: {UNWIRED}")))
    }
    fn wiki_get(&self, path: &str) -> Result<String, OpsError> {
        let id = self.wiki_id()?;
        self.rt
            .block_on(wiki::get_page(&self.client, id, path))
            .map_err(to_ops_err)
    }
    fn wiki_list(&self, root: &str) -> Result<Vec<String>, OpsError> {
        let id = self.wiki_id()?;
        self.rt
            .block_on(wiki::list_pages(&self.client, id, root))
            .map_err(to_ops_err)
    }
    fn print(&self, msg: &str) {
        println!("{msg}");
    }
    fn prompt(&self, _msg: &str) -> Result<String, OpsError> {
        Err(OpsError::Io(format!("prompt: {UNWIRED}")))
    }
    fn confirm(&self, _msg: &str) -> Result<bool, OpsError> {
        Err(OpsError::Io(format!("confirm: {UNWIRED}")))
    }
    fn choose(&self, _msg: &str, _opts: &[String]) -> Result<usize, OpsError> {
        Err(OpsError::Io(format!("choose: {UNWIRED}")))
    }
}

impl WriteOps for RealOps {
    fn set_state(&self, _id: WorkItemId, _state: &str) -> Result<(), OpsError> {
        Err(OpsError::Io(format!("set_state: {UNWIRED}")))
    }
    fn comment(&self, _id: WorkItemId, _body: &str) -> Result<(), OpsError> {
        Err(OpsError::Io(format!("comment: {UNWIRED}")))
    }
    fn build_start(&self, def: &str, params: BuildParams) -> Result<BuildRef, OpsError> {
        let def_id =
            self.builds.get(def).copied().ok_or_else(|| {
                OpsError::Io(format!("no build_definition_id for product `{def}`"))
            })?;
        let parameters = if params.0.is_empty() {
            None
        } else {
            let map: BTreeMap<&str, &str> = params
                .0
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            Some(serde_json::to_string(&map).map_err(|e| OpsError::Io(e.to_string()))?)
        };
        let b = self
            .rt
            .block_on(build::queue_build(&self.client, def_id, parameters))
            .map_err(to_ops_err)?;
        Ok(BuildRef {
            id: b.id,
            url: self
                .client
                .project_url(&format!("/_build/results?buildId={}", b.id)),
        })
    }
    fn wiki_put(&self, path: &str, content: &str) -> Result<(), OpsError> {
        let id = self.wiki_id()?;
        self.rt
            .block_on(wiki::put_page(&self.client, id, path, content))
            .map_err(to_ops_err)
    }
}

impl EscapeOps for RealOps {
    fn http(&self, _req: HttpReq) -> Result<HttpResp, OpsError> {
        Err(OpsError::Io(format!("http: {UNWIRED}")))
    }
    fn sh(&self, _cmd: &str, _args: &[String]) -> Result<ShOut, OpsError> {
        Err(OpsError::Io(format!("sh: {UNWIRED}")))
    }
}

// Rhai's fallible host-fn ABI is `Result<T, Box<EvalAltResult>>`, so these
// error constructors must hand back a `Box<EvalAltResult>` — the box is
// required by the API, not an avoidable indirection.
#[allow(
    clippy::unnecessary_box_returns,
    reason = "Rhai's fallible host-fn ABI requires Box<EvalAltResult>"
)]
/// Unwinding error used by `fail`/`stop` to halt the script; its payload is
/// ignored — the real outcome is read from the shared cell.
fn workflow_halt() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from("workflow halted".to_owned()),
        Position::NONE,
    ))
}

#[allow(
    clippy::unnecessary_box_returns,
    reason = "Rhai's fallible host-fn ABI requires Box<EvalAltResult>"
)]
fn ops_to_rhai(e: &OpsError) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(format!("{e:?}")),
        Position::NONE,
    ))
}

#[allow(
    clippy::unnecessary_box_returns,
    reason = "Rhai's fallible host-fn ABI requires Box<EvalAltResult>"
)]
fn rhai_err(msg: &str) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(msg.to_owned()),
        Position::NONE,
    ))
}

fn task_to_map(t: Task) -> Result<Map, Box<EvalAltResult>> {
    let id = i64::try_from(t.id.0).map_err(|_| rhai_err("task id overflows i64"))?;
    let mut m = Map::new();
    drop(m.insert("id".into(), id.into()));
    drop(m.insert("kind".into(), t.kind.into()));
    drop(m.insert("state".into(), t.state.into()));
    drop(m.insert("title".into(), t.title.into()));
    drop(m.insert("description".into(), t.description.into()));
    drop(m.insert("url".into(), t.url.into()));
    Ok(m)
}

/// Register the wiki verbs (`wiki_list`/`wiki_read`/`wiki_create`, over the
/// `Ops` seam) and the pure host helpers (`daily_target_path`/`pick_latest`,
/// whose gnarly date/path math lives in `crate::daily` so property-based
/// tests pin it — the script only orchestrates the flow).
fn register_daily_surface(engine: &mut Engine, ops: &Rc<dyn Ops>) {
    let ops_wlist = Rc::clone(ops);
    engine.register_fn(
        "wiki_list",
        move |root: &str| -> Result<Array, Box<EvalAltResult>> {
            let paths = ops_wlist.wiki_list(root).map_err(|e| ops_to_rhai(&e))?;
            Ok(paths.into_iter().map(Dynamic::from).collect())
        },
    );

    let ops_wread = Rc::clone(ops);
    engine.register_fn(
        "wiki_read",
        move |path: &str| -> Result<String, Box<EvalAltResult>> {
            ops_wread.wiki_get(path).map_err(|e| ops_to_rhai(&e))
        },
    );

    let ops_wcreate = Rc::clone(ops);
    engine.register_fn(
        "wiki_create",
        move |path: &str, content: &str| -> Result<(), Box<EvalAltResult>> {
            ops_wcreate
                .wiki_put(path, content)
                .map_err(|e| ops_to_rhai(&e))
        },
    );

    engine.register_fn("daily_target_path", move |root: &str| -> String {
        daily::daily_page_path(root, daily::today_utc())
    });
    engine.register_fn("pick_latest", move |paths: Array| -> Dynamic {
        let owned: Vec<String> = paths
            .into_iter()
            .filter_map(|d| d.into_string().ok())
            .collect();
        daily::pick_latest(&owned).map_or(Dynamic::UNIT, Dynamic::from)
    });
}

/// Build the engine, run `src`, and reduce the result to an [`Outcome`].
/// Pure over any `Rc<dyn Ops>`, so tests drive it with a recording fake and
/// no network.
fn run_source(ops: &Rc<dyn Ops>, dry_run: bool, daily_root: Option<&str>, src: &str) -> Outcome {
    let mut engine = Engine::new();

    let ops_print = Rc::clone(ops);
    engine.on_print(move |s| ops_print.print(s));

    let ops_task = Rc::clone(ops);
    engine.register_fn("task", move |id: i64| -> Result<Map, Box<EvalAltResult>> {
        let id = u64::try_from(id).map_err(|_| rhai_err("task: id must be >= 0"))?;
        let t = ops_task.task(WorkItemId(id)).map_err(|e| ops_to_rhai(&e))?;
        task_to_map(t)
    });

    let ops_build = Rc::clone(ops);
    engine.register_fn(
        "build_start",
        move |def: &str, params: Map| -> Result<i64, Box<EvalAltResult>> {
            let bp = BuildParams(
                params
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            );
            let r = ops_build
                .build_start(def, bp)
                .map_err(|e| ops_to_rhai(&e))?;
            i64::try_from(r.id).map_err(|_| rhai_err("build id overflows i64"))
        },
    );

    register_daily_surface(&mut engine, ops);

    let result: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));

    let r_fail = Rc::clone(&result);
    engine.register_fn("fail", move |msg: &str| -> Result<(), Box<EvalAltResult>> {
        *r_fail.borrow_mut() = Some(Outcome::Fail(msg.to_owned()));
        Err(workflow_halt())
    });
    let r_stop = Rc::clone(&result);
    engine.register_fn("stop", move |msg: &str| -> Result<(), Box<EvalAltResult>> {
        *r_stop.borrow_mut() = Some(Outcome::Stop(msg.to_owned()));
        Err(workflow_halt())
    });

    let mut scope = Scope::new();
    scope.push_constant("DRY_RUN", dry_run);
    if let Some(root) = daily_root {
        scope.push_constant("DAILY_ROOT", root.to_owned());
    }

    match engine.run_with_scope(&mut scope, src) {
        Ok(()) => result.borrow_mut().take().unwrap_or(Outcome::Done),
        Err(e) => result
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Outcome::Fail(format!("script error: {e}"))),
    }
}

/// Read and run a `.rhai` workflow file; returns the process exit code.
pub(crate) fn run_file(
    client: &AzdoClient,
    cfg: &Config,
    path: &Path,
    dry_run: bool,
) -> Result<u8, AzdoError> {
    let src = fs::read_to_string(path)?;
    let real = RealOps::new(client, cfg)?;
    let ops: Rc<dyn Ops> = if dry_run {
        Rc::new(DryRun::new(real))
    } else {
        Rc::new(real)
    };
    let daily_root = cfg.wiki.as_ref().map(|w| w.daily_path.as_str());
    let outcome = run_source(&ops, dry_run, daily_root, &src);
    match &outcome {
        Outcome::Done => {}
        Outcome::Stop(reason) => println!("stop: {reason}"),
        Outcome::Fail(reason) => eprintln!("fail: {reason}"),
    }
    Ok(outcome.exit_code())
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
    use std::rc::Rc;

    use super::{daily, run_source, BuildParams, BuildRef, EscapeOps, HttpReq, HttpResp, Ops};
    use super::{OpsError, Outcome, ReadOps, ShOut, Task, WorkItemId, WriteOps};

    /// Records every call; `task` returns a canned item unless `task_fails`.
    /// `pages`/`body` seed the wiki reads for the daily-workflow tests.
    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        task_fails: bool,
        pages: Vec<String>,
        body: String,
    }

    impl Fake {
        fn log(&self, s: &str) {
            self.calls.borrow_mut().push(s.to_owned());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl ReadOps for Fake {
        fn task(&self, id: WorkItemId) -> Result<Task, OpsError> {
            self.log(&format!("task {}", id.0));
            if self.task_fails {
                return Err(OpsError::NotFound(id));
            }
            Ok(Task {
                id,
                kind: "Bug".to_owned(),
                state: "Ready for Release".to_owned(),
                title: "ship it".to_owned(),
                description: String::new(),
                tags: vec![],
                assigned_to: None,
                url: "u".to_owned(),
            })
        }
        fn my_open(&self) -> Result<Vec<Task>, OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
        fn wiki_get(&self, p: &str) -> Result<String, OpsError> {
            self.log(&format!("wiki_get {p}"));
            Ok(self.body.clone())
        }
        fn wiki_list(&self, r: &str) -> Result<Vec<String>, OpsError> {
            self.log(&format!("wiki_list {r}"));
            Ok(self.pages.clone())
        }
        fn print(&self, msg: &str) {
            self.log(&format!("print {msg}"));
        }
        fn prompt(&self, _m: &str) -> Result<String, OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
        fn confirm(&self, _m: &str) -> Result<bool, OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
        fn choose(&self, _m: &str, _o: &[String]) -> Result<usize, OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
    }

    impl WriteOps for Fake {
        fn set_state(&self, _id: WorkItemId, _s: &str) -> Result<(), OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
        fn comment(&self, _id: WorkItemId, _b: &str) -> Result<(), OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
        fn build_start(&self, def: &str, params: BuildParams) -> Result<BuildRef, OpsError> {
            self.log(&format!("build_start {def} {:?}", params.0));
            Ok(BuildRef {
                id: 9001,
                url: "b".to_owned(),
            })
        }
        fn wiki_put(&self, p: &str, c: &str) -> Result<(), OpsError> {
            self.log(&format!("wiki_put {p} {c}"));
            Ok(())
        }
    }

    impl EscapeOps for Fake {
        fn http(&self, _r: HttpReq) -> Result<HttpResp, OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
        fn sh(&self, _c: &str, _a: &[String]) -> Result<ShOut, OpsError> {
            Err(OpsError::Io("x".to_owned()))
        }
    }

    fn drive(fake: &Rc<Fake>, dry_run: bool, src: &str) -> Outcome {
        let concrete: Rc<Fake> = Rc::clone(fake);
        let ops: Rc<dyn Ops> = concrete;
        run_source(&ops, dry_run, None, src)
    }

    fn drive_root(fake: &Rc<Fake>, dry_run: bool, root: &str, src: &str) -> Outcome {
        let concrete: Rc<Fake> = Rc::clone(fake);
        let ops: Rc<dyn Ops> = concrete;
        run_source(&ops, dry_run, Some(root), src)
    }

    /// The shipped daily workflow, exercised verbatim so the artifact
    /// itself is under test, not a paraphrase.
    const DAILY_SRC: &str = include_str!("../examples/daily.rhai");

    /// Today's expected target path, computed through the same pure core
    /// the script calls, so the assertion tracks the clock.
    fn today_target(root: &str) -> String {
        daily::daily_page_path(root, daily::today_utc())
    }

    #[test]
    fn happy_path_reads_task_prints_and_queues_build() {
        let fake = Rc::new(Fake::default());
        let outcome = drive(
            &fake,
            false,
            r#"
                let t = task(12345);
                print(`#${t.id} ${t.state}`);
                let b = build_start("declaration", #{ "WorkItemId": "12345" });
                print(`build ${b}`);
            "#,
        );
        assert_eq!(outcome, Outcome::Done, "clean script ends Done");
        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(
            fake.calls(),
            vec![
                "task 12345".to_owned(),
                "print #12345 Ready for Release".to_owned(),
                r#"build_start declaration [("WorkItemId", "12345")]"#.to_owned(),
                "print build 9001".to_owned(),
            ],
            "the spike surface drives reads, prints and the build verbatim",
        );
    }

    #[test]
    fn fail_sets_outcome_and_halts_immediately() {
        let fake = Rc::new(Fake::default());
        let outcome = drive(
            &fake,
            false,
            r#"
                task(1);
                fail("not ready");
                build_start("declaration", #{});
            "#,
        );
        assert_eq!(outcome, Outcome::Fail("not ready".to_owned()));
        assert_eq!(outcome.exit_code(), 6, "Fail uses the dedicated code 6");
        assert_eq!(
            fake.calls(),
            vec!["task 1".to_owned()],
            "fail() must halt before the build is queued",
        );
    }

    #[test]
    fn stop_is_an_informative_success() {
        let fake = Rc::new(Fake::default());
        let outcome = drive(&fake, false, r#"stop("already shipped");"#);
        assert_eq!(outcome, Outcome::Stop("already shipped".to_owned()));
        assert_eq!(outcome.exit_code(), 0, "Stop is not a failure");
    }

    #[test]
    fn dry_run_constant_is_visible_to_the_script() {
        let fake = Rc::new(Fake::default());
        let dry = drive(
            &fake,
            true,
            r#"if DRY_RUN { print("dry") } else { print("wet") }"#,
        );
        assert_eq!(dry, Outcome::Done);
        assert_eq!(fake.calls(), vec!["print dry".to_owned()]);

        let fake2 = Rc::new(Fake::default());
        drop(drive(
            &fake2,
            false,
            r#"if DRY_RUN { print("dry") } else { print("wet") }"#,
        ));
        assert_eq!(fake2.calls(), vec!["print wet".to_owned()]);
    }

    #[test]
    fn script_error_becomes_fail() {
        let fake = Rc::new(Fake::default());
        let outcome = drive(&fake, false, "this is not valid rhai @@@");
        match outcome {
            Outcome::Fail(msg) => assert!(
                msg.starts_with("script error:"),
                "a broken script is a Fail with a script-error reason, got {msg:?}",
            ),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn ops_error_propagates_as_fail() {
        let fake = Rc::new(Fake {
            calls: RefCell::new(vec![]),
            task_fails: true,
            ..Default::default()
        });
        let outcome = drive(&fake, false, "task(7);");
        match outcome {
            Outcome::Fail(msg) => assert!(
                msg.contains("NotFound"),
                "an Ops error must surface in the Fail reason, got {msg:?}",
            ),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn daily_creates_today_from_the_latest_page() {
        let root = "Daily";
        let fake = Rc::new(Fake {
            pages: vec![
                "Daily/2099/Q1/05.02.2099".to_owned(),
                "Daily/2099/Q3/30.09.2099".to_owned(),
                "Daily/2099/Q2/14.05.2099".to_owned(),
            ],
            body: "yesterday's notes".to_owned(),
            ..Default::default()
        });
        let outcome = drive_root(&fake, false, root, DAILY_SRC);
        assert_eq!(outcome, Outcome::Done, "a clean daily run ends Done");

        let target = today_target(root);
        let calls = fake.calls();
        assert!(
            calls.contains(&"wiki_list Daily".to_owned()),
            "must list the subtree once, got {calls:?}",
        );
        assert!(
            calls.contains(&"wiki_get Daily/2099/Q3/30.09.2099".to_owned()),
            "must read the chronologically latest page (not the lexical \
             max, not last in the list), got {calls:?}",
        );
        assert!(
            calls.contains(&format!("wiki_put {target} yesterday's notes")),
            "must create today's page seeded with the latest body, got {calls:?}",
        );
    }

    #[test]
    fn daily_refuses_to_overwrite_an_existing_page() {
        let root = "Daily";
        let target = today_target(root);
        let fake = Rc::new(Fake {
            pages: vec![target.clone(), "Daily/2099/Q1/01.01.2099".to_owned()],
            body: "x".to_owned(),
            ..Default::default()
        });
        let outcome = drive_root(&fake, false, root, DAILY_SRC);
        match outcome {
            Outcome::Stop(msg) => assert!(
                msg.contains(&target),
                "the stop reason must name the existing page, got {msg:?}",
            ),
            other => panic!("expected Stop, got {other:?}"),
        }
        assert!(
            !fake.calls().iter().any(|c| c.starts_with("wiki_put ")),
            "an existing page must never be overwritten, got {:?}",
            fake.calls(),
        );
    }

    #[test]
    fn daily_fails_when_there_is_nothing_to_copy() {
        let fake = Rc::new(Fake {
            pages: vec![],
            body: String::new(),
            ..Default::default()
        });
        let outcome = drive_root(&fake, false, "Daily", DAILY_SRC);
        match outcome {
            Outcome::Fail(msg) => assert!(
                msg.contains("no existing daily page"),
                "must fail clearly when the tree has no prior page, got {msg:?}",
            ),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn daily_dry_run_previews_without_creating() {
        let root = "Daily";
        let fake = Rc::new(Fake {
            pages: vec!["Daily/2099/Q2/14.05.2099".to_owned()],
            body: "seed".to_owned(),
            ..Default::default()
        });
        let outcome = drive_root(&fake, true, root, DAILY_SRC);
        assert_eq!(outcome, Outcome::Done, "a dry-run still completes");

        let calls = fake.calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("wiki_put ")),
            "dry-run must not create the page, got {calls:?}",
        );
        assert!(
            calls.iter().any(|c| c.contains("[dry-run]")),
            "dry-run must announce the preview, got {calls:?}",
        );
    }
}
