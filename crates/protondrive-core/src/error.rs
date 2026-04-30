use thiserror::Error;

/// Result alias for the core crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error type for `protondrive-core`.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("TOML deserialize: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("keyring: {0}")]
    Keyring(String),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("Proton API error {code}: {message}")]
    Api { code: i64, message: String },

    #[error("crypto: {0}")]
    Crypto(String),

    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("invalid config: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Other(e.to_string())
    }
}
