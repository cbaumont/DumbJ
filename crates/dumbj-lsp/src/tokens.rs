use std::path::Path;
use std::time::Duration;

use lsp_types::request::SemanticTokensFullRequest;
use lsp_types::{
    PartialResultParams, SemanticTokensLegend, SemanticTokensParams, SemanticTokensResult,
    TextDocumentIdentifier, WorkDoneProgressParams,
};

use crate::{LspError, Server, path_to_uri};

/// Absolute UTF-16 coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub line: u32,
    pub start: u32,
    pub length: u32,
    pub kind: String,
    pub modifiers: Vec<String>,
}

pub(crate) fn decode(
    data: &[lsp_types::SemanticToken],
    legend: &SemanticTokensLegend,
) -> Vec<SemanticToken> {
    let (mut line, mut start) = (0_u32, 0_u32);
    let mut tokens = Vec::with_capacity(data.len());
    for token in data {
        if token.delta_line == 0 {
            start = start.saturating_add(token.delta_start);
        } else {
            line = line.saturating_add(token.delta_line);
            start = token.delta_start;
        }
        let Some(kind) = usize::try_from(token.token_type)
            .ok()
            .and_then(|index| legend.token_types.get(index))
        else {
            continue;
        };
        let modifiers = legend
            .token_modifiers
            .iter()
            .enumerate()
            .filter(|(bit, _)| *bit < 32 && token.token_modifiers_bitset & (1 << bit) != 0)
            .map(|(_, modifier)| modifier.as_str().to_owned())
            .collect();
        tokens.push(SemanticToken {
            line,
            start,
            length: token.length,
            kind: kind.as_str().to_owned(),
            modifiers,
        });
    }
    tokens
}

pub fn semantic_tokens(
    server: &Server,
    path: &Path,
    legend: &SemanticTokensLegend,
    timeout: Duration,
) -> Result<Vec<SemanticToken>, LspError> {
    let params = SemanticTokensParams {
        text_document: TextDocumentIdentifier {
            uri: path_to_uri(path),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let result = server
        .request::<SemanticTokensFullRequest>(params)
        .wait(timeout)?;
    let data = match result {
        Some(SemanticTokensResult::Tokens(tokens)) => tokens.data,
        Some(SemanticTokensResult::Partial(partial)) => partial.data,
        None => Vec::new(),
    };
    Ok(decode(&data, legend))
}

#[cfg(test)]
mod tests {
    use lsp_types::{SemanticTokenModifier, SemanticTokenType};

    use super::*;

    fn raw(
        delta_line: u32,
        delta_start: u32,
        length: u32,
        kind: u32,
        bits: u32,
    ) -> lsp_types::SemanticToken {
        lsp_types::SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: kind,
            token_modifiers_bitset: bits,
        }
    }

    #[test]
    fn relative_positions_become_absolute_and_indices_names() {
        let legend = SemanticTokensLegend {
            token_types: vec![SemanticTokenType::CLASS, SemanticTokenType::PROPERTY],
            token_modifiers: vec![
                SemanticTokenModifier::STATIC,
                SemanticTokenModifier::READONLY,
            ],
        };
        let tokens = decode(
            &[
                raw(2, 4, 3, 0, 0),
                raw(0, 6, 5, 1, 0b11),
                raw(1, 2, 1, 9, 0),
                raw(0, 3, 2, 1, 0b10),
            ],
            &legend,
        );
        let seen: Vec<_> = tokens
            .iter()
            .map(|t| {
                (
                    t.line,
                    t.start,
                    t.length,
                    t.kind.as_str(),
                    t.modifiers.join(","),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (2, 4, 3, "class", String::new()),
                (2, 10, 5, "property", "static,readonly".to_owned()),
                (3, 5, 2, "property", "readonly".to_owned()),
            ]
        );
    }
}

/// An inlay hint: text drawn before `position`, padding included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inlay {
    pub position: lsp_types::Position,
    pub label: String,
}

pub fn inlay_hints(
    server: &Server,
    path: &Path,
    end: lsp_types::Position,
    timeout: Duration,
) -> Result<Vec<Inlay>, LspError> {
    let params = lsp_types::InlayHintParams {
        text_document: TextDocumentIdentifier {
            uri: path_to_uri(path),
        },
        range: lsp_types::Range::new(lsp_types::Position::new(0, 0), end),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let hints = server
        .request::<lsp_types::request::InlayHintRequest>(params)
        .wait(timeout)?;
    Ok(hints.unwrap_or_default().into_iter().map(inlay).collect())
}

fn inlay(hint: lsp_types::InlayHint) -> Inlay {
    let text = match hint.label {
        lsp_types::InlayHintLabel::String(text) => text,
        lsp_types::InlayHintLabel::LabelParts(parts) => {
            parts.into_iter().map(|part| part.value).collect()
        }
    };
    let pad = |on: Option<bool>| if on == Some(true) { " " } else { "" };
    Inlay {
        position: hint.position,
        label: format!(
            "{}{}{}",
            pad(hint.padding_left),
            text.trim(),
            pad(hint.padding_right)
        ),
    }
}

#[cfg(test)]
mod inlay_tests {
    use super::*;

    #[test]
    fn labels_are_joined_and_padded() {
        let hint: lsp_types::InlayHint = serde_json::from_value(serde_json::json!({
            "position": {"line": 1, "character": 4},
            "label": [{"value": "name"}, {"value": ":"}],
            "paddingRight": true
        }))
        .unwrap();
        assert_eq!(inlay(hint).label, "name: ");
        let hint: lsp_types::InlayHint = serde_json::from_value(serde_json::json!({
            "position": {"line": 0, "character": 9},
            "label": ": Int",
            "paddingLeft": true
        }))
        .unwrap();
        assert_eq!(inlay(hint).label, " : Int");
    }
}
