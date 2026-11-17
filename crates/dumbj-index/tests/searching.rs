#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use common::{shop_index, shop_root};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use dumbj_index::{HitTarget, Index, SearchScope, file_structure, index_project, search};

fn names(index: &Index, query: &str, scope: SearchScope) -> Vec<String> {
    search(index, query, scope, 50, || false)
        .unwrap()
        .into_iter()
        .map(|hit| match hit.target {
            HitTarget::File(id) => {
                let relative = &index.file(id).unwrap().relative;
                relative.rsplit('/').next().unwrap().to_owned()
            }
            HitTarget::Symbol(id) => index.fqn(id),
        })
        .collect()
}

#[test]
fn go_to_class() {
    let index = shop_index();
    assert_eq!(
        names(&index, "money", SearchScope::Classes),
        ["com.shop.core.Money", "com.shop.util.Money"]
    );
    assert_eq!(
        names(&index, "Pr", SearchScope::Classes),
        ["com.shop.core.Priced", "com.shop.core.Product"]
    );
    assert_eq!(
        names(&index, "CO", SearchScope::Classes),
        [] as [String; 0],
        "Checkout has no capital O"
    );
    assert_eq!(
        names(&index, "curr", SearchScope::Classes),
        ["com.shop.core.Money.Currency"],
        "nested"
    );
    assert_eq!(
        names(&index, "zzz", SearchScope::Classes),
        [] as [String; 0]
    );
    assert_eq!(
        names(&index, "  ", SearchScope::Classes),
        [] as [String; 0],
        "an empty query finds nothing"
    );
}

#[test]
fn go_to_file() {
    let index = shop_index();
    assert_eq!(names(&index, "cart", SearchScope::Files), ["Cart.java"]);
    assert_eq!(
        names(&index, "money", SearchScope::Files),
        ["Money.java", "Money.java"]
    );
    assert_eq!(
        names(&index, "util/money", SearchScope::Files),
        ["Money.java"],
        "a slash matches the path"
    );
    assert_eq!(
        names(&index, "settings", SearchScope::Files),
        ["settings.gradle.kts"]
    );
    assert_eq!(names(&index, "git", SearchScope::Files), [".gitignore"]);
}

#[test]
fn go_to_symbol() {
    let index = shop_index();
    let found = names(&index, "price", SearchScope::Symbols);
    assert_eq!(
        found,
        [
            "com.shop.app.Cart.price",
            "com.shop.core.Priced.price",
            "com.shop.core.Product.price",
            "com.shop.core.Priced"
        ],
        "exact names first, then the type that merely contains the letters"
    );
    assert_eq!(
        names(&index, "fM", SearchScope::Symbols),
        [
            "com.shop.core.formatMoney",
            "com.shop.core.Product.Companion.DEFAULT_NAME"
        ],
        "the hump match first, then deFault_naMe"
    );
    assert_eq!(
        names(&index, "DEFAULT", SearchScope::Symbols),
        ["com.shop.core.Product.Companion.DEFAULT_NAME"]
    );
}

#[test]
fn search_everywhere_mixes_kinds_with_classes_first() {
    let index = shop_index();
    let found = names(&index, "money", SearchScope::All);
    assert_eq!(
        found,
        [
            "com.shop.core.Money",
            "com.shop.util.Money",
            "com.shop.core.Money.Money",
            "Money.java",
            "Money.java",
            "com.shop.core.formatMoney",
        ],
        "classes, their constructor, files, then weaker symbol matches"
    );
}

#[test]
fn limits_and_cancellation() {
    let index = shop_index();
    assert_eq!(
        search(&index, "a", SearchScope::All, 3, || false)
            .unwrap()
            .len(),
        3
    );
    // The flag is only polled every so often, so a tiny index finishes before the first poll.
    assert!(search(&index, "a", SearchScope::All, 3, || true).is_some());
}

#[test]
fn match_positions_point_into_the_name() {
    let index = shop_index();
    let hits = search(&index, "fM", SearchScope::Symbols, 5, || false).unwrap();
    assert_eq!(hits[0].positions, [0, 6], "f and M of formatMoney");
}

#[test]
fn file_structure_lists_and_filters() {
    let index = shop_index();
    let file = index
        .file_by_path(&shop_root().join("core/src/main/java/com/shop/core/Money.java"))
        .unwrap();
    let list = |query: &str| -> Vec<String> {
        file_structure(&index, file, query)
            .into_iter()
            .map(|hit| match hit.target {
                HitTarget::Symbol(id) => index.qualified_name(id),
                HitTarget::File(_) => unreachable!(),
            })
            .collect()
    };
    assert_eq!(
        list(""),
        [
            "Money",
            "Money.ZERO",
            "Money.amount",
            "Money.Money",
            "Money.of",
            "Money.add",
            "Money.amount",
            "Money.Currency",
            "Money.Currency.code"
        ],
        "source order"
    );
    assert_eq!(list("am"), ["Money.amount", "Money.amount"]);
    assert_eq!(list("zzz"), [] as [String; 0]);
}

#[test]
fn document_outlines_show_in_file_structure_only() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/notes");
    let index = index_project(&root, &AtomicBool::new(false), |_| {}).unwrap();
    let file = index.file_by_path(&root.join("README.md")).unwrap();
    let structure: Vec<_> = file_structure(&index, file, "")
        .into_iter()
        .map(|hit| match hit.target {
            HitTarget::Symbol(id) => index.qualified_name(id),
            HitTarget::File(_) => unreachable!(),
        })
        .collect();
    assert_eq!(structure, ["Notes", "Notes.Money", "Notes.Money.Currency"]);
    assert_eq!(
        names(&index, "money", SearchScope::Symbols),
        [] as [String; 0],
        "headings are not declarations to go to"
    );
    assert_eq!(names(&index, "readme", SearchScope::Files), ["README.md"]);
}
