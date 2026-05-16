use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use reqwest::{Client, Method, RequestBuilder, Response};

use crate::config::{Config, Pat};
use crate::error::{AzdoError, AzdoResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_BODY_LIMIT: usize = 500;

#[derive(Debug, Clone)]
pub(crate) struct AzdoClient {
    http: Client,
    server: String,
    collection: String,
    project: String,
    api_version: String,
}

impl AzdoClient {
    pub(crate) fn new(cfg: &Config, pat: &Pat) -> AzdoResult<Self> {
        let token = B64.encode(format!(":{}", pat.as_str()));
        let auth_value = format!("Basic {token}");

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).map_err(|e| AzdoError::Header(e.to_string()))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let http = Client::builder()
            .default_headers(headers)
            .user_agent(concat!("azdo/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            http,
            server: trim_slash(&cfg.server).to_owned(),
            collection: trim_slash(&cfg.collection).to_owned(),
            project: trim_slash(&cfg.project).to_owned(),
            api_version: cfg.api_version.clone(),
        })
    }

    pub(crate) fn collection_url(&self, path: &str) -> String {
        format!(
            "{}/{}{}",
            self.server,
            self.collection,
            ensure_leading_slash(path),
        )
    }

    pub(crate) fn project_url(&self, path: &str) -> String {
        format!(
            "{}/{}/{}{}",
            self.server,
            self.collection,
            self.project,
            ensure_leading_slash(path),
        )
    }

    pub(crate) fn project_name(&self) -> &str {
        &self.project
    }

    /// Browser URL for a work item's edit page.
    pub(crate) fn web_item_url(&self, id: u64) -> String {
        format!(
            "{}/{}/{}/_workitems/edit/{id}",
            self.server, self.collection, self.project,
        )
    }

    pub(crate) fn api_version(&self) -> &str {
        &self.api_version
    }

    pub(crate) fn request(&self, method: Method, url: String) -> RequestBuilder {
        self.http.request(method, url)
    }

    pub(crate) async fn check_response(resp: Response) -> AzdoResult<Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let url = resp.url().to_string();
        let body = resp.text().await.unwrap_or_default();
        let truncated: String = body.chars().take(ERROR_BODY_LIMIT).collect();
        Err(AzdoError::Http {
            status,
            url,
            body: truncated,
        })
    }

    pub(crate) async fn ping(&self) -> AzdoResult<u16> {
        let url = format!(
            "{}/_apis/projects/{}?api-version={}",
            self.collection_url(""),
            self.project,
            self.api_version,
        );
        let resp = self.request(Method::GET, url).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.status().as_u16())
    }
}

fn trim_slash(s: &str) -> &str {
    s.trim_end_matches('/')
}

fn ensure_leading_slash(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else if s.starts_with('/') {
        s.to_owned()
    } else {
        format!("/{s}")
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::absolute_paths,
    clippy::arithmetic_side_effects,
    reason = "tests legitimately panic on bad fixtures; proptest macro emits absolute paths"
)]
mod tests {
    use proptest::prelude::*;

    use super::{ensure_leading_slash, trim_slash, AzdoClient, Config, Pat};

    fn cfg() -> Config {
        Config::from_str_for_tests(
            r#"
server = "https://azdo.company.local/tfs/"
collection = "DefaultCollection"
project = "Customs"
"#,
        )
        .unwrap_or_else(|e| panic!("must parse: {e}"))
    }

    fn client() -> AzdoClient {
        AzdoClient::new(&cfg(), &Pat::new("x".into()))
            .unwrap_or_else(|e| panic!("must build client: {e}"))
    }

    #[test]
    fn builds_collection_url() {
        assert_eq!(
            client().collection_url("/_apis/projects"),
            "https://azdo.company.local/tfs/DefaultCollection/_apis/projects",
        );
    }

    #[test]
    fn builds_project_url() {
        assert_eq!(
            client().project_url("/_apis/wit/workitems/12345"),
            "https://azdo.company.local/tfs/DefaultCollection/Customs/_apis/wit/workitems/12345",
        );
    }

    #[test]
    fn url_normalizes_missing_leading_slash() {
        assert_eq!(
            client().project_url("_apis/wit"),
            "https://azdo.company.local/tfs/DefaultCollection/Customs/_apis/wit",
        );
    }

    proptest! {
        /// `trim_slash` only ever strips a run of trailing `/`, nothing else.
        #[test]
        fn trim_slash_props(s in ".*") {
            let out = trim_slash(&s);

            // 1. The result never ends with a slash.
            prop_assert!(!out.ends_with('/'), "still ends with '/': {:?}", out);

            // 2. It is a prefix of the input (only the tail was touched).
            prop_assert!(s.starts_with(out), "{:?} is not a prefix of {:?}", out, s);

            // 3. Everything that was removed was a slash.
            let removed = s.strip_prefix(out).expect("out is a prefix of s");
            prop_assert!(
                removed.chars().all(|c| c == '/'),
                "removed non-slash content: {:?}",
                removed,
            );

            // 4. Idempotent.
            prop_assert_eq!(trim_slash(out), out);
        }

        /// `ensure_leading_slash` guarantees exactly one leading slash for
        /// non-empty input and is content-preserving.
        #[test]
        fn ensure_leading_slash_props(s in ".*") {
            let out = ensure_leading_slash(&s);

            if s.is_empty() {
                prop_assert!(out.is_empty(), "empty input must stay empty");
            } else {
                prop_assert!(
                    out.starts_with('/'),
                    "missing leading slash: {:?}",
                    out,
                );
                // Tail is preserved verbatim.
                prop_assert!(
                    out.ends_with(&s),
                    "{:?} does not end with {:?}",
                    out,
                    s,
                );
                // At most one character (the slash) is ever added.
                prop_assert!(
                    out.chars().count() <= s.chars().count() + 1,
                    "added more than one char: {:?} -> {:?}",
                    s,
                    out,
                );
            }

            // Idempotent: a value that already has a leading slash is unchanged.
            prop_assert_eq!(ensure_leading_slash(&out), out);
        }
    }
}
