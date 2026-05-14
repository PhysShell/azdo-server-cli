use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AzdoError, AzdoResult};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: String,
    pub collection: String,
    pub project: String,
    #[serde(default = "default_api_version")]
    pub api_version: String,
    #[serde(default)]
    pub auth: AuthConfig,
    #[allow(dead_code)]
    #[serde(default)]
    pub wiki: Option<WikiConfig>,
    #[allow(dead_code)]
    #[serde(default)]
    pub states: BTreeMap<String, String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub products: BTreeMap<String, ProductConfig>,
    #[allow(dead_code)]
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_pat_env")]
    pub pat_env: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            pat_env: default_pat_env(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct WikiConfig {
    pub id: String,
    pub daily_path: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ProductConfig {
    pub build_definition_id: Option<u32>,
    pub test_state: Option<String>,
    pub test_comment_template: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ProfileConfig {
    #[serde(default)]
    pub users: Vec<String>,
}

fn default_api_version() -> String {
    "6.0".to_string()
}

fn default_pat_env() -> String {
    "AZDO_PAT".to_string()
}

pub struct Pat(String);

impl Pat {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Pat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pat(***redacted, len={})", self.0.len())
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> AzdoResult<Self> {
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
        let raw = std::fs::read_to_string(&resolved)?;
        let cfg: Config = toml::from_str(&raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    #[cfg(test)]
    pub fn from_str_for_tests(raw: &str) -> AzdoResult<Self> {
        let cfg: Config = toml::from_str(raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> AzdoResult<()> {
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

    pub fn read_pat(&self) -> AzdoResult<Pat> {
        match std::env::var(&self.auth.pat_env) {
            Ok(v) if !v.is_empty() => Ok(Pat::new(v)),
            _ => Err(AzdoError::PatMissing(self.auth.pat_env.clone())),
        }
    }
}

/// Resolve default config path:
///   Windows: %APPDATA%\azdo\config.toml
///   Linux:   $XDG_CONFIG_HOME/azdo/config.toml, fallback $HOME/.config/azdo/config.toml
pub fn default_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|s| PathBuf::from(s).join("azdo").join("config.toml"))
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            Some(PathBuf::from(xdg).join("azdo").join("config.toml"))
        } else {
            std::env::var_os("HOME").map(|h| {
                PathBuf::from(h)
                    .join(".config")
                    .join("azdo")
                    .join("config.toml")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn parses_full_config() {
        let cfg = Config::from_str_for_tests(SAMPLE).expect("must parse");
        assert_eq!(cfg.server, "https://azdo.company.local/tfs");
        assert_eq!(cfg.project, "Customs");
        assert_eq!(cfg.api_version, "6.0");
        assert_eq!(cfg.auth.pat_env, "AZDO_PAT");
        assert_eq!(cfg.states.get("test").unwrap(), "Ready for Test");
        assert_eq!(
            cfg.products.get("declaration").unwrap().build_definition_id,
            Some(42)
        );
        assert_eq!(
            cfg.profiles.get("support").unwrap().users,
            vec!["Ivan Petrov", "Maria Ivanova"]
        );
    }

    #[test]
    fn defaults_api_version_and_pat_env() {
        let raw = r#"
server = "https://x/tfs"
collection = "Col"
project = "P"
"#;
        let cfg = Config::from_str_for_tests(raw).expect("must parse");
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
        let err = Config::from_str_for_tests(raw).expect_err("must reject");
        assert!(matches!(err, AzdoError::Config(_)), "got {err:?}");
    }

    #[test]
    fn rejects_empty_required() {
        let raw = r#"
server = ""
collection = "Col"
project = "P"
"#;
        let err = Config::from_str_for_tests(raw).expect_err("must reject");
        assert!(matches!(err, AzdoError::Config(_)));
    }

    #[test]
    fn pat_debug_does_not_leak_value() {
        let pat = Pat::new("super-secret-token-do-not-print".to_string());
        let dbg = format!("{pat:?}");
        assert!(!dbg.contains("super-secret-token"));
        assert!(dbg.contains("redacted"));
    }
}
