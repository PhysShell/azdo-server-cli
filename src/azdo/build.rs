//! `azdo build start`: queue a build definition and optionally wait for it.

use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use super::AzdoClient;
use crate::error::AzdoResult;

/// Poll interval while `--wait` blocks on a running build.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Request body for `POST /_apis/build/builds`. `parameters` is itself a
/// JSON-encoded string (Azure DevOps' own contract), so it is built by the
/// caller and passed through verbatim.
#[derive(Debug, Serialize)]
struct BuildQueue {
    definition: DefinitionRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<String>,
}

#[derive(Debug, Serialize)]
struct DefinitionRef {
    id: u32,
}

/// A build as returned by the queue and get endpoints (the fields we use).
#[derive(Debug, Deserialize)]
pub(crate) struct Build {
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) result: Option<String>,
    #[serde(rename = "buildNumber", default)]
    pub(crate) number: Option<String>,
}

impl Build {
    /// A build is terminal once the server reports `status == "completed"`.
    pub(crate) fn is_complete(&self) -> bool {
        self.status.as_deref() == Some("completed")
    }
}

fn builds_url(client: &AzdoClient, suffix: &str) -> String {
    format!(
        "{}?api-version={}",
        client.project_url(&format!("/_apis/build/builds{suffix}")),
        client.api_version(),
    )
}

/// Queue a build for `definition_id`, passing `parameters` (a JSON-encoded
/// string) through unchanged when present.
pub(crate) async fn queue_build(
    client: &AzdoClient,
    definition_id: u32,
    parameters: Option<String>,
) -> AzdoResult<Build> {
    let resp = client
        .request(Method::POST, builds_url(client, ""))
        .json(&BuildQueue {
            definition: DefinitionRef { id: definition_id },
            parameters,
        })
        .send()
        .await?;
    let resp = AzdoClient::check_response(resp).await?;
    Ok(resp.json::<Build>().await?)
}

/// Fetch a single build's current state.
pub(crate) async fn get_build(client: &AzdoClient, id: u64) -> AzdoResult<Build> {
    let resp = client
        .request(Method::GET, builds_url(client, &format!("/{id}")))
        .send()
        .await?;
    let resp = AzdoClient::check_response(resp).await?;
    Ok(resp.json::<Build>().await?)
}

/// Poll `id` until the server reports it complete, then return the final
/// build. Polling cadence is fixed; the caller (or Ctrl-C) bounds the wait.
pub(crate) async fn wait_build(client: &AzdoClient, id: u64) -> AzdoResult<Build> {
    loop {
        let build = get_build(client, id).await?;
        if build.is_complete() {
            return Ok(build);
        }
        sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use super::{Build, BuildQueue, DefinitionRef};

    fn parse(raw: &str) -> Build {
        serde_json::from_str(raw).unwrap_or_else(|e| panic!("fixture must parse: {e}"))
    }

    #[test]
    fn queue_body_serializes_with_passthrough_parameters() {
        let json = serde_json::to_string(&BuildQueue {
            definition: DefinitionRef { id: 42 },
            parameters: Some(r#"{"WorkItemId":"12345"}"#.to_owned()),
        })
        .unwrap_or_else(|e| panic!("must serialize: {e}"));
        assert_eq!(
            json, r#"{"definition":{"id":42},"parameters":"{\"WorkItemId\":\"12345\"}"}"#,
            "body must carry the definition id and the parameters string verbatim",
        );
    }

    #[test]
    fn queue_body_omits_parameters_when_absent() {
        let json = serde_json::to_string(&BuildQueue {
            definition: DefinitionRef { id: 7 },
            parameters: None,
        })
        .unwrap_or_else(|e| panic!("must serialize: {e}"));
        assert_eq!(
            json, r#"{"definition":{"id":7}}"#,
            "an absent parameters field must not be sent",
        );
    }

    #[test]
    fn completion_is_driven_by_status_field() {
        let running = parse(r#"{"id":1,"status":"inProgress"}"#);
        assert!(!running.is_complete(), "inProgress is not terminal");

        let done = parse(
            r#"{"id":1,"status":"completed","result":"succeeded","buildNumber":"20260517.1"}"#,
        );
        assert!(done.is_complete(), "completed is terminal");
        assert_eq!(done.result.as_deref(), Some("succeeded"));
        assert_eq!(done.number.as_deref(), Some("20260517.1"));

        let no_status = parse(r#"{"id":1}"#);
        assert!(
            !no_status.is_complete(),
            "a missing status must not read as complete",
        );
    }
}
