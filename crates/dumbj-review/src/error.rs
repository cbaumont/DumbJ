#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("the gh program was not found; install the GitHub CLI and run `gh auth login`")]
    GhMissing,
    #[error("gh has no token for {host}; run `gh auth login --hostname {host}`")]
    NotSignedIn { host: String },
    #[error("GitHub rejected the token (401); run `gh auth login` again")]
    Unauthorized,
    #[error("GitHub request failed: {0}")]
    Http(String),
    #[error("GitHub API error: {0}")]
    Api(String),
    #[error("unexpected GitHub response: {0}")]
    Decode(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<serde_json::Error> for ReviewError {
    fn from(error: serde_json::Error) -> Self {
        Self::Decode(error.to_string())
    }
}
