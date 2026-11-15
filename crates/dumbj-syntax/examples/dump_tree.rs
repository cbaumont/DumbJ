//! Prints the tree-sitter syntax tree of a file, one node per line with field names.
//!
//! Usage: `cargo run -p dumbj-syntax --example dump_tree -- FILE`

#![expect(clippy::print_stdout, reason = "a command-line tool")]

use std::path::Path;

use tree_sitter::{Parser, TreeCursor};

fn dump(cursor: &mut TreeCursor<'_>, source: &str, depth: usize) {
    let node = cursor.node();
    if node.is_named() {
        let field = cursor
            .field_name()
            .map_or_else(String::new, |name| format!("{name}: "));
        let text = if node.child_count() == 0 {
            let text: String = source
                .get(node.byte_range())
                .unwrap_or("")
                .chars()
                .take(40)
                .collect();
            format!("  {text:?}")
        } else {
            String::new()
        };
        println!("{}{field}{}{text}", "  ".repeat(depth), node.kind());
    }
    if cursor.goto_first_child() {
        loop {
            dump(cursor, source, depth + 1);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        println!("usage: dump_tree FILE");
        return;
    };
    let path = Path::new(&path);
    let language = match path.extension().and_then(|ext| ext.to_str()) {
        Some("java") => tree_sitter_java::LANGUAGE.into(),
        Some("kt" | "kts") => tree_sitter_kotlin_sg::LANGUAGE.into(),
        Some("yaml" | "yml") => tree_sitter_yaml::LANGUAGE.into(),
        Some("xml") => tree_sitter_xml::LANGUAGE_XML.into(),
        Some("toml") => tree_sitter_toml_ng::LANGUAGE.into(),
        Some("md") => tree_sitter_md::LANGUAGE.into(),
        _ => {
            println!("unsupported file type");
            return;
        }
    };
    let mut parser = Parser::new();
    let (Ok(()), Ok(source)) = (
        parser.set_language(&language),
        std::fs::read_to_string(path),
    ) else {
        println!("cannot read or parse {}", path.display());
        return;
    };
    if let Some(tree) = parser.parse(&source, None) {
        dump(&mut tree.walk(), &source, 0);
    }
}
