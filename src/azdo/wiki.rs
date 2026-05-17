//! `azdo` wiki REST: read a page, list a subtree, create/update a page.
//!
//! Mirrors the `build` module's shape (URL builder + thin async calls over
//! `AzdoClient`). The daily workflow needs only these three verbs; the
//! "which page is latest" / "don't overwrite" policy lives in the script
//! and in [`crate::daily`], not here.

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::AzdoClient;
use crate::error::AzdoResult;

/// A wiki page node from `Pages - Get`. Only the fields the daily workflow
/// needs; `sub_pages` is populated when fetched with `recursionLevel=full`.
#[derive(Debug, Deserialize)]
pub(crate) struct WikiPage {
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(rename = "subPages", default)]
    pub(crate) sub_pages: Vec<Self>,
}

/// Request body for `Pages - Create Or Update`.
#[derive(Debug, Serialize)]
struct PageContent<'a> {
    content: &'a str,
}

fn pages_url(client: &AzdoClient, wiki_id: &str) -> String {
    client.project_url(&format!("/_apis/wiki/wikis/{wiki_id}/pages"))
}

/// Read a single page's content; an absent body is normalised to empty.
pub(crate) async fn get_page(client: &AzdoClient, wiki_id: &str, path: &str) -> AzdoResult<String> {
    let resp = client
        .request(Method::GET, pages_url(client, wiki_id))
        .query(&[
            ("path", path),
            ("includeContent", "true"),
            ("api-version", client.api_version()),
        ])
        .send()
        .await?;
    let resp = AzdoClient::check_response(resp).await?;
    let page = resp.json::<WikiPage>().await?;
    Ok(page.content.unwrap_or_default())
}

/// Every descendant page path under `root` (the root included), flattened
/// from one `recursionLevel=full` fetch in the server's pre-order. Callers
/// that want "the latest" sort by parsed date themselves
/// ([`crate::daily::pick_latest`]) — order is not relied on here.
pub(crate) async fn list_pages(
    client: &AzdoClient,
    wiki_id: &str,
    root: &str,
) -> AzdoResult<Vec<String>> {
    let resp = client
        .request(Method::GET, pages_url(client, wiki_id))
        .query(&[
            ("path", root),
            ("recursionLevel", "full"),
            ("api-version", client.api_version()),
        ])
        .send()
        .await?;
    let resp = AzdoClient::check_response(resp).await?;
    let tree = resp.json::<WikiPage>().await?;
    let mut out = Vec::new();
    flatten(&tree, &mut out);
    Ok(out)
}

fn flatten(node: &WikiPage, out: &mut Vec<String>) {
    if !node.path.is_empty() {
        out.push(node.path.clone());
    }
    for child in &node.sub_pages {
        flatten(child, out);
    }
}

/// Create or update the page at `path`. The daily workflow only ever
/// creates (it guards against overwrite via the listing), so no `If-Match`
/// is sent; Azure DevOps auto-creates any missing parent pages.
pub(crate) async fn put_page(
    client: &AzdoClient,
    wiki_id: &str,
    path: &str,
    content: &str,
) -> AzdoResult<()> {
    let resp = client
        .request(Method::PUT, pages_url(client, wiki_id))
        .query(&[("path", path), ("api-version", client.api_version())])
        .json(&PageContent { content })
        .send()
        .await?;
    drop(AzdoClient::check_response(resp).await?);
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests legitimately panic on bad fixtures"
)]
mod tests {
    use super::{flatten, PageContent, WikiPage};

    fn parse(raw: &str) -> WikiPage {
        serde_json::from_str(raw).unwrap_or_else(|e| panic!("fixture must parse: {e}"))
    }

    #[test]
    fn put_body_is_just_the_content_field() {
        let json = serde_json::to_string(&PageContent { content: "hello" })
            .unwrap_or_else(|e| panic!("must serialize: {e}"));
        assert_eq!(
            json, r#"{"content":"hello"}"#,
            "the create/update body must be exactly {{\"content\": ...}}",
        );
    }

    #[test]
    fn flatten_is_preorder_and_skips_empty_paths() {
        // A realistic daily subtree: root -> year -> quarter -> pages,
        // plus a page node with an empty path (must be skipped).
        let tree = parse(
            r#"{
                "path": "/Daily",
                "subPages": [
                    { "path": "",
                      "subPages": [ { "path": "/Daily/2026" } ] },
                    { "path": "/Daily/2026/Q1",
                      "subPages": [
                        { "path": "/Daily/2026/Q1/30.01.2026" },
                        { "path": "/Daily/2026/Q1/31.12.2025" }
                      ] }
                ]
            }"#,
        );
        let mut out = Vec::new();
        flatten(&tree, &mut out);
        assert_eq!(
            out,
            vec![
                "/Daily".to_owned(),
                "/Daily/2026".to_owned(),
                "/Daily/2026/Q1".to_owned(),
                "/Daily/2026/Q1/30.01.2026".to_owned(),
                "/Daily/2026/Q1/31.12.2025".to_owned(),
            ],
            "flatten must be pre-order and drop empty-path nodes",
        );
    }

    #[test]
    fn missing_optional_fields_default_cleanly() {
        let leaf = parse(r#"{ "path": "/Daily/2026/Q1/01.01.2026" }"#);
        assert!(
            leaf.content.is_none() && leaf.sub_pages.is_empty(),
            "absent content/subPages must deserialize as None/empty",
        );
    }
}
