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
#[cfg(test)]
mod testing;
mod tokens;
mod uri;
mod watch;

pub use error::LspError;
pub use goto::{GotoKind, Place, goto, hover, references};
pub use hierarchy::{
    TypeItem, call_hierarchy_prepare, incoming_calls, outgoing_calls, subtypes, supertypes,
    type_hierarchy_prepare,
};
pub use position::{from_lsp, from_lsp_range, to_lsp};
pub use preset::{Detection, Environment, ServerKind, detect};
pub use server::{Pending, Server, ServerCommand, ServerEvent};
pub use session::{Session, SessionState};
pub use tokens::{Inlay, SemanticToken, inlay_hints, semantic_tokens};
pub use uri::{path_to_uri, uri_to_path};
pub use watch::{is_watched, stamp, sync_files};

#[cfg(test)]
mod tests;
