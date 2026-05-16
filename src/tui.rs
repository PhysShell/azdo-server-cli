//! Interactive work item view (ratatui + crossterm).
//!
//! Terminal state is restored on every exit path: a `TerminalGuard`
//! covers normal returns and `?` propagation, and a chained panic hook
//! restores the screen before the default hook prints.

use std::io::{self, stdout, Stdout};
use std::panic::{set_hook, take_hook};
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::azdo::workitem::{self, Comment, WorkItem};
use crate::azdo::AzdoClient;
use crate::browser;
use crate::error::AzdoResult;

/// Comments fetched for the TUI (API caps `$top` at 200).
const TUI_COMMENTS_TOP: u32 = 200;

/// Event poll interval; also the redraw cadence.
const TICK: Duration = Duration::from_millis(100);

/// Prefix shown on the comment-entry line.
const PROMPT: &str = "comment> ";

/// Which pane the scroll keys act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Description,
    Comments,
}

/// All view state. Text panes are pre-joined so drawing allocates nothing.
#[derive(Debug)]
struct App {
    header: String,
    meta: String,
    meta_lines: u16,
    description: String,
    description_lines: u16,
    comments: String,
    comments_lines: u16,
    url: String,
    pane: Pane,
    desc_scroll: u16,
    comments_scroll: u16,
    status: String,
    /// `Some` while the comment-entry line is active; holds the buffer.
    input: Option<String>,
}

impl App {
    fn toggle_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Description => Pane::Comments,
            Pane::Comments => Pane::Description,
        };
    }

    fn scroll_down(&mut self) {
        match self.pane {
            Pane::Description => self.desc_scroll = self.desc_scroll.saturating_add(1),
            Pane::Comments => self.comments_scroll = self.comments_scroll.saturating_add(1),
        }
    }

    fn scroll_up(&mut self) {
        match self.pane {
            Pane::Description => self.desc_scroll = self.desc_scroll.saturating_sub(1),
            Pane::Comments => self.comments_scroll = self.comments_scroll.saturating_sub(1),
        }
    }

    fn start_comment(&mut self) {
        self.input = Some(String::new());
        self.status = String::from("comment: type, Enter to send, Esc to cancel");
    }

    fn input_push(&mut self, ch: char) {
        if let Some(buf) = self.input.as_mut() {
            buf.push(ch);
        }
    }

    fn input_backspace(&mut self) {
        if let Some(buf) = self.input.as_mut() {
            buf.pop();
        }
    }

    fn input_cancel(&mut self) {
        self.input = None;
        self.status = String::from("comment cancelled");
    }
}

/// Saturating count cast for line totals.
fn line_count(text: &str) -> u16 {
    u16::try_from(text.lines().count()).unwrap_or(u16::MAX)
}

/// Largest in-range scroll offset for `content` lines in a `viewport`.
const fn max_scroll(content: u16, viewport: u16) -> u16 {
    content.saturating_sub(viewport)
}

fn build_app(item: &WorkItem, comments: &[Comment], url: String) -> AzdoResult<App> {
    let summary = workitem::summary_lines(item);
    let (header, meta) = summary.split_first().map_or_else(
        || (String::new(), String::new()),
        |(first, rest)| (first.clone(), rest.join("\n")),
    );
    let description = workitem::description_text(item)?;
    let comments = workitem::render_comments(comments)?;

    Ok(App {
        meta_lines: line_count(&meta),
        description_lines: line_count(&description),
        comments_lines: line_count(&comments),
        header,
        meta,
        description,
        comments,
        url,
        pane: Pane::Description,
        desc_scroll: 0,
        comments_scroll: 0,
        status: String::from("ready"),
        input: None,
    })
}

async fn load(client: &AzdoClient, id: u64) -> AzdoResult<App> {
    let item = workitem::fetch(client, id).await?;
    let comments = workitem::fetch_comments(client, id, TUI_COMMENTS_TOP).await?;
    build_app(&item, &comments, client.web_item_url(id))
}

/// Open `url` in the system browser, returning a status string. Never
/// fails the UI.
fn open_in_browser(url: &str) -> String {
    match browser::open(url) {
        Ok(()) => format!("opened in browser: {url}"),
        Err(e) => format!("could not open browser: {e}"),
    }
}

fn split(area: Rect, meta_lines: u16) -> [Rect; 4] {
    Layout::vertical([
        Constraint::Length(meta_lines.saturating_add(2)),
        Constraint::Min(3),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area)
}

fn pane_widget<'a>(name: &str, body: &'a str, scroll: u16, active: bool) -> Paragraph<'a> {
    let (title, style) = if active {
        (
            format!("{name} [active]"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (name.to_owned(), Style::default())
    };
    Paragraph::new(body)
        .block(
            Block::new()
                .borders(Borders::ALL)
                .title(title)
                .border_style(style),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
}

fn ui(frame: &mut Frame<'_>, app: &App) {
    let [header_a, desc_a, comments_a, footer_a] = split(frame.area(), app.meta_lines);

    frame.render_widget(
        Paragraph::new(app.meta.as_str())
            .block(Block::new().borders(Borders::ALL).title(app.header.clone())),
        header_a,
    );
    frame.render_widget(
        pane_widget(
            "Description",
            &app.description,
            app.desc_scroll,
            app.pane == Pane::Description,
        ),
        desc_a,
    );
    frame.render_widget(
        pane_widget(
            "Comments",
            &app.comments,
            app.comments_scroll,
            app.pane == Pane::Comments,
        ),
        comments_a,
    );
    match app.input.as_deref() {
        Some(buf) => {
            frame.render_widget(Paragraph::new(format!("{PROMPT}{buf}")), footer_a);
            let typed = u16::try_from(PROMPT.chars().count().saturating_add(buf.chars().count()))
                .unwrap_or(u16::MAX);
            let max_x = footer_a.x.saturating_add(footer_a.width.saturating_sub(1));
            frame.set_cursor_position((footer_a.x.saturating_add(typed).min(max_x), footer_a.y));
        }
        None => frame.render_widget(
            Paragraph::new(format!(
                "q quit  r refresh  c comment  o open  Tab switch  j/k scroll  |  {}",
                app.status,
            )),
            footer_a,
        ),
    }
}

/// Best-effort terminal restore (idempotent enough to run twice).
fn restore_terminal() -> io::Result<()> {
    let mut out = stdout();
    execute!(out, LeaveAlternateScreen, Show)?;
    disable_raw_mode()
}

fn init_terminal() -> AzdoResult<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

#[derive(Debug)]
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        drop(restore_terminal());
    }
}

fn install_panic_hook() {
    let previous = take_hook();
    set_hook(Box::new(move |info| {
        drop(restore_terminal());
        previous(info);
    }));
}

async fn refresh(client: &AzdoClient, id: u64, app: &mut App) {
    match load(client, id).await {
        Ok(fresh) => {
            let pane = app.pane;
            *app = fresh;
            app.pane = pane;
            app.status = String::from("refreshed");
        }
        Err(e) => app.status = format!("refresh failed: {e}"),
    }
}

async fn submit_comment(client: &AzdoClient, id: u64, app: &mut App) {
    let Some(raw) = app.input.take() else {
        return;
    };
    let body = raw.trim();
    if body.is_empty() {
        app.status = String::from("empty comment, not sent");
        return;
    }
    match workitem::post_comment(client, id, body).await {
        Ok(_) => {
            refresh(client, id, app).await;
            app.status = String::from("comment posted");
        }
        Err(e) => app.status = format!("post failed: {e}"),
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &AzdoClient,
    id: u64,
    app: &mut App,
) -> AzdoResult<()> {
    loop {
        let size = terminal.size()?;
        let [_, desc_a, comments_a, _] =
            split(Rect::new(0, 0, size.width, size.height), app.meta_lines);
        app.desc_scroll = app.desc_scroll.min(max_scroll(
            app.description_lines,
            desc_a.height.saturating_sub(2),
        ));
        app.comments_scroll = app.comments_scroll.min(max_scroll(
            app.comments_lines,
            comments_a.height.saturating_sub(2),
        ));

        let view: &App = app;
        terminal.draw(|frame| ui(frame, view))?;

        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.input.is_some() {
            match key.code {
                KeyCode::Esc => app.input_cancel(),
                KeyCode::Enter => submit_comment(client, id, app).await,
                KeyCode::Backspace => app.input_backspace(),
                KeyCode::Char(ch) => app.input_push(ch),
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') => app.start_comment(),
                KeyCode::Char('r') => refresh(client, id, app).await,
                KeyCode::Char('o') => app.status = open_in_browser(&app.url),
                KeyCode::Tab | KeyCode::BackTab => app.toggle_pane(),
                KeyCode::Char('j') | KeyCode::Down => app.scroll_down(),
                KeyCode::Char('k') | KeyCode::Up => app.scroll_up(),
                _ => {}
            }
        }
    }
}

/// Run the interactive viewer for work item `id`.
pub(crate) async fn run(client: &AzdoClient, id: u64) -> AzdoResult<()> {
    let mut app = load(client, id).await?;
    install_panic_hook();
    let _guard = TerminalGuard;
    let mut terminal = init_terminal()?;
    event_loop(&mut terminal, client, id, &mut app).await
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::{build_app, max_scroll, ui, App, Pane};
    use crate::azdo::workitem::{Comment, WorkItem};

    const ITEM: &str = r#"
{
  "id": 12345,
  "fields": {
    "System.WorkItemType": "Bug",
    "System.State": "Active",
    "System.Title": "Customs declaration form rejects valid TIN",
    "System.AssignedTo": { "displayName": "Ivan Petrov" },
    "System.ChangedDate": "2026-05-14T10:23:00Z",
    "System.Tags": "customs; urgent",
    "System.Description": "<div>The form rejects a valid TIN.</div>"
  }
}
"#;

    const COMMENTS: &str = r#"
{
  "comments": [
    {
      "id": 1,
      "text": "<div>Reproduced on staging.</div>",
      "createdBy": { "displayName": "Maria Ivanova" },
      "createdDate": "2026-05-14T11:30:00Z"
    }
  ]
}
"#;

    fn fixture_app() -> App {
        let item: WorkItem =
            serde_json::from_str(ITEM).unwrap_or_else(|e| panic!("item fixture: {e}"));
        let root: serde_json::Value =
            serde_json::from_str(COMMENTS).unwrap_or_else(|e| panic!("comments fixture: {e}"));
        let arr = root
            .get("comments")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let comments: Vec<Comment> =
            serde_json::from_value(arr).unwrap_or_else(|e| panic!("comments fixture: {e}"));
        build_app(
            &item,
            &comments,
            "http://azdo.local/tfs/Col/Proj/_workitems/edit/12345".to_owned(),
        )
        .expect("build_app must succeed")
    }

    #[test]
    fn renders_fixture_into_buffer() {
        let app = fixture_app();
        let mut term = Terminal::new(TestBackend::new(64, 22)).expect("test backend");
        term.draw(|f| ui(f, &app)).expect("draw must succeed");
        let rendered = format!("{}", term.backend());

        for needle in [
            "#12345",
            "[Bug]",
            "State: Active",
            "Title:",
            "Ivan Petrov",
            "Description [active]",
            "The form rejects a valid TIN.",
            "Comments",
            "Maria Ivanova",
            "Reproduced on staging.",
            "q quit",
        ] {
            assert!(
                rendered.contains(needle),
                "expected {needle:?} in rendered frame:\n{rendered}",
            );
        }
    }

    #[test]
    fn scroll_math_saturates_and_clamps() {
        assert_eq!(max_scroll(10, 4), 6, "in-range max");
        assert_eq!(max_scroll(3, 10), 0, "viewport larger than content");

        let mut app = fixture_app();
        app.pane = Pane::Description;
        app.scroll_up();
        assert_eq!(app.desc_scroll, 0, "scroll up saturates at 0");
        app.scroll_down();
        assert_eq!(app.desc_scroll, 1, "scroll down advances");

        app.toggle_pane();
        assert_eq!(app.pane, Pane::Comments, "Tab switches pane");
        app.scroll_up();
        assert_eq!(app.comments_scroll, 0, "comments scroll saturates");
    }

    #[test]
    fn comment_input_edits_renders_and_cancels() {
        let mut app = fixture_app();
        assert!(app.input.is_none(), "starts outside input mode");

        app.start_comment();
        assert_eq!(app.input.as_deref(), Some(""), "entry buffer opens empty");

        app.input_push('h');
        app.input_push('i');
        app.input_backspace();
        assert_eq!(app.input.as_deref(), Some("h"), "edits mutate the buffer");

        let mut term = Terminal::new(TestBackend::new(40, 16)).expect("test backend");
        term.draw(|f| ui(f, &app)).expect("draw must succeed");
        let rendered = format!("{}", term.backend());
        assert!(
            rendered.contains("comment> h"),
            "prompt + buffer must show on the footer line:\n{rendered}",
        );

        app.input_cancel();
        assert!(app.input.is_none(), "Esc leaves input mode");
        assert_eq!(app.status, "comment cancelled");
    }
}
