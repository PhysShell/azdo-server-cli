use thiserror::Error;

#[derive(Debug, Error)]
pub enum AzdoError {
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
    Io(#[from] std::io::Error),

    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("invalid header: {0}")]
    Header(String),
}

impl AzdoError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AzdoError::Config(_) | AzdoError::PatMissing(_) => 2,
            AzdoError::Http { status, .. } => match *status {
                400..=499 => 4,
                500..=599 => 5,
                _ => 1,
            },
            AzdoError::Transport(_) => 3,
            AzdoError::Io(_) | AzdoError::Toml(_) | AzdoError::Header(_) => 1,
        }
    }
}

pub type AzdoResult<T> = Result<T, AzdoError>;
