//! DumbJ's Language Server Protocol client: blocking and thread-based, no async runtime.
//! A [`Server`] talks to one language server over any pair of byte streams.

mod backoff;
mod error;
mod frame;
mod goto;
mod position;
mod preset;
mod server;
mod session;
mod uri;

pub use backoff::Backoff;
pub use error::LspError;
pub use goto::{GotoKind, Place, goto, hover, references};
pub use position::{from_lsp, from_lsp_range, to_lsp, to_lsp_range};
pub use preset::{Detection, Environment, ServerKind, detect, initialize_params};
pub use server::{Pending, Server, ServerCommand, ServerEvent};
pub use session::{Session, SessionState};
pub use uri::{path_to_uri, uri_to_path};

#[cfg(test)]
mod tests;
