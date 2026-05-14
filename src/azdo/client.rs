use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use reqwest::{Method, RequestBuilder};

use crate::config::{Config, Pat};
use crate::error::{AzdoError, AzdoResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct AzdoClient {
    http: reqwest::Client,
    server: String,
    collection: String,
    project: String,
    api_version: String,
}

impl AzdoClient {
    pub fn new(cfg: &Config, pat: &Pat) -> AzdoResult<Self> {
        let token = B64.encode(format!(":{}", pat.as_str()));
        let auth_value = format!("Basic {token}");

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).map_err(|e| AzdoError::Header(e.to_string()))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("azdo/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            http,
            server: trim_slash(&cfg.server).to_string(),
            collection: trim_slash(&cfg.collection).to_string(),
            project: trim_slash(&cfg.project).to_string(),
            api_version: cfg.api_version.clone(),
        })
    }

    pub fn collection_url(&self, path: &str) -> String {
        format!(
            "{}/{}{}",
            self.server,
            self.collection,
            ensure_leading_slash(path)
        )
    }

    #[allow(dead_code)]
    pub fn project_url(&self, path: &str) -> String {
        format!(
            "{}/{}/{}{}",
            self.server,
            self.collection,
            self.project,
            ensure_leading_slash(path)
        )
    }

    pub fn project_name(&self) -> &str {
        &self.project
    }

    #[allow(dead_code)]
    pub fn api_version(&self) -> &str {
        &self.api_version
    }

    pub fn request(&self, method: Method, url: String) -> RequestBuilder {
        self.http.request(method, url)
    }

    pub async fn check_response(resp: reqwest::Response) -> AzdoResult<reqwest::Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let url = resp.url().to_string();
        let body = resp.text().await.unwrap_or_default();
        let truncated = body.chars().take(500).collect::<String>();
        Err(AzdoError::Http {
            status,
            url,
            body: truncated,
        })
    }

    pub async fn ping(&self) -> AzdoResult<u16> {
        let url = format!(
            "{}/_apis/projects/{}?api-version={}",
            self.collection_url(""),
            self.project,
            self.api_version
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
        s.to_string()
    } else {
        format!("/{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::from_str_for_tests(
            r#"
server = "https://azdo.company.local/tfs/"
collection = "DefaultCollection"
project = "Customs"
"#,
        )
        .expect("must parse")
    }

    #[test]
    fn builds_collection_url() {
        let c = AzdoClient::new(&cfg(), &Pat::new("x".into())).unwrap();
        assert_eq!(
            c.collection_url("/_apis/projects"),
            "https://azdo.company.local/tfs/DefaultCollection/_apis/projects"
        );
    }

    #[test]
    fn builds_project_url() {
        let c = AzdoClient::new(&cfg(), &Pat::new("x".into())).unwrap();
        assert_eq!(
            c.project_url("/_apis/wit/workitems/12345"),
            "https://azdo.company.local/tfs/DefaultCollection/Customs/_apis/wit/workitems/12345"
        );
    }

    #[test]
    fn url_normalizes_missing_leading_slash() {
        let c = AzdoClient::new(&cfg(), &Pat::new("x".into())).unwrap();
        assert_eq!(
            c.project_url("_apis/wit"),
            "https://azdo.company.local/tfs/DefaultCollection/Customs/_apis/wit"
        );
    }
}
