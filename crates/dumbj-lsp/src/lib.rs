//! DumbJ's Language Server Protocol client: blocking and thread-based, no async runtime.
//! A [`Server`] talks to one language server over any pair of byte streams.

mod backoff;
mod error;
mod frame;
mod goto;
mod hierarchy;
mod position;
mod preset;
mod server;
mod session;
mod tokens;
mod uri;

pub use backoff::Backoff;
pub use error::LspError;
pub use goto::{GotoKind, Place, goto, hover, references};
pub use hierarchy::{
    call_hierarchy_prepare, incoming_calls, outgoing_calls, subtypes, supertypes,
    type_hierarchy_prepare,
};
pub use position::{from_lsp, from_lsp_range, to_lsp, to_lsp_range};
pub use preset::{Detection, Environment, ServerKind, detect, initialize_params};
pub use server::{Pending, Server, ServerCommand, ServerEvent};
pub use session::{Session, SessionState};
pub use tokens::{SemanticToken, semantic_tokens};
pub use uri::{path_to_uri, uri_to_path};

#[cfg(test)]
mod tests;
