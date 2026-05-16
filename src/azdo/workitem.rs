use reqwest::header::{HeaderValue, CONTENT_TYPE};
use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::AzdoClient;
use crate::error::{AzdoError, AzdoResult};

/// Column width used when flattening the HTML description to text.
const DESC_WIDTH: usize = 100;

/// A work item as returned by `GET /_apis/wit/workitems/{id}`.
#[derive(Debug, Deserialize)]
pub(crate) struct WorkItem {
    pub(crate) id: u64,
    pub(crate) fields: Fields,
}

impl WorkItem {
    pub(crate) fn state(&self) -> &str {
        &self.fields.state
    }
}

/// The subset of `System.*` fields this stage renders.
#[derive(Debug, Deserialize)]
pub(crate) struct Fields {
    #[serde(rename = "System.WorkItemType")]
    work_item_type: String,
    #[serde(rename = "System.State")]
    state: String,
    #[serde(rename = "System.Title")]
    title: String,
    #[serde(rename = "System.AssignedTo")]
    assigned_to: Option<Identity>,
    #[serde(rename = "System.ChangedDate")]
    changed_date: Option<String>,
    #[serde(rename = "System.Tags")]
    tags: Option<String>,
    #[serde(rename = "System.Description")]
    description: Option<String>,
}

/// Identity ref. Modern servers return an object with `displayName`; some
/// older configurations return a bare string. Accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Identity {
    Ref {
        #[serde(rename = "displayName")]
        display_name: String,
    },
    Raw(String),
}

impl Identity {
    fn name(&self) -> &str {
        match self {
            Self::Ref { display_name } => display_name.trim(),
            Self::Raw(s) => s.trim(),
        }
    }
}

/// `api-version` pinned for the work item comments preview endpoint.
const COMMENTS_API_VERSION: &str = "6.0-preview.3";

/// Envelope returned by `GET /_apis/wit/workItems/{id}/comments`.
#[derive(Debug, Deserialize)]
struct CommentsResponse {
    #[serde(default)]
    comments: Vec<Comment>,
}

/// A single work item comment.
#[derive(Debug, Deserialize)]
pub(crate) struct Comment {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    text: String,
    #[serde(rename = "createdBy")]
    created_by: Option<Identity>,
    #[serde(rename = "createdDate")]
    created_date: Option<String>,
}

/// Fetch a single work item by numeric id.
pub(crate) async fn fetch(client: &AzdoClient, id: u64) -> AzdoResult<WorkItem> {
    let url = format!(
        "{}?api-version={}",
        client.collection_url(&format!("/_apis/wit/workitems/{id}")),
        client.api_version(),
    );
    let resp = client.request(Method::GET, url).send().await?;
    let resp = AzdoClient::check_response(resp).await?;
    Ok(resp.json::<WorkItem>().await?)
}

/// Fetch up to `top` comments for a work item, sorted chronologically
/// (oldest first; ties broken by comment id).
pub(crate) async fn fetch_comments(
    client: &AzdoClient,
    id: u64,
    top: u32,
) -> AzdoResult<Vec<Comment>> {
    let url = format!(
        "{}?api-version={COMMENTS_API_VERSION}&$top={top}",
        client.project_url(&format!("/_apis/wit/workItems/{id}/comments")),
    );
    let resp = client.request(Method::GET, url).send().await?;
    let resp = AzdoClient::check_response(resp).await?;
    let mut comments = resp.json::<CommentsResponse>().await?.comments;
    sort_chrono(&mut comments);
    Ok(comments)
}

/// Request body for `POST .../comments`.
#[derive(Debug, Serialize)]
struct CommentCreate {
    text: String,
}

/// Post a new comment and return the server's stored copy.
pub(crate) async fn post_comment(client: &AzdoClient, id: u64, text: &str) -> AzdoResult<Comment> {
    let url = format!(
        "{}?api-version={COMMENTS_API_VERSION}",
        client.project_url(&format!("/_apis/wit/workItems/{id}/comments")),
    );
    let resp = client
        .request(Method::POST, url)
        .json(&CommentCreate {
            text: text.to_owned(),
        })
        .send()
        .await?;
    let resp = AzdoClient::check_response(resp).await?;
    Ok(resp.json::<Comment>().await?)
}

/// JSON-Patch path for the work item state field.
const STATE_PATH: &str = "/fields/System.State";

/// One operation in an `application/json-patch+json` document.
#[derive(Debug, Serialize)]
struct JsonPatchOp {
    op: String,
    path: String,
    value: String,
}

/// Set the work item state and return the server's updated copy.
pub(crate) async fn set_state(client: &AzdoClient, id: u64, state: &str) -> AzdoResult<WorkItem> {
    let url = format!(
        "{}?api-version={}",
        client.collection_url(&format!("/_apis/wit/workitems/{id}")),
        client.api_version(),
    );
    let patch = [JsonPatchOp {
        op: "add".to_owned(),
        path: STATE_PATH.to_owned(),
        value: state.to_owned(),
    }];
    let resp = client
        .request(Method::PATCH, url)
        .header(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json-patch+json"),
        )
        .json(&patch)
        .send()
        .await?;
    let resp = AzdoClient::check_response(resp).await?;
    Ok(resp.json::<WorkItem>().await?)
}

/// Order comments oldest-first; ties broken by comment id. ISO 8601
/// timestamps sort chronologically as plain strings.
fn sort_chrono(comments: &mut [Comment]) {
    comments.sort_by(|a, b| a.created_date.cmp(&b.created_date).then(a.id.cmp(&b.id)));
}

/// Header + meta lines (`#id …`, `Title:`, `Assignee:`, optional
/// `Changed:`/`Tags:`). Shared by the plain renderer and the TUI.
pub(crate) fn summary_lines(item: &WorkItem) -> Vec<String> {
    let f = &item.fields;

    let assignee = f
        .assigned_to
        .as_ref()
        .map(Identity::name)
        .filter(|n| !n.is_empty())
        .unwrap_or("Unassigned");

    let mut lines = vec![
        format!(
            "#{id}  [{ty}]  State: {state}",
            id = item.id,
            ty = f.work_item_type,
            state = f.state,
        ),
        format!("Title:    {}", f.title),
        format!("Assignee: {assignee}"),
    ];

    if let Some(changed) = f.changed_date.as_deref().map(str::trim) {
        if !changed.is_empty() {
            lines.push(format!("Changed:  {changed}"));
        }
    }

    if let Some(tags) = f.tags.as_deref() {
        let tags = normalize_tags(tags);
        if !tags.is_empty() {
            lines.push(format!("Tags:     {tags}"));
        }
    }

    lines
}

/// The work item description, HTML-flattened, or `(none)`.
pub(crate) fn description_text(item: &WorkItem) -> AzdoResult<String> {
    match item.fields.description.as_deref() {
        Some(html) if !html.trim().is_empty() => html_to_text(html),
        _ => Ok("(none)".to_owned()),
    }
}

/// One comment's header line, e.g. `[2026-05-13T09:00:00Z] Ivan Petrov`.
pub(crate) fn comment_header(c: &Comment) -> String {
    let author = c
        .created_by
        .as_ref()
        .map(Identity::name)
        .filter(|n| !n.is_empty())
        .unwrap_or("Unknown");
    match c.created_date.as_deref().map(str::trim) {
        Some(date) if !date.is_empty() => format!("[{date}] {author}"),
        _ => author.to_owned(),
    }
}

/// One comment's body, HTML-flattened, or `(empty)`.
pub(crate) fn comment_body(c: &Comment) -> AzdoResult<String> {
    if c.text.trim().is_empty() {
        Ok("(empty)".to_owned())
    } else {
        html_to_text(&c.text)
    }
}

/// Render a comments section (no trailing newline).
pub(crate) fn render_comments(comments: &[Comment]) -> AzdoResult<String> {
    if comments.is_empty() {
        return Ok("Comments: (none)".to_owned());
    }

    let mut lines = vec![format!("Comments ({}):", comments.len())];
    for c in comments {
        lines.push(String::new());
        lines.push(comment_header(c));
        lines.push(comment_body(c)?);
    }
    Ok(lines.join("\n"))
}

/// Render a work item as a readable plain-text block (no trailing newline).
pub(crate) fn render(item: &WorkItem) -> AzdoResult<String> {
    let mut lines = summary_lines(item);
    lines.push(String::new());
    lines.push("Description:".to_owned());
    lines.push(description_text(item)?);
    Ok(lines.join("\n"))
}

/// Azure DevOps joins tags with `"; "`. Normalise to a comma-separated list.
fn normalize_tags(raw: &str) -> String {
    raw.split(';')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Flatten an HTML fragment to wrapped plain text, trailing blank lines
/// trimmed.
fn html_to_text(html: &str) -> AzdoResult<String> {
    let text = html2text::from_read(html.as_bytes(), DESC_WIDTH)
        .map_err(|e| AzdoError::Render(e.to_string()))?;
    Ok(text.trim_end().to_owned())
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use super::{
        render, render_comments, sort_chrono, CommentCreate, CommentsResponse, JsonPatchOp,
        WorkItem,
    };

    fn parse(raw: &str) -> WorkItem {
        serde_json::from_str(raw).unwrap_or_else(|e| panic!("fixture must parse: {e}"))
    }

    fn parse_comments(raw: &str) -> CommentsResponse {
        serde_json::from_str(raw).unwrap_or_else(|e| panic!("fixture must parse: {e}"))
    }

    #[test]
    fn comment_body_serializes_to_text_field() {
        let json = serde_json::to_string(&CommentCreate {
            text: "line one\n\"quoted\" & <tagged>".to_owned(),
        })
        .unwrap_or_else(|e| panic!("must serialize: {e}"));
        assert_eq!(
            json, r#"{"text":"line one\n\"quoted\" & <tagged>"}"#,
            "POST body must be a single JSON-escaped `text` field",
        );
    }

    #[test]
    fn set_state_serializes_json_patch_document() {
        let patch = [JsonPatchOp {
            op: "add".to_owned(),
            path: "/fields/System.State".to_owned(),
            value: "Ready for Test".to_owned(),
        }];
        let json = serde_json::to_string(&patch).unwrap_or_else(|e| panic!("must serialize: {e}"));
        assert_eq!(
            json, r#"[{"op":"add","path":"/fields/System.State","value":"Ready for Test"}]"#,
            "body must be a single-op application/json-patch+json array",
        );
    }

    const FULL: &str = r#"
{
  "id": 12345,
  "fields": {
    "System.WorkItemType": "Bug",
    "System.State": "Active",
    "System.Title": "Customs declaration form rejects valid TIN",
    "System.AssignedTo": {
      "displayName": "Ivan Petrov",
      "uniqueName": "DOMAIN\\ipetrov"
    },
    "System.ChangedDate": "2026-05-14T10:23:00Z",
    "System.Tags": "customs; urgent",
    "System.Description": "<div>The form rejects a valid TIN.</div><div>Steps: open the form, enter a TIN.</div>"
  }
}
"#;

    #[test]
    fn renders_full_work_item() {
        let out = render(&parse(FULL)).expect("render must succeed");
        let expected = "#12345  [Bug]  State: Active\n\
             Title:    Customs declaration form rejects valid TIN\n\
             Assignee: Ivan Petrov\n\
             Changed:  2026-05-14T10:23:00Z\n\
             Tags:     customs, urgent\n\
             \n\
             Description:\n\
             The form rejects a valid TIN.\n\
             Steps: open the form, enter a TIN.";
        assert_eq!(out, expected, "rendered block drifted from snapshot");
    }

    #[test]
    fn renders_minimal_work_item_without_optionals() {
        let raw = r#"
{
  "id": 7,
  "fields": {
    "System.WorkItemType": "Task",
    "System.State": "New",
    "System.Title": "Wire up the thing"
  }
}
"#;
        let out = render(&parse(raw)).expect("render must succeed");
        let expected = "#7  [Task]  State: New\n\
             Title:    Wire up the thing\n\
             Assignee: Unassigned\n\
             \n\
             Description:\n\
             (none)";
        assert_eq!(out, expected, "minimal item must omit optional lines");
    }

    #[test]
    fn assigned_to_accepts_bare_string() {
        let raw = r#"
{
  "id": 9,
  "fields": {
    "System.WorkItemType": "Task",
    "System.State": "New",
    "System.Title": "Legacy identity shape",
    "System.AssignedTo": "Maria Ivanova"
  }
}
"#;
        let out = render(&parse(raw)).expect("render must succeed");
        assert!(
            out.contains("Assignee: Maria Ivanova"),
            "bare-string identity not handled: {out}",
        );
    }

    // Deliberately out of chronological order to also exercise the sort.
    const COMMENTS: &str = r#"
{
  "count": 2,
  "comments": [
    {
      "id": 11,
      "text": "<div>Second, posted later.</div>",
      "createdBy": { "displayName": "Maria Ivanova" },
      "createdDate": "2026-05-14T11:30:00Z"
    },
    {
      "id": 10,
      "text": "<div>First, posted earlier.</div>",
      "createdBy": { "displayName": "Ivan Petrov" },
      "createdDate": "2026-05-13T09:00:00Z"
    }
  ]
}
"#;

    #[test]
    fn renders_comments_sorted_chronologically() {
        let mut list = parse_comments(COMMENTS).comments;
        sort_chrono(&mut list);
        let out = render_comments(&list).expect("render must succeed");
        let expected = "Comments (2):\n\
             \n\
             [2026-05-13T09:00:00Z] Ivan Petrov\n\
             First, posted earlier.\n\
             \n\
             [2026-05-14T11:30:00Z] Maria Ivanova\n\
             Second, posted later.";
        assert_eq!(out, expected, "comments section drifted from snapshot");
    }

    #[test]
    fn empty_comments_render_none() {
        let out = render_comments(&[]).expect("render must succeed");
        assert_eq!(out, "Comments: (none)");
    }

    #[test]
    fn comment_author_falls_back_when_missing() {
        let raw = r#"
{
  "comments": [
    { "id": 1, "text": "<div>orphan</div>", "createdDate": "2026-05-15T08:00:00Z" }
  ]
}
"#;
        let out = render_comments(&parse_comments(raw).comments).expect("render must succeed");
        assert!(
            out.contains("[2026-05-15T08:00:00Z] Unknown"),
            "missing author not handled: {out}",
        );
    }
}
