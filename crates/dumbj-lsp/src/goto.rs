use std::path::Path;
use std::time::Duration;

use lsp_types::request::{GotoDefinition, GotoImplementation, GotoTypeDefinition};
use lsp_types::{
    GotoDefinitionParams, GotoDefinitionResponse, PartialResultParams, Position, Range,
    TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
};

use crate::{LspError, Server, path_to_uri};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GotoKind {
    Definition,
    TypeDefinition,
    Implementation,
}

/// Where a server says a symbol is; the range is in UTF-16 units of that document.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub uri: Uri,
    pub range: Range,
}

pub(crate) fn position_params(path: &Path, position: Position) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier {
            uri: path_to_uri(path),
        },
        position,
    }
}

/// Asks where the symbol at `position` of `path` is declared, typed or implemented.
/// Blocks for up to `timeout`.
pub fn goto(
    server: &Server,
    kind: GotoKind,
    path: &Path,
    position: Position,
    timeout: Duration,
) -> Result<Vec<Place>, LspError> {
    let params = GotoDefinitionParams {
        text_document_position_params: position_params(path, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let response = match kind {
        GotoKind::Definition => server.request::<GotoDefinition>(params).wait(timeout),
        GotoKind::TypeDefinition => server.request::<GotoTypeDefinition>(params).wait(timeout),
        GotoKind::Implementation => server.request::<GotoImplementation>(params).wait(timeout),
    }?;
    Ok(match response {
        None => Vec::new(),
        Some(GotoDefinitionResponse::Scalar(location)) => vec![Place {
            uri: location.uri,
            range: location.range,
        }],
        Some(GotoDefinitionResponse::Array(locations)) => locations
            .into_iter()
            .map(|location| Place {
                uri: location.uri,
                range: location.range,
            })
            .collect(),
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| Place {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    })
}
