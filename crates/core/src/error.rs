use std::path::PathBuf;

/// Errors produced by the core metadata layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unsupported or malformed maven coordinate `{0}`")]
    Maven(String),

    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Msg(s.into())
    }
}

/// Helper so `?` works on `io::Result` inside a function returning our `Error`
/// while keeping the failing path in the message.
pub trait IoResultExt<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
