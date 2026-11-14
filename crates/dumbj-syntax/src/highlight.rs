//! Syntax highlighting from tree-sitter highlight queries.

use dumbj_text::{Buffer, ByteOffset, ByteRange};
use tree_sitter::{Node, QueryCursor, StreamingIterator as _};

use crate::{SyntaxError, SyntaxTree};

/// The semantic role of a piece of source text. Themes map each role to a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    Comment,
    Keyword,
    String,
    StringEscape,
    Regex,
    Number,
    /// `true`, `false`, `null` and similar built-in values.
    ConstantBuiltin,
    Constant,
    Type,
    /// Primitive types such as `int`.
    TypeBuiltin,
    Constructor,
    Function,
    FunctionBuiltin,
    Variable,
    /// `this`, `super`, Kotlin's `it`.
    VariableBuiltin,
    Parameter,
    Property,
    /// Annotations.
    Attribute,
    Namespace,
    Label,
    Operator,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationSpecial,
}

impl HighlightKind {
    pub const ALL: &[Self] = &[
        Self::Comment,
        Self::Keyword,
        Self::String,
        Self::StringEscape,
        Self::Regex,
        Self::Number,
        Self::ConstantBuiltin,
        Self::Constant,
        Self::Type,
        Self::TypeBuiltin,
        Self::Constructor,
        Self::Function,
        Self::FunctionBuiltin,
        Self::Variable,
        Self::VariableBuiltin,
        Self::Parameter,
        Self::Property,
        Self::Attribute,
        Self::Namespace,
        Self::Label,
        Self::Operator,
        Self::PunctuationBracket,
        Self::PunctuationDelimiter,
        Self::PunctuationSpecial,
    ];

    /// Maps a query capture name such as `function.method` to a kind, falling back to the
    /// name's dotted prefixes. Unknown names give `None`.
    fn from_capture(name: &str) -> Option<Self> {
        let mut name = name;
        loop {
            let kind = match name {
                "comment" => Some(Self::Comment),
                // nvim-treesitter's older names for kinds of keyword.
                "keyword" | "repeat" | "conditional" | "include" | "exception" => {
                    Some(Self::Keyword)
                }
                "string" | "character" => Some(Self::String),
                "string.escape" => Some(Self::StringEscape),
                "string.regex" => Some(Self::Regex),
                "number" | "float" => Some(Self::Number),
                "boolean" | "constant.builtin" => Some(Self::ConstantBuiltin),
                "constant" => Some(Self::Constant),
                "type" => Some(Self::Type),
                "type.builtin" => Some(Self::TypeBuiltin),
                "constructor" => Some(Self::Constructor),
                "function" => Some(Self::Function),
                "function.builtin" => Some(Self::FunctionBuiltin),
                "variable" => Some(Self::Variable),
                "variable.builtin" => Some(Self::VariableBuiltin),
                "parameter" | "variable.parameter" => Some(Self::Parameter),
                "property" => Some(Self::Property),
                "attribute" => Some(Self::Attribute),
                "namespace" => Some(Self::Namespace),
                "label" => Some(Self::Label),
                "operator" => Some(Self::Operator),
                "punctuation.bracket" => Some(Self::PunctuationBracket),
                "punctuation.delimiter" => Some(Self::PunctuationDelimiter),
                "punctuation.special" => Some(Self::PunctuationSpecial),
                _ => None,
            };
            if kind.is_some() {
                return kind;
            }
            name = name.rsplit_once('.')?.0;
        }
    }
}

/// A run of text with one highlight kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: ByteRange,
    pub kind: HighlightKind,
}

/// Yields the buffer's text between two offsets as rope chunks, for query predicates.
struct Chunks<'a> {
    buffer: &'a Buffer,
    offset: usize,
    end: usize,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.end {
            return None;
        }
        let chunk = self.buffer.chunk_at(ByteOffset(self.offset)).as_bytes();
        let chunk = chunk.get(..chunk.len().min(self.end - self.offset))?;
        if chunk.is_empty() {
            return None;
        }
        self.offset += chunk.len();
        Some(chunk)
    }
}

/// One query capture: a node's range, the pattern that produced it and what it means.
/// `kind` is `None` for `@none`, which clears highlighting inside an enclosing capture.
struct Capture {
    start: usize,
    end: usize,
    pattern: usize,
    kind: Option<HighlightKind>,
}

impl SyntaxTree {
    /// The highlight spans that intersect `range`, sorted and non-overlapping.
    ///
    /// `buffer` must hold the text this tree was parsed from. Where captures nest, the
    /// innermost node wins; where several patterns capture the same node, the pattern
    /// that appears last in the query wins.
    pub fn highlights(
        &self,
        buffer: &Buffer,
        range: ByteRange,
    ) -> Result<Vec<HighlightSpan>, SyntaxError> {
        let query = self.language().highlights_query()?;
        let names = query.capture_names();
        let (range_start, range_end) = (range.start.get(), range.end.get().min(buffer.len().get()));
        if range_start >= range_end {
            return Ok(Vec::new());
        }

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range_start..range_end);
        let text = |node: Node<'_>| Chunks {
            buffer,
            offset: node.start_byte(),
            end: node.end_byte(),
        };
        let mut found = cursor.captures(query, self.tree().root_node(), text);
        let mut captures = Vec::new();
        while let Some((matched, capture_index)) = found.next() {
            let Some(capture) = matched.captures().get(*capture_index) else {
                continue;
            };
            let Some(name) = names.get(capture.index as usize) else {
                continue;
            };
            let kind = match *name {
                "none" => None,
                name => match HighlightKind::from_capture(name) {
                    Some(kind) => Some(kind),
                    // Private captures (`@_name`) and names no theme role exists for.
                    None => continue,
                },
            };
            let (start, end) = (capture.node.start_byte(), capture.node.end_byte());
            if start < end {
                captures.push(Capture {
                    start,
                    end,
                    pattern: matched.pattern_index,
                    kind,
                });
            }
        }
        // Outer nodes first; for the same node, later patterns last so they end up on top.
        captures.sort_by(|a, b| {
            (a.start, std::cmp::Reverse(a.end), a.pattern).cmp(&(
                b.start,
                std::cmp::Reverse(b.end),
                b.pattern,
            ))
        });
        Ok(flatten(&captures, range_start, range_end))
    }
}

/// Flattens nested captures into non-overlapping spans clipped to `clip_start..clip_end`.
fn flatten(captures: &[Capture], clip_start: usize, clip_end: usize) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut emit = |start: usize, end: usize, kind: Option<HighlightKind>| {
        let (start, end) = (start.max(clip_start), end.min(clip_end));
        let Some(kind) = kind else { return };
        if start >= end {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.kind == kind && last.range.end.get() == start => {
                last.range.end = ByteOffset(end);
            }
            _ => spans.push(HighlightSpan {
                range: ByteOffset(start)..ByteOffset(end),
                kind,
            }),
        }
    };

    // The captures enclosing the current position, innermost last.
    let mut open: Vec<&Capture> = Vec::new();
    let mut position = 0;
    for capture in captures {
        while let Some(top) = open.last().filter(|top| top.end <= capture.start) {
            emit(position, top.end, top.kind);
            position = position.max(top.end);
            open.pop();
        }
        if let Some(top) = open.last() {
            emit(position, capture.start, top.kind);
        }
        position = position.max(capture.start);
        open.push(capture);
    }
    while let Some(top) = open.pop() {
        emit(position, top.end, top.kind);
        position = position.max(top.end);
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(start: usize, end: usize, pattern: usize, kind: Option<HighlightKind>) -> Capture {
        Capture {
            start,
            end,
            pattern,
            kind,
        }
    }

    fn flat(
        mut captures: Vec<Capture>,
        clip: std::ops::Range<usize>,
    ) -> Vec<(usize, usize, HighlightKind)> {
        captures.sort_by_key(|c| (c.start, std::cmp::Reverse(c.end), c.pattern));
        flatten(&captures, clip.start, clip.end)
            .into_iter()
            .map(|span| (span.range.start.get(), span.range.end.get(), span.kind))
            .collect()
    }

    use HighlightKind::{Function, Keyword, String as Str, Variable};

    #[test]
    fn capture_names_fall_back_to_their_prefix() {
        assert_eq!(
            HighlightKind::from_capture("function.method"),
            Some(Function)
        );
        assert_eq!(HighlightKind::from_capture("keyword.return"), Some(Keyword));
        assert_eq!(
            HighlightKind::from_capture("string.escape"),
            Some(HighlightKind::StringEscape)
        );
        assert_eq!(
            HighlightKind::from_capture("string.special.symbol"),
            Some(Str)
        );
        assert_eq!(HighlightKind::from_capture("_function"), None);
        assert_eq!(HighlightKind::from_capture("nope.deeper"), None);
    }

    #[test]
    fn inner_captures_split_outer_ones() {
        let spans = flat(
            vec![
                capture(0, 10, 5, Some(Str)),
                capture(3, 5, 0, Some(Variable)),
            ],
            0..100,
        );
        assert_eq!(spans, [(0, 3, Str), (3, 5, Variable), (5, 10, Str)]);
    }

    #[test]
    fn later_pattern_wins_on_the_same_node() {
        let spans = flat(
            vec![
                capture(2, 6, 9, Some(Function)),
                capture(2, 6, 1, Some(Variable)),
            ],
            0..100,
        );
        assert_eq!(spans, [(2, 6, Function)]);
    }

    #[test]
    fn none_clears_and_adjacent_spans_merge() {
        let spans = flat(
            vec![
                capture(0, 10, 0, Some(Str)),
                capture(4, 6, 1, None),
                capture(10, 12, 0, Some(Str)),
                capture(20, 22, 0, Some(Keyword)),
            ],
            0..100,
        );
        assert_eq!(spans, [(0, 4, Str), (6, 12, Str), (20, 22, Keyword)]);
    }

    #[test]
    fn spans_are_clipped_to_the_requested_range() {
        let spans = flat(
            vec![
                capture(0, 10, 0, Some(Str)),
                capture(8, 9, 1, Some(Variable)),
                capture(30, 40, 0, Some(Keyword)),
            ],
            5..32,
        );
        assert_eq!(
            spans,
            [
                (5, 8, Str),
                (8, 9, Variable),
                (9, 10, Str),
                (30, 32, Keyword)
            ]
        );
    }
}
