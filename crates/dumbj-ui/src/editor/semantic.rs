use std::ops::Range;
use std::sync::Arc;

use dumbj_lsp::{Inlay, SemanticToken};
use dumbj_syntax::{HighlightKind, HighlightSpan};
use dumbj_text::{Buffer, ByteOffset, Edit};
use gpui::{Context, Hsla, TextRun};
use lsp_types::Position;

use super::EditorView;

/// The colour a token type gets; `None` leaves the tree-sitter colour in place.
fn kind_of(token: &SemanticToken) -> Option<HighlightKind> {
    let constant = token.modifiers.iter().any(|m| m == "static")
        && token.modifiers.iter().any(|m| m == "readonly");
    Some(match token.kind.as_str() {
        "namespace" | "package" => HighlightKind::Namespace,
        "class" | "interface" | "enum" | "struct" | "type" | "typeParameter" | "record" => {
            HighlightKind::Type
        }
        "enumMember" => HighlightKind::Constant,
        "method" | "function" => HighlightKind::Function,
        "property" | "field" | "variable" if constant => HighlightKind::Constant,
        "property" | "field" => HighlightKind::Property,
        "variable" => HighlightKind::Variable,
        "parameter" => HighlightKind::Parameter,
        "annotation" | "annotationMember" | "decorator" => HighlightKind::Attribute,
        "keyword" | "modifier" => HighlightKind::Keyword,
        _ => return None,
    })
}

fn spans(buffer: &Buffer, tokens: &[SemanticToken]) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = tokens
        .iter()
        .filter_map(|token| {
            let kind = kind_of(token)?;
            let start = dumbj_lsp::from_lsp(buffer, Position::new(token.line, token.start));
            let end = dumbj_lsp::from_lsp(
                buffer,
                Position::new(token.line, token.start.saturating_add(token.length)),
            );
            (end > start).then_some(HighlightSpan {
                range: start..end,
                kind,
            })
        })
        .collect();
    spans.sort_by_key(|span| span.range.start);
    spans
}

pub(super) fn overlay(base: Vec<HighlightSpan>, top: &[HighlightSpan]) -> Vec<HighlightSpan> {
    if top.is_empty() {
        return base;
    }
    let mut result = Vec::with_capacity(base.len() + top.len());
    let mut first = 0;
    for span in base {
        while top
            .get(first)
            .is_some_and(|t| t.range.end <= span.range.start)
        {
            first += 1;
        }
        let mut start = span.range.start;
        for cut in top[first..]
            .iter()
            .take_while(|t| t.range.start < span.range.end)
        {
            if cut.range.start > start {
                result.push(HighlightSpan {
                    range: start..cut.range.start,
                    kind: span.kind,
                });
            }
            start = start.max(cut.range.end);
        }
        if start < span.range.end {
            result.push(HighlightSpan {
                range: start..span.range.end,
                kind: span.kind,
            });
        }
    }
    result.extend_from_slice(top);
    result.sort_by_key(|span| span.range.start);
    result
}

pub(super) fn mark_inlays(
    runs: Vec<TextRun>,
    inlays: &[Range<usize>],
    color: Hsla,
    background: Hsla,
) -> Vec<TextRun> {
    if inlays.is_empty() {
        return runs;
    }
    let mut cuts: Vec<usize> = inlays.iter().flat_map(|r| [r.start, r.end]).collect();
    let mut result = Vec::with_capacity(runs.len() + 2 * inlays.len());
    let mut position = 0;
    for run in runs {
        let end = position + run.len;
        cuts.retain(|cut| *cut > position);
        let mut start = position;
        for cut in cuts.iter().copied().filter(|cut| *cut < end).chain([end]) {
            if cut <= start {
                continue;
            }
            let mut piece = run.clone();
            piece.len = cut - start;
            if inlays.iter().any(|r| r.start <= start && start < r.end) {
                piece.color = color;
                piece.background_color = Some(background);
            }
            result.push(piece);
            start = cut;
        }
        position = end;
    }
    result
}

/// Where `range` is after `edits`; `None` when an edit touched it.
pub(super) fn shifted(range: &Range<ByteOffset>, edits: &[Edit]) -> Option<Range<ByteOffset>> {
    let mut range = range.clone();
    for edit in edits {
        if range.start >= edit.old_end {
            let moved =
                |at: ByteOffset| ByteOffset(at.get() - edit.old_end.get() + edit.new_end.get());
            range = moved(range.start)..moved(range.end);
        } else if range.end > edit.start {
            return None;
        }
    }
    Some(range)
}

impl EditorView {
    pub(crate) fn set_semantic_tokens(
        &mut self,
        version: u64,
        tokens: &[SemanticToken],
        cx: &mut Context<Self>,
    ) {
        if !self.is_ready() || self.model.buffer().version() != version {
            return;
        }
        self.semantic = Arc::new(spans(self.model.buffer(), tokens));
        self.highlights = None;
        cx.notify();
    }

    pub(crate) fn set_inlays(&mut self, version: u64, inlays: &[Inlay], cx: &mut Context<Self>) {
        if !self.is_ready() || self.model.buffer().version() != version {
            return;
        }
        let buffer = self.model.buffer();
        let mut placed: Vec<(ByteOffset, String)> = inlays
            .iter()
            .filter(|inlay| !inlay.label.trim().is_empty())
            .map(|inlay| {
                (
                    dumbj_lsp::from_lsp(buffer, inlay.position),
                    inlay.label.clone(),
                )
            })
            .collect();
        placed.sort_by_key(|(at, _)| *at);
        self.inlays = Arc::new(placed);
        cx.notify();
    }

    /// The end of the buffer as a language server counts it.
    pub(crate) fn lsp_end(&self) -> Position {
        dumbj_lsp::to_lsp(self.model.buffer(), self.model.buffer().len())
    }

    pub(super) fn inlays_in(&self, start: ByteOffset, end: ByteOffset) -> Vec<(usize, &str)> {
        let first = self.inlays.partition_point(|(at, _)| *at < start);
        self.inlays[first..]
            .iter()
            .take_while(|(at, _)| *at <= end)
            .map(|(at, label)| (at.get() - start.get(), label.as_str()))
            .collect()
    }

    pub(super) fn shift_server_marks(&mut self, edits: &[Edit]) {
        if !self.inlays.is_empty() {
            self.inlays = Arc::new(
                self.inlays
                    .iter()
                    .filter_map(|(at, label)| {
                        Some((shifted(&(*at..*at), edits)?.start, label.clone()))
                    })
                    .collect(),
            );
        }
        if !self.semantic.is_empty() {
            self.semantic = Arc::new(
                self.semantic
                    .iter()
                    .filter_map(|span| {
                        Some(HighlightSpan {
                            range: shifted(&span.range, edits)?,
                            kind: span.kind,
                        })
                    })
                    .collect(),
            );
        }
        if !self.problems.is_empty() {
            self.problems = Arc::new(
                self.problems
                    .iter()
                    .filter_map(|problem| {
                        let mut problem = problem.clone();
                        problem.range = shifted(&problem.range, edits)?;
                        Some(problem)
                    })
                    .collect(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use dumbj_text::Point;

    use super::*;

    fn span(start: usize, end: usize, kind: HighlightKind) -> HighlightSpan {
        HighlightSpan {
            range: ByteOffset(start)..ByteOffset(end),
            kind,
        }
    }

    fn token(line: u32, start: u32, length: u32, kind: &str, modifiers: &[&str]) -> SemanticToken {
        SemanticToken {
            line,
            start,
            length,
            kind: kind.to_owned(),
            modifiers: modifiers.iter().map(|m| (*m).to_owned()).collect(),
        }
    }

    #[test]
    fn tokens_become_spans_and_constants_are_told_apart() {
        let buffer = Buffer::new("int a = B.C;\n");
        let found = spans(
            &buffer,
            &[
                token(0, 10, 1, "property", &["static", "readonly"]),
                token(0, 4, 1, "variable", &[]),
                token(0, 8, 1, "class", &[]),
                token(0, 0, 3, "unknownThing", &[]),
            ],
        );
        assert_eq!(
            found,
            [
                span(4, 5, HighlightKind::Variable),
                span(8, 9, HighlightKind::Type),
                span(10, 11, HighlightKind::Constant),
            ]
        );
    }

    #[test]
    fn the_overlay_cuts_holes_in_the_base() {
        let base = vec![
            span(0, 10, HighlightKind::Variable),
            span(12, 14, HighlightKind::Keyword),
        ];
        let top = [
            span(2, 4, HighlightKind::Type),
            span(6, 7, HighlightKind::Constant),
            span(13, 16, HighlightKind::Function),
        ];
        assert_eq!(
            overlay(base, &top),
            [
                span(0, 2, HighlightKind::Variable),
                span(2, 4, HighlightKind::Type),
                span(4, 6, HighlightKind::Variable),
                span(6, 7, HighlightKind::Constant),
                span(7, 10, HighlightKind::Variable),
                span(12, 13, HighlightKind::Keyword),
                span(13, 16, HighlightKind::Function),
            ]
        );
    }

    #[test]
    fn inlay_text_gets_its_own_runs() {
        let plain = gpui::hsla(0.0, 0.0, 1.0, 1.0);
        let faint = gpui::hsla(0.0, 0.0, 0.5, 1.0);
        let background = gpui::hsla(0.0, 0.0, 0.1, 1.0);
        let run = |len| TextRun {
            len,
            font: gpui::font("Mono"),
            color: plain,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = mark_inlays(vec![run(4), run(6)], &[2..5, 8..10], faint, background);
        let seen: Vec<_> = runs
            .iter()
            .map(|r| (r.len, r.background_color.is_some()))
            .collect();
        assert_eq!(
            seen,
            [(2, false), (2, true), (1, true), (3, false), (2, true)]
        );
    }

    #[test]
    fn ranges_move_with_edits_before_them_and_vanish_under_them() {
        let edit = Edit {
            start: ByteOffset(5),
            old_end: ByteOffset(7),
            new_end: ByteOffset(10),
            start_point: Point::new(0, 5),
            old_end_point: Point::new(0, 7),
            new_end_point: Point::new(0, 10),
        };
        let at = |start, end| ByteOffset(start)..ByteOffset(end);
        assert_eq!(shifted(&at(0, 5), &[edit]), Some(at(0, 5)));
        assert_eq!(shifted(&at(8, 9), &[edit]), Some(at(11, 12)));
        assert_eq!(shifted(&at(4, 6), &[edit]), None);
    }
}
