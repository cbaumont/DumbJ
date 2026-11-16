//! Indexing a revision that is not checked out: the `shop` fixture with one file changed,
//! one added and one removed.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::{shop_index, shop_root};
use dumbj_index::{
    FileReader, Index, RevisionSource, Site, find_usages, index_revision, usage_target_at,
};
use dumbj_syntax::{Language, parse};
use dumbj_text::{Buffer, ByteOffset};

const MONEY: &str = "core/src/main/java/com/shop/core/Money.java";
const REFUND: &str = "core/src/main/java/com/shop/core/Refund.java";
const RECEIPT: &str = "core/src/main/java/com/shop/core/Receipt.java";
const REFUND_TEXT: &str = "package com.shop.core;\n\npublic class Refund {\n    \
    Money owed() {\n        return Money.ZERO.negate();\n    }\n}\n";

fn virtual_path(relative: &str) -> PathBuf {
    Path::new("/nowhere/shop!0123456789abcdef0123456789abcdef01234567").join(relative)
}

fn revision() -> (Index, Arc<HashMap<PathBuf, String>>) {
    let working = shop_index();
    let money = std::fs::read_to_string(shop_root().join(MONEY))
        .unwrap()
        .replace(
            "    public int amount() {",
            "    public Money negate() {\n        return new Money(-amount);\n    }\n\n    \
            public int amount() {",
        );
    let changed: Arc<HashMap<PathBuf, String>> = Arc::new(HashMap::from([
        (virtual_path(MONEY), money),
        (virtual_path(REFUND), REFUND_TEXT.to_owned()),
    ]));
    let mut sources: Vec<RevisionSource> = working
        .files()
        .filter(|(_, file)| file.relative != RECEIPT)
        .map(|(_, file)| RevisionSource {
            path: virtual_path(&file.relative),
            relative: file.relative.clone(),
            text: changed.get(&virtual_path(&file.relative)).cloned(),
        })
        .collect();
    sources.push(RevisionSource {
        path: virtual_path(REFUND),
        relative: REFUND.to_owned(),
        text: Some(REFUND_TEXT.to_owned()),
    });
    let texts = changed.clone();
    let reader = FileReader::new(move |path| {
        texts.get(path).cloned().or_else(|| {
            let relative = path.strip_prefix(virtual_path("")).ok()?;
            std::fs::read_to_string(shop_root().join(relative)).ok()
        })
    });
    let index = index_revision(&working, &sources, reader, &AtomicBool::new(false)).unwrap();
    (index, changed)
}

#[test]
fn the_revision_has_its_own_files_and_symbols() {
    let (index, _) = revision();
    assert!(index.file_by_path(&virtual_path(REFUND)).is_some());
    assert!(index.file_by_path(&virtual_path(RECEIPT)).is_none());
    assert!(index.file_by_path(&shop_root().join(MONEY)).is_none());
    assert_eq!(index.symbols_by_fqn("com.shop.core.Money.negate").len(), 1);
    assert_eq!(index.symbols_by_fqn("com.shop.core.Refund").len(), 1);
    assert_eq!(index.symbols_by_fqn("com.shop.core.Receipt").len(), 0);
    assert_eq!(index.symbols_by_fqn("com.shop.core.Product").len(), 1);
    let working = shop_index();
    assert_eq!(
        working.symbols_by_fqn("com.shop.core.Money.negate").len(),
        0
    );
}

#[test]
fn the_project_model_is_read_through_the_reader() {
    let (index, _) = revision();
    let modules = |index: &Index| -> Vec<String> {
        index
            .project()
            .modules()
            .iter()
            .map(|module| module.name.clone())
            .collect()
    };
    assert_eq!(modules(&index), modules(&shop_index()));
    assert!(modules(&index).len() > 1);
}

#[test]
fn usages_are_found_in_changed_and_unchanged_files() {
    let (index, texts) = revision();
    let path = virtual_path(MONEY);
    let source = &texts[&path];
    let offset = source.find("negate()").unwrap();
    let buffer = Buffer::new(source);
    let tree = parse(Language::Java, &buffer, || false).unwrap().unwrap();
    let site = Site {
        index: &index,
        file: index.file_by_path(&path).unwrap(),
        tree: &tree,
        buffer: &buffer,
    };
    let target = usage_target_at(site, ByteOffset(offset)).unwrap();
    let found: Vec<_> = find_usages(&index, &target, &AtomicBool::new(false))
        .unwrap()
        .into_iter()
        .map(|usage| (usage.path, usage.line + 1))
        .collect();
    assert_eq!(found, [(virtual_path(REFUND), 5)]);

    let offset = source.find("class Money").unwrap() + "class ".len();
    let target = usage_target_at(site, ByteOffset(offset)).unwrap();
    let files: Vec<_> = find_usages(&index, &target, &AtomicBool::new(false))
        .unwrap()
        .into_iter()
        .map(|usage| usage.path)
        .collect();
    assert!(files.contains(&virtual_path(REFUND)));
    assert!(files.contains(&virtual_path("app/src/main/java/com/shop/app/Cart.java")));
    assert!(files.iter().all(|path| path.starts_with(virtual_path(""))));
}

#[test]
fn a_cancelled_run_returns_nothing() {
    let working = shop_index();
    let sources = vec![RevisionSource {
        path: virtual_path(REFUND),
        relative: REFUND.to_owned(),
        text: Some(REFUND_TEXT.to_owned()),
    }];
    let cancelled = AtomicBool::new(true);
    assert!(index_revision(&working, &sources, FileReader::default(), &cancelled).is_none());
}
