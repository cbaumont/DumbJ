//! Compares the candidate Kotlin grammars on real code (BRIEF §5.1).
//!
//! Usage: `cargo run --release -p dumbj-syntax --example kotlin_grammar_eval -- DIR...`
//! Parses every `.kt` and `.kts` file under the directories and reports how much of the code
//! ends up inside ERROR nodes. Add `--no-default-features --features kotlin-ng-eval` to
//! evaluate tree-sitter-kotlin-ng instead of the grammar DumbJ ships.

#![expect(clippy::print_stdout, reason = "a command-line report")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tree_sitter::{Language, Node, Parser};

#[derive(Default)]
struct Totals {
    files: usize,
    bytes: usize,
    files_with_errors: usize,
    error_nodes: usize,
    missing_nodes: usize,
    error_bytes: usize,
    time: Duration,
    worst: Vec<(usize, PathBuf)>,
}

/// Counts ERROR and MISSING nodes below `node`, and the bytes covered by outermost ERRORs.
fn count_errors(node: Node<'_>, totals: &mut Totals) -> usize {
    if !node.has_error() {
        return 0;
    }
    if node.is_error() {
        totals.error_nodes += 1;
        totals.error_bytes += node.byte_range().len();
        return node.byte_range().len();
    }
    if node.is_missing() {
        totals.missing_nodes += 1;
        return 0;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .map(|child| count_errors(child, totals))
        .sum()
}

/// The first ERROR node in document order.
fn first_error(node: Node<'_>) -> Option<Node<'_>> {
    if node.is_error() {
        return Some(node);
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).filter(Node::has_error).collect();
    children.into_iter().find_map(first_error)
}

fn kotlin_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != ".git" && entry.file_name() != "build" {
                kotlin_files(&path, out);
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext == "kt" || ext == "kts")
        {
            out.push(path);
        }
    }
}

fn evaluate(name: &str, language: &Language, files: &[PathBuf]) {
    let mut parser = Parser::new();
    if let Err(error) = parser.set_language(language) {
        println!("{name}: cannot load grammar: {error}");
        return;
    }
    let mut totals = Totals::default();
    for path in files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let started = Instant::now();
        let Some(tree) = parser.parse(&source, None) else {
            continue;
        };
        totals.time += started.elapsed();
        totals.files += 1;
        totals.bytes += source.len();
        let error_bytes = count_errors(tree.root_node(), &mut totals);
        if tree.root_node().has_error() {
            totals.files_with_errors += 1;
            totals.worst.push((error_bytes, path.clone()));
        }
    }
    totals.worst.sort_unstable_by(|a, b| b.cmp(a));
    #[expect(clippy::cast_precision_loss, reason = "report percentages")]
    let percent = |part: usize, whole: usize| 100.0 * part as f64 / whole.max(1) as f64;
    println!("== {name}");
    println!(
        "  files: {} ({:.1} MB), parsed in {:.2?} ({:.1} MB/s)",
        totals.files,
        percent(totals.bytes, 100_000_000),
        totals.time,
        percent(totals.bytes, 100_000_000) / totals.time.as_secs_f64().max(1e-9),
    );
    println!(
        "  files with errors: {} ({:.1}%)",
        totals.files_with_errors,
        percent(totals.files_with_errors, totals.files)
    );
    println!(
        "  ERROR nodes: {}, MISSING nodes: {}, bytes inside ERROR: {} ({:.3}%)",
        totals.error_nodes,
        totals.missing_nodes,
        totals.error_bytes,
        percent(totals.error_bytes, totals.bytes)
    );
    for (bytes, path) in totals.worst.iter().take(8) {
        println!("    {bytes:>7} error bytes  {}", path.display());
        // Show where the first error starts, to see which construct the grammar misses.
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(tree) = parser.parse(&source, None) else {
            continue;
        };
        if let Some(error) = first_error(tree.root_node()) {
            let row = error.start_position().row;
            let line = source.lines().nth(row).unwrap_or("").trim();
            println!(
                "            line {}: {}",
                row + 1,
                line.chars().take(90).collect::<String>()
            );
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for dir in std::env::args_os().skip(1) {
        kotlin_files(Path::new(&dir), &mut files);
    }
    files.sort();
    #[cfg(feature = "kotlin-sg")]
    let (name, language) = (
        "tree-sitter-kotlin-sg 0.4 (fwcd grammar)",
        tree_sitter_kotlin_sg::LANGUAGE,
    );
    #[cfg(not(feature = "kotlin-sg"))]
    let (name, language) = ("tree-sitter-kotlin-ng 1.1", tree_sitter_kotlin_ng::LANGUAGE);
    evaluate(name, &language.into(), &files);
}
