//! The on-disk index cache: round trips, reuse, invalidation and recovery.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::shop_root;
use dumbj_index::{FileStamp, Index, IndexStore, cache_path, index_project, index_project_cached};
use dumbj_syntax::{FileFacts, Language};

const MONEY: &str = "core/src/main/java/com/shop/core/Money.java";

/// A scratch directory that is removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dumbj-store-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn db(&self) -> PathBuf {
        self.0.join("cache/index.sqlite")
    }

    /// A copy of the fixture project that tests may modify.
    fn project(&self) -> PathBuf {
        let root = self.0.join("project");
        copy_dir(&shop_root(), &root);
        root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

type Summary = Vec<(String, Option<Language>, FileFacts)>;

fn summary(index: &Index) -> Summary {
    let mut files: Summary = index
        .files()
        .map(|(_, file)| (file.relative.clone(), file.language, file.facts.clone()))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn run_cached(root: &Path, db: &Path) -> Index {
    let mut store = IndexStore::open(db).unwrap();
    index_project_cached(root, &mut store, &AtomicBool::new(false), |_| {}).unwrap()
}

fn symbol_names(index: &Index, relative: &str) -> Vec<String> {
    let (_, file) = index
        .files()
        .find(|(_, file)| file.relative == relative)
        .unwrap();
    file.facts.symbols.iter().map(|s| s.name.clone()).collect()
}

fn sql(db: &Path, statement: &str) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute_batch(statement)
        .unwrap();
}

#[test]
fn cached_index_equals_the_uncached_one_and_survives_a_second_run() {
    let scratch = Scratch::new("roundtrip");
    let root = shop_root();
    let expected = summary(&index_project(&root, &AtomicBool::new(false), |_| {}).unwrap());
    assert!(expected.len() > 1);
    assert_eq!(summary(&run_cached(&root, &scratch.db())), expected);
    assert_eq!(summary(&run_cached(&root, &scratch.db())), expected);
}

#[test]
fn unchanged_files_are_taken_from_the_cache() {
    let scratch = Scratch::new("reuse");
    let root = scratch.project();
    run_cached(&root, &scratch.db());
    sql(
        &scratch.db(),
        &format!(
            "UPDATE symbols SET name = 'FromCache' WHERE idx = 0
             AND file_id = (SELECT id FROM files WHERE path = '{MONEY}')"
        ),
    );
    let index = run_cached(&root, &scratch.db());
    assert_eq!(symbol_names(&index, MONEY)[0], "FromCache");
}

#[test]
fn modified_deleted_and_new_files_are_brought_up_to_date() {
    let scratch = Scratch::new("changes");
    let root = scratch.project();
    run_cached(&root, &scratch.db());
    let gone = "core/src/main/java/com/shop/core/Priced.java";
    assert!(root.join(gone).exists());

    fs::write(
        root.join(MONEY),
        "package com.shop.core;\npublic class Rewritten { void extra() {} }\n",
    )
    .unwrap();
    fs::remove_file(root.join(gone)).unwrap();
    fs::write(root.join("core/Fresh.java"), "class Fresh {}\n").unwrap();

    let index = run_cached(&root, &scratch.db());
    assert_eq!(symbol_names(&index, MONEY), ["Rewritten", "extra"]);
    assert!(index.files().all(|(_, file)| file.relative != gone));
    assert_eq!(symbol_names(&index, "core/Fresh.java"), ["Fresh"]);

    // The store agrees: a further run returns the same thing.
    assert_eq!(summary(&run_cached(&root, &scratch.db())), summary(&index));
    let store = IndexStore::open(&scratch.db()).unwrap();
    let stored = store.load().unwrap();
    assert!(!stored.contains_key(gone));
    assert!(stored.contains_key("core/Fresh.java"));
}

#[test]
fn a_corrupt_database_is_replaced() {
    let scratch = Scratch::new("corrupt");
    let root = shop_root();
    fs::create_dir_all(scratch.db().parent().unwrap()).unwrap();
    fs::write(scratch.db(), vec![0x5a_u8; 8192]).unwrap();
    let expected = summary(&index_project(&root, &AtomicBool::new(false), |_| {}).unwrap());
    assert_eq!(summary(&run_cached(&root, &scratch.db())), expected);
    assert!(
        IndexStore::open(&scratch.db())
            .unwrap()
            .file_count()
            .unwrap()
            > 0
    );
}

#[test]
fn a_database_with_another_schema_version_is_rebuilt() {
    let scratch = Scratch::new("version");
    let root = shop_root();
    run_cached(&root, &scratch.db());
    sql(&scratch.db(), "PRAGMA user_version = 9999");
    let store = IndexStore::open(&scratch.db()).unwrap();
    assert_eq!(store.file_count().unwrap(), 0);
    drop(store);
    let expected = summary(&index_project(&root, &AtomicBool::new(false), |_| {}).unwrap());
    assert_eq!(summary(&run_cached(&root, &scratch.db())), expected);
}

#[test]
fn rows_with_unknown_kinds_are_parsed_again() {
    let scratch = Scratch::new("unknown-kind");
    let root = scratch.project();
    let expected = summary(&run_cached(&root, &scratch.db()));
    sql(
        &scratch.db(),
        &format!(
            "UPDATE symbols SET kind = 'Bogus' WHERE idx = 0
             AND file_id = (SELECT id FROM files WHERE path = '{MONEY}')"
        ),
    );
    assert_eq!(summary(&run_cached(&root, &scratch.db())), expected);
}

#[test]
fn cancelling_returns_nothing_and_saves_nothing() {
    let scratch = Scratch::new("cancel");
    let mut store = IndexStore::open(&scratch.db()).unwrap();
    let cancelled = AtomicBool::new(true);
    assert!(index_project_cached(&shop_root(), &mut store, &cancelled, |_| {}).is_none());
    assert_eq!(store.file_count().unwrap(), 0);
}

#[test]
fn file_stamp_reflects_size_and_missing_files() {
    let scratch = Scratch::new("stamp");
    let file = scratch.0.join("a.txt");
    fs::write(&file, "four").unwrap();
    assert_eq!(FileStamp::of(&file).unwrap().size, 4);
    assert!(FileStamp::of(&scratch.0.join("missing")).is_none());
}

#[test]
fn cache_path_is_stable_distinct_and_outside_the_project() {
    let a = shop_root();
    let b = a.join("core");
    let path_a = cache_path(&a).unwrap();
    assert_eq!(cache_path(&a).unwrap(), path_a);
    assert_ne!(cache_path(&b).unwrap(), path_a);
    assert!(!path_a.starts_with(&a));
    assert_eq!(path_a.extension().unwrap(), "sqlite");
}
