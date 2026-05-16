use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AzdoError, AzdoResult};

#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    pub(crate) server: String,
    pub(crate) collection: String,
    pub(crate) project: String,
    #[serde(default = "default_api_version")]
    pub(crate) api_version: String,
    #[serde(default)]
    pub(crate) auth: AuthConfig,
    #[allow(dead_code, reason = "wired up by later commands")]
    #[serde(default)]
    pub(crate) wiki: Option<WikiConfig>,
    #[serde(default)]
    pub(crate) states: BTreeMap<String, String>,
    #[allow(dead_code, reason = "wired up by later commands")]
    #[serde(default)]
    pub(crate) products: BTreeMap<String, ProductConfig>,
    #[allow(dead_code, reason = "wired up by later commands")]
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthConfig {
    #[serde(default = "default_pat_env")]
    pub(crate) pat_env: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            pat_env: default_pat_env(),
        }
    }
}

#[allow(dead_code, reason = "wired up by later commands")]
#[derive(Debug, Deserialize)]
pub(crate) struct WikiConfig {
    pub(crate) id: String,
    pub(crate) daily_path: String,
}

#[allow(dead_code, reason = "wired up by later commands")]
#[derive(Debug, Deserialize)]
pub(crate) struct ProductConfig {
    pub(crate) build_definition_id: Option<u32>,
    pub(crate) test_state: Option<String>,
    pub(crate) test_comment_template: Option<String>,
}

#[allow(dead_code, reason = "wired up by later commands")]
#[derive(Debug, Deserialize)]
pub(crate) struct ProfileConfig {
    #[serde(default)]
    pub(crate) users: Vec<String>,
}

fn default_api_version() -> String {
    "6.0".to_owned()
}

fn default_pat_env() -> String {
    "AZDO_PAT".to_owned()
}

pub(crate) struct Pat(String);

impl Pat {
    pub(crate) const fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Pat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pat(***redacted, len={})", self.0.len())
    }
}

impl Config {
    pub(crate) fn load(path: Option<&Path>) -> AzdoResult<Self> {
        let resolved = match path {
            Some(p) => p.to_path_buf(),
            None => default_config_path()
                .ok_or_else(|| AzdoError::Config("cannot resolve config directory".into()))?,
        };
        if !resolved.exists() {
            return Err(AzdoError::Config(format!(
                "config file not found: {}",
                resolved.display()
            )));
        }
        let raw = fs::read_to_string(&resolved)?;
        let cfg: Self = toml::from_str(&raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    #[cfg(test)]
    pub(crate) fn from_str_for_tests(raw: &str) -> AzdoResult<Self> {
        let cfg: Self = toml::from_str(raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub(crate) fn validate(&self) -> AzdoResult<()> {
        if self.server.trim().is_empty() {
            return Err(AzdoError::Config("`server` is empty".into()));
        }
        if self.collection.trim().is_empty() {
            return Err(AzdoError::Config("`collection` is empty".into()));
        }
        if self.project.trim().is_empty() {
            return Err(AzdoError::Config("`project` is empty".into()));
        }
        if !self.server.starts_with("http://") && !self.server.starts_with("https://") {
            return Err(AzdoError::Config(format!(
                "`server` must start with http:// or https:// (got: {})",
                self.server
            )));
        }
        if self.auth.pat_env.trim().is_empty() {
            return Err(AzdoError::Config("`auth.pat_env` is empty".into()));
        }
        Ok(())
    }

    pub(crate) fn read_pat(&self) -> AzdoResult<Pat> {
        env::var(&self.auth.pat_env).map_or_else(
            |_| Err(AzdoError::PatMissing(self.auth.pat_env.clone())),
            |v| {
                if v.is_empty() {
                    Err(AzdoError::PatMissing(self.auth.pat_env.clone()))
                } else {
                    Ok(Pat::new(v))
                }
            },
        )
    }
}

/// Resolve default config path.
///
/// - Windows: `%APPDATA%\azdo\config.toml`
/// - Linux: `$XDG_CONFIG_HOME/azdo/config.toml`, fallback `$HOME/.config/azdo/config.toml`
pub(crate) fn default_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("APPDATA").map(|s| PathBuf::from(s).join("azdo").join("config.toml"))
    }
    #[cfg(not(windows))]
    {
        env::var_os("XDG_CONFIG_HOME").map_or_else(
            || {
                env::var_os("HOME").map(|h| {
                    PathBuf::from(h)
                        .join(".config")
                        .join("azdo")
                        .join("config.toml")
                })
            },
            |xdg| Some(PathBuf::from(xdg).join("azdo").join("config.toml")),
        )
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::absolute_paths,
    clippy::arithmetic_side_effects,
    reason = "tests legitimately panic on bad fixtures; proptest macro emits absolute paths"
)]
mod tests {
    use proptest::prelude::*;

    use super::{AuthConfig, AzdoError, Config, Pat};

    const SAMPLE: &str = r#"
server = "https://azdo.company.local/tfs"
collection = "DefaultCollection"
project = "Customs"
api_version = "6.0"

[auth]
pat_env = "AZDO_PAT"

[wiki]
id = "TeamWiki"
daily_path = "/Meetings/Daily"

[states]
test = "Ready for Test"
active = "Active"

[products.declaration]
build_definition_id = 42
test_state = "Ready for Test"

[profiles.support]
users = ["Ivan Petrov", "Maria Ivanova"]
"#;

    fn parse(raw: &str) -> Config {
        Config::from_str_for_tests(raw).unwrap_or_else(|e| panic!("must parse: {e}"))
    }

    #[test]
    fn parses_full_config() {
        let cfg = parse(SAMPLE);
        assert_eq!(cfg.server, "https://azdo.company.local/tfs");
        assert_eq!(cfg.project, "Customs");
        assert_eq!(cfg.api_version, "6.0");
        assert_eq!(cfg.auth.pat_env, "AZDO_PAT");
        assert_eq!(
            cfg.states.get("test").map(String::as_str),
            Some("Ready for Test"),
        );
        assert_eq!(
            cfg.products
                .get("declaration")
                .and_then(|p| p.build_definition_id),
            Some(42),
        );
        assert_eq!(
            cfg.profiles.get("support").map(|p| p.users.as_slice()),
            Some(&["Ivan Petrov".to_owned(), "Maria Ivanova".to_owned()][..]),
        );
    }

    #[test]
    fn defaults_api_version_and_pat_env() {
        let raw = r#"
server = "https://x/tfs"
collection = "Col"
project = "P"
"#;
        let cfg = parse(raw);
        assert_eq!(cfg.api_version, "6.0");
        assert_eq!(cfg.auth.pat_env, "AZDO_PAT");
    }

    #[test]
    fn rejects_non_http_server() {
        let raw = r#"
server = "azdo.company.local"
collection = "Col"
project = "P"
"#;
        let err = Config::from_str_for_tests(raw)
            .err()
            .unwrap_or_else(|| panic!("must reject"));
        assert!(matches!(err, AzdoError::Config(_)), "got {err:?}");
    }

    #[test]
    fn rejects_empty_required() {
        let raw = r#"
server = ""
collection = "Col"
project = "P"
"#;
        let err = Config::from_str_for_tests(raw)
            .err()
            .unwrap_or_else(|| panic!("must reject"));
        assert!(matches!(err, AzdoError::Config(_)), "got {err:?}");
    }

    #[test]
    fn pat_debug_does_not_leak_value() {
        let pat = Pat::new("super-secret-token-do-not-print".to_owned());
        let dbg = format!("{pat:?}");
        assert!(
            !dbg.contains("super-secret-token"),
            "debug output leaked secret: {dbg}",
        );
        assert!(
            dbg.contains("redacted"),
            "debug missing redaction marker: {dbg}"
        );
    }

    fn cfg_with_server(server: String) -> Config {
        Config {
            server,
            collection: "Col".to_owned(),
            project: "Proj".to_owned(),
            api_version: "6.0".to_owned(),
            auth: AuthConfig {
                pat_env: "AZDO_PAT".to_owned(),
            },
            wiki: None,
            states: super::BTreeMap::new(),
            products: super::BTreeMap::new(),
            profiles: super::BTreeMap::new(),
        }
    }

    proptest! {
        /// With every other field valid, `validate()` accepts the config
        /// **iff** `server` carries an http(s) scheme. This pins the exact
        /// acceptance boundary against arbitrary inputs.
        #[test]
        fn validate_accepts_iff_http_scheme(
            server in prop_oneof![".*", "https?://[a-zA-Z0-9./:_-]{0,24}"],
        ) {
            let expected_ok =
                server.starts_with("http://") || server.starts_with("https://");
            let got_ok = cfg_with_server(server.clone()).validate().is_ok();
            prop_assert_eq!(
                got_ok,
                expected_ok,
                "server {:?}: validate()={}, expected {}",
                server,
                got_ok,
                expected_ok,
            );
        }

        /// `Pat`'s `Debug` output is a fixed template that depends only on
        /// the secret's length — never its content. Equality with the
        /// length-only template is the precise non-leak contract (a
        /// "does not contain" check would false-positive on short secrets
        /// like "P" or secrets equal to "redacted").
        #[test]
        fn pat_debug_depends_only_on_length(secret in ".{0,64}") {
            let rendered = format!("{:?}", Pat::new(secret.clone()));
            let expected = format!("Pat(***redacted, len={})", secret.len());
            prop_assert_eq!(rendered, expected);
        }
    }
}
