use crate::frame::FrameError;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("the language server has exited")]
    ServerExited,
    #[error("the language server did not answer in time")]
    Timeout,
    #[error("the language server answered with error {code}: {message}")]
    Response { code: i64, message: String },
    #[error("could not decode the message: {0}")]
    Decode(String),
    #[error("could not talk to the language server: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed message from the language server: {0}")]
    Frame(#[from] FrameError),
}
