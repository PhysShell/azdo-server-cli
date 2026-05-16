//! `azdo my`: the caller's open work items via WIQL + `workitemsbatch`.

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::workitem::WorkItem;
use super::AzdoClient;
use crate::error::AzdoResult;

/// "Open" == assigned to the caller and not in a terminal state. `@Me`
/// is resolved server-side from the PAT's identity.
const WIQL: &str = "SELECT [System.Id] FROM WorkItems \
WHERE [System.AssignedTo] = @Me \
AND [System.State] NOT IN ('Closed', 'Done', 'Removed') \
ORDER BY [System.ChangedDate] DESC";

/// `workitemsbatch` rejects requests with more than 200 ids.
const BATCH_MAX: usize = 200;

/// Fields requested for the table; matches what `WorkItem` renders.
const BATCH_FIELDS: [&str; 4] = [
    "System.Id",
    "System.WorkItemType",
    "System.State",
    "System.Title",
];

#[derive(Debug, Serialize)]
struct WiqlQuery {
    query: String,
}

#[derive(Debug, Deserialize)]
struct WiqlResponse {
    #[serde(rename = "workItems", default)]
    work_items: Vec<WiqlRef>,
}

#[derive(Debug, Deserialize)]
struct WiqlRef {
    id: u64,
}

#[derive(Debug, Serialize)]
struct BatchRequest {
    ids: Vec<u64>,
    fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BatchResponse {
    #[serde(default)]
    value: Vec<WorkItem>,
}

/// Fetch the caller's open work items, newest change first.
pub(crate) async fn my_open_items(client: &AzdoClient) -> AzdoResult<Vec<WorkItem>> {
    let wiql_url = format!(
        "{}?api-version={}",
        client.project_url("/_apis/wit/wiql"),
        client.api_version(),
    );
    let ids: Vec<u64> = {
        let resp = client
            .request(Method::POST, wiql_url)
            .json(&WiqlQuery {
                query: WIQL.to_owned(),
            })
            .send()
            .await?;
        let resp = AzdoClient::check_response(resp).await?;
        resp.json::<WiqlResponse>()
            .await?
            .work_items
            .into_iter()
            .map(|w| w.id)
            .collect()
    };

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let batch_url = format!(
        "{}?api-version={}",
        client.collection_url("/_apis/wit/workitemsbatch"),
        client.api_version(),
    );
    let mut items = Vec::with_capacity(ids.len());
    // `workitemsbatch` caps at 200 ids and returns them in request
    // order, so chunking preserves the WIQL ordering.
    for chunk in ids.chunks(BATCH_MAX) {
        let resp = client
            .request(Method::POST, batch_url.clone())
            .json(&BatchRequest {
                ids: chunk.to_vec(),
                fields: BATCH_FIELDS.iter().copied().map(str::to_owned).collect(),
            })
            .send()
            .await?;
        let resp = AzdoClient::check_response(resp).await?;
        items.extend(resp.json::<BatchResponse>().await?.value);
    }
    Ok(items)
}

/// Column widths (id, type, state) sized to the header and the data.
fn widths(items: &[WorkItem]) -> (usize, usize, usize) {
    let id = items
        .iter()
        .fold("ID".len(), |w, it| w.max(it.id.to_string().len()));
    let ty = items
        .iter()
        .fold("Type".len(), |w, it| w.max(it.work_item_type().len()));
    let st = items
        .iter()
        .fold("State".len(), |w, it| w.max(it.state().len()));
    (id, ty, st)
}

/// Aligned data rows (no header); empty when there are no items.
pub(crate) fn rows(items: &[WorkItem]) -> Vec<String> {
    let (id_w, ty_w, st_w) = widths(items);
    items
        .iter()
        .map(|it| {
            format!(
                "{:<id_w$}  {:<ty_w$}  {:<st_w$}  {}",
                it.id,
                it.work_item_type(),
                it.state(),
                it.title(),
            )
        })
        .collect()
}

/// Render the items as an aligned plain-text table (header + rows), or a
/// friendly message when the list is empty.
pub(crate) fn render_table(items: &[WorkItem]) -> String {
    if items.is_empty() {
        return "No open work items.".to_owned();
    }
    let (id_w, ty_w, st_w) = widths(items);
    let mut out = format!(
        "{:<id_w$}  {:<ty_w$}  {:<st_w$}  {}",
        "ID", "Type", "State", "Title"
    );
    for row in rows(items) {
        out.push('\n');
        out.push_str(&row);
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use super::{render_table, rows, BatchRequest, WiqlQuery};
    use crate::azdo::workitem::WorkItem;

    const BATCH: &str = r#"
{
  "value": [
    {
      "id": 7,
      "fields": {
        "System.WorkItemType": "Bug",
        "System.State": "Active",
        "System.Title": "Short id, long type alignment check"
      }
    },
    {
      "id": 123456,
      "fields": {
        "System.WorkItemType": "Task",
        "System.State": "New",
        "System.Title": "Wide id row"
      }
    }
  ]
}
"#;

    fn fixture() -> Vec<WorkItem> {
        let root: serde_json::Value =
            serde_json::from_str(BATCH).unwrap_or_else(|e| panic!("fixture: {e}"));
        let value = root
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value).unwrap_or_else(|e| panic!("fixture: {e}"))
    }

    #[test]
    fn wiql_and_batch_bodies_serialize() {
        let wiql = serde_json::to_string(&WiqlQuery {
            query: "SELECT [System.Id] FROM WorkItems".to_owned(),
        })
        .unwrap_or_else(|e| panic!("serialize: {e}"));
        assert_eq!(
            wiql, r#"{"query":"SELECT [System.Id] FROM WorkItems"}"#,
            "WIQL body must be a single `query` string field",
        );

        let batch = serde_json::to_string(&BatchRequest {
            ids: vec![1, 2],
            fields: vec!["System.Id".to_owned(), "System.Title".to_owned()],
        })
        .unwrap_or_else(|e| panic!("serialize: {e}"));
        assert_eq!(
            batch, r#"{"ids":[1,2],"fields":["System.Id","System.Title"]}"#,
            "batch body must carry ids then fields",
        );
    }

    #[test]
    fn empty_list_has_friendly_message() {
        assert_eq!(
            render_table(&[]),
            "No open work items.",
            "empty result must not print an empty table",
        );
    }

    #[test]
    fn table_is_column_aligned() {
        let items = fixture();
        let table = render_table(&items);
        let mut lines = table.lines();

        let header = lines.next().unwrap_or_else(|| panic!("missing header"));
        assert!(
            header.starts_with("ID    "),
            "id column must be padded to the widest id (123456): {header:?}",
        );
        assert!(
            header.contains("Type") && header.contains("State") && header.contains("Title"),
            "header must name every column: {header:?}",
        );

        let data = rows(&items);
        assert_eq!(data.len(), 2, "one row per work item");
        assert!(
            data.iter()
                .all(|r| r.starts_with("7     ") || r.starts_with("123456")),
            "ids must be left-aligned in a fixed-width column: {data:?}",
        );
        let first = data.first().unwrap_or_else(|| panic!("missing row 0"));
        assert!(
            first.contains("Bug") && first.ends_with("alignment check"),
            "row keeps type and full title: {first:?}",
        );
    }
}
