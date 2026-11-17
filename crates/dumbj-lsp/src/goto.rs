use std::path::Path;
use std::time::Duration;

use lsp_types::request::{GotoDefinition, GotoImplementation, GotoTypeDefinition};
use lsp_types::{
    GotoDefinitionParams, GotoDefinitionResponse, Location, PartialResultParams, Position, Range,
    TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
};

use crate::{LspError, Server, path_to_uri};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GotoKind {
    Definition,
    TypeDefinition,
    Implementation,
}

/// The range is in UTF-16 units of that document.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub uri: Uri,
    pub range: Range,
}

impl From<Location> for Place {
    fn from(location: Location) -> Self {
        Self {
            uri: location.uri,
            range: location.range,
        }
    }
}

pub(crate) fn position_params(path: &Path, position: Position) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier {
            uri: path_to_uri(path),
        },
        position,
    }
}

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
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location.into()],
        Some(GotoDefinitionResponse::Array(locations)) => {
            locations.into_iter().map(Place::from).collect()
        }
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| Place {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    })
}

pub fn references(
    server: &Server,
    path: &Path,
    position: Position,
    timeout: Duration,
) -> Result<Vec<Place>, LspError> {
    let params = lsp_types::ReferenceParams {
        text_document_position: position_params(path, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: lsp_types::ReferenceContext {
            include_declaration: false,
        },
    };
    let found = server
        .request::<lsp_types::request::References>(params)
        .wait(timeout)?;
    Ok(found
        .unwrap_or_default()
        .into_iter()
        .map(Place::from)
        .collect())
}

fn marked(text: lsp_types::MarkedString) -> String {
    match text {
        lsp_types::MarkedString::String(text) => text,
        lsp_types::MarkedString::LanguageString(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
    }
}

pub fn hover(
    server: &Server,
    path: &Path,
    position: Position,
    timeout: Duration,
) -> Result<Option<String>, LspError> {
    let params = lsp_types::HoverParams {
        text_document_position_params: position_params(path, position),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let hover = server
        .request::<lsp_types::request::HoverRequest>(params)
        .wait(timeout)?;
    let text = hover.map(|hover| match hover.contents {
        lsp_types::HoverContents::Scalar(text) => marked(text),
        lsp_types::HoverContents::Array(parts) => parts
            .into_iter()
            .map(marked)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lsp_types::HoverContents::Markup(markup) => markup.value,
    });
    Ok(text.filter(|text| !text.trim().is_empty()))
}
