//! The outline of documents and configuration files: Markdown headings and the like.
//! These are the entries File Structure shows; none of them is a declaration that
//! navigation can land on from code.

use dumbj_text::{ByteOffset, ByteRange};
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

/// The key node of a TOML table header or pair: a bare, quoted or dotted key.
fn toml_key(node: Node<'_>) -> Option<Node<'_>> {
    named_children(node)
        .into_iter()
        .find(|child| matches!(child.kind(), "bare_key" | "quoted_key" | "dotted_key"))
}

/// Reports the tables of a TOML file and the keys in them. Keys of inline tables are not
/// listed; a pair before the first table has no parent.
pub(crate) fn toml(collector: &mut Collector<'_>, root: Node<'_>) {
    for item in named_children(root) {
        match item.kind() {
            "pair" => {
                if let Some(key) = toml_key(item) {
                    collector.push(SymbolKind::Key, item, key, None);
                }
            }
            "table" | "table_array_element" => {
                let Some(name) = toml_key(item) else {
                    continue;
                };
                let table = collector.push(SymbolKind::Table, item, name, None);
                for pair in named_children(item) {
                    if pair.kind() != "pair" {
                        continue;
                    }
                    if let Some(key) = toml_key(pair) {
                        collector.push(SymbolKind::Key, pair, key, Some(table));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The `Name` node of an XML element's start (or empty-element) tag.
fn xml_tag_name(element: Node<'_>) -> Option<Node<'_>> {
    let tag = named_children(element)
        .into_iter()
        .find(|child| matches!(child.kind(), "STag" | "EmptyElemTag"))?;
    named_children(tag)
        .into_iter()
        .find(|child| child.kind() == "Name")
}

/// The child elements of an XML element.
fn xml_children(element: Node<'_>) -> Vec<Node<'_>> {
    named_children(element)
        .into_iter()
        .filter(|child| child.kind() == "content")
        .flat_map(named_children)
        .filter(|child| child.kind() == "element")
        .collect()
}

/// The text content of an XML element without surrounding whitespace, and where it is.
fn xml_text(collector: &Collector<'_>, element: Node<'_>) -> Option<(String, ByteRange)> {
    let content = named_children(element)
        .into_iter()
        .find(|part| part.kind() == "content")?;
    let range = trimmed(collector, content, false)?;
    let text = collector.source.buffer.text(range.clone()).ok()?;
    Some((text, range))
}

/// The text of the first child element called `tag`.
fn xml_child_text(
    collector: &Collector<'_>,
    element: Node<'_>,
    tag: &str,
) -> Option<(String, ByteRange)> {
    let child = xml_children(element).into_iter().find(|child| {
        xml_tag_name(*child).is_some_and(|name| collector.source.text(name) == tag)
    })?;
    xml_text(collector, child)
}

/// The name of a Maven `dependency` or `plugin`, `groupId:artifactId` (just the
/// `artifactId` when there is no group, as plugins often do), and where to land: the
/// artifact id, as a point when the name is not written in one piece.
fn xml_artifact(collector: &Collector<'_>, element: Node<'_>) -> Option<(String, ByteRange)> {
    let (artifact, range) = xml_child_text(collector, element, "artifactId")?;
    match xml_child_text(collector, element, "groupId") {
        Some((group, _)) => Some((format!("{group}:{artifact}"), range.start..range.start)),
        None => Some((artifact, range)),
    }
}

/// Reports the XML elements worth an outline entry: the root, its direct children, and
/// anywhere below them the Maven elements that carry an identity, `module`, `dependency`
/// and `plugin`. A dependency or plugin is named `groupId:artifactId`, a module by its
/// path. Entries nest under the closest reported ancestor.
pub(crate) fn xml(collector: &mut Collector<'_>, root: Node<'_>) {
    if let Some(element) = named_children(root)
        .into_iter()
        .find(|child| child.kind() == "element")
    {
        xml_element(collector, element, 0, None);
    }
}

fn xml_element(
    collector: &mut Collector<'_>,
    element: Node<'_>,
    depth: usize,
    parent: Option<usize>,
) {
    let Some(tag) = xml_tag_name(element) else {
        return;
    };
    let identity = if depth < 2 {
        None
    } else {
        match collector.source.text(tag).as_str() {
            "module" => xml_text(collector, element),
            "dependency" | "plugin" => xml_artifact(collector, element),
            _ => None,
        }
    };
    let entry = if let Some((name, landing)) = identity {
        Some(collector.push_with_name(
            SymbolKind::Element,
            range_of(element),
            name,
            landing,
            parent,
        ))
    } else if depth < 2 {
        Some(collector.push(SymbolKind::Element, element, tag, parent))
    } else {
        None
    };
    for child in xml_children(element) {
        xml_element(collector, child, depth + 1, entry.or(parent));
    }
}

/// How deep nested YAML mappings are followed; guards the recursion against absurd input.
const MAX_YAML_DEPTH: usize = 24;

/// Reports the keys of the block mappings of a YAML file, nested the way the mappings
/// are. Keys inside sequences and flow mappings (`{a: 1}`) are not listed, nor are merge
/// keys (`<<`).
pub(crate) fn yaml(collector: &mut Collector<'_>, root: Node<'_>) {
    for document in named_children(root) {
        if document.kind() == "document" {
            yaml_node(collector, document, None, 0);
        }
    }
}

/// Finds the block mappings at or below `node` (through the wrappers a document and a
/// value put around them) and reports their pairs.
fn yaml_node(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>, depth: usize) {
    if depth > MAX_YAML_DEPTH {
        return;
    }
    for child in named_children(node) {
        match child.kind() {
            "block_node" => yaml_node(collector, child, parent, depth),
            "block_mapping" => {
                for pair in named_children(child) {
                    if pair.kind() != "block_mapping_pair" {
                        continue;
                    }
                    let Some(key) = pair.child_by_field_name("key") else {
                        continue;
                    };
                    if collector.source.text(key) == "<<" {
                        continue;
                    }
                    let entry = collector.push(SymbolKind::Key, pair, key, parent);
                    if let Some(value) = pair.child_by_field_name("value") {
                        yaml_node(collector, value, Some(entry), depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}
