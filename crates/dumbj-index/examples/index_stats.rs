//! Indexes a directory and prints how long it took and what was found.
//!
//! Usage: `cargo run --release -p dumbj-index --example index_stats -- DIR`

#![expect(clippy::print_stdout, reason = "a command-line report")]

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use dumbj_index::{SearchScope, index_project, search};

fn main() {
    let Some(dir) = std::env::args_os().nth(1) else {
        println!("usage: index_stats DIR");
        return;
    };
    let started = Instant::now();
    let Some(index) = index_project(Path::new(&dir), &AtomicBool::new(false), |_| {}) else {
        return;
    };
    println!(
        "indexed {} files, {} symbols in {:.2?}",
        index.file_count(),
        index.symbol_count(),
        started.elapsed()
    );
    for query in ["Job", "launch", "CS", "flow"] {
        let started = Instant::now();
        let hits = search(&index, query, SearchScope::All, 50, || false).unwrap_or_default();
        println!(
            "search {query:?}: {} hits in {:.2?}",
            hits.len(),
            started.elapsed()
        );
    }
}
