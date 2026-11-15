//! The outline of documents and configuration files: Markdown headings and the like.
//! These are the entries File Structure shows; none of them is a declaration that
//! navigation can land on from code.

use dumbj_text::ByteOffset;
use tree_sitter::Node;

use crate::structure::{Collector, SymbolKind, named_children, range_of};

/// The sub-range of `node` without surrounding whitespace, or `None` when it is blank.
/// With `closing_hashes`, an ATX heading's optional closing `##` sequence is cut off too.
fn trimmed(
    collector: &Collector<'_>,
    node: Node<'_>,
    closing_hashes: bool,
) -> Option<dumbj_text::ByteRange> {
    let range = range_of(node);
    let text = collector.source.buffer.text(range.clone()).ok()?;
    let mut name = text.trim();
    if closing_hashes {
        let without = name.trim_end_matches('#');
        // `# C#` keeps its hash: a closing sequence is separated by a space.
        if without.len() != name.len() && (without.is_empty() || without.ends_with(' ')) {
            name = without.trim_end();
        }
    }
    if name.is_empty() {
        return None;
    }
    let start = range.start.get() + (text.len() - text.trim_start().len());
    Some(ByteOffset(start)..ByteOffset(start + name.len()))
}

/// The heading level of an ATX or setext heading node.
fn heading_level(heading: Node<'_>) -> Option<usize> {
    named_children(heading)
        .into_iter()
        .find_map(|child| match child.kind() {
            "atx_h1_marker" | "setext_h1_underline" => Some(1),
            "atx_h2_marker" | "setext_h2_underline" => Some(2),
            "atx_h3_marker" => Some(3),
            "atx_h4_marker" => Some(4),
            "atx_h5_marker" => Some(5),
            "atx_h6_marker" => Some(6),
            _ => None,
        })
}

/// Reports the headings of a Markdown document, nested by level: a heading's parent is
/// the closest preceding heading with a smaller level. Headings inside lists and quotes
/// are left out.
pub(crate) fn markdown(collector: &mut Collector<'_>, root: Node<'_>) {
    // The enclosing headings: (level, symbol index), levels strictly increasing.
    let mut open: Vec<(usize, usize)> = Vec::new();
    headings(collector, root, &mut open);
}

fn headings(collector: &mut Collector<'_>, node: Node<'_>, open: &mut Vec<(usize, usize)>) {
    for child in named_children(node) {
        match child.kind() {
            "section" => headings(collector, child, open),
            "atx_heading" | "setext_heading" => {
                let Some(level) = heading_level(child) else {
                    continue;
                };
                let Some(content) = child.child_by_field_name("heading_content") else {
                    continue;
                };
                let Some(name) = trimmed(collector, content, child.kind() == "atx_heading") else {
                    continue;
                };
                while open
                    .last()
                    .is_some_and(|&(open_level, _)| open_level >= level)
                {
                    open.pop();
                }
                let parent = open.last().map(|&(_, index)| index);
                let index =
                    collector.push_range(SymbolKind::Heading, range_of(child), name, parent);
                open.push((level, index));
            }
            _ => {}
        }
    }
}
