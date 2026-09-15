use thiserror::Error;

/// Errors surfaced by the core. Kept coarse on purpose: callers either retry a
/// feed, tell the user the feed is broken, or bubble up a storage failure.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),

    #[error("could not parse feed: {0}")]
    Feed(String),

    #[error("search failed: {0}")]
    Search(String),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("storage error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;
