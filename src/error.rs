use std::io;

use thiserror::Error;
use toml::de::Error as TomlError;

#[derive(Debug, Error)]
pub(crate) enum AzdoError {
    #[error("config error: {0}")]
    Config(String),

    #[error("PAT env variable `{0}` not set or empty")]
    PatMissing(String),

    #[error("http error: {status} {url}\n{body}")]
    Http {
        status: u16,
        url: String,
        body: String,
    },

    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("toml parse error: {0}")]
    Toml(#[from] TomlError),

    #[error("invalid header: {0}")]
    Header(String),

    #[error("render error: {0}")]
    Render(String),
}

/// Exit codes the CLI reports for each error category.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitCode {
    Generic = 1,
    Config = 2,
    Transport = 3,
    HttpClient = 4,
    HttpServer = 5,
}

impl AzdoError {
    pub(crate) const fn exit_code(&self) -> ExitCode {
        match self {
            Self::Config(_) | Self::PatMissing(_) => ExitCode::Config,
            Self::Http { status, .. } => match *status {
                400..=499 => ExitCode::HttpClient,
                500..=599 => ExitCode::HttpServer,
                _ => ExitCode::Generic,
            },
            Self::Transport(_) => ExitCode::Transport,
            Self::Io(_) | Self::Toml(_) | Self::Header(_) | Self::Render(_) => ExitCode::Generic,
        }
    }
}

pub(crate) type AzdoResult<T> = Result<T, AzdoError>;
