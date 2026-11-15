//! Indexing the `shop` fixture project.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::{shop_index, shop_root};
use dumbj_index::{index_file, index_project};
use dumbj_syntax::SymbolKind;

#[test]
fn indexes_sources_and_skips_ignored_files() {
    let index = shop_index();
    let files: Vec<&str> = index
        .files()
        .map(|(_, file)| file.relative.as_str())
        .collect();
    assert_eq!(
        files,
        [
            ".gitignore",
            "app/build.gradle.kts",
            "app/src/main/java/com/shop/app/Cart.java",
            "app/src/main/kotlin/com/shop/app/Checkout.kt",
            "core/build.gradle.kts",
            "core/src/main/java/com/shop/core/Money.java",
            "core/src/main/java/com/shop/core/Priced.java",
            "core/src/main/java/com/shop/core/Receipt.java",
            "core/src/main/java/com/shop/util/Money.java",
            "core/src/main/kotlin/com/shop/core/Product.kt",
            "settings.gradle.kts",
        ],
        "build/ is git-ignored"
    );
}

#[test]
fn looks_symbols_up_by_name_package_and_fqn() {
    let index = shop_index();
    let fqns = |name: &str| -> Vec<String> {
        index
            .symbols_named(name)
            .iter()
            .map(|id| index.fqn(*id))
            .collect()
    };
    assert_eq!(
        fqns("Money"),
        [
            "com.shop.core.Money",
            "com.shop.core.Money.Money",
            "com.shop.util.Money"
        ]
    );
    assert_eq!(fqns("sample"), ["com.shop.core.Product.Companion.sample"]);
    assert_eq!(fqns("label"), ["com.shop.core.label"]);
    assert_eq!(fqns("nope"), [] as [String; 0]);

    assert_eq!(index.files_in_package("com.shop.core").len(), 4);
    assert_eq!(index.files_in_package("com.shop.nope").len(), 0);

    let money = index.symbols_by_fqn("com.shop.core.Money");
    assert_eq!(money.len(), 1);
    assert_eq!(index.symbol(money[0]).unwrap().kind, SymbolKind::Class);
    let members: Vec<&str> = index
        .members(money[0])
        .map(|(_, symbol)| symbol.name.as_str())
        .collect();
    assert_eq!(
        members,
        ["ZERO", "amount", "Money", "of", "add", "amount", "Currency"]
    );
    let currency = index.symbols_by_fqn("com.shop.core.Money.Currency")[0];
    assert_eq!(index.parent(currency), Some(money[0]));
    assert_eq!(index.parent(money[0]), None);
    assert_eq!(index.package_of(currency), "com.shop.core");
}

#[test]
fn updating_one_file_replaces_its_symbols() {
    let index = shop_index();
    let root = shop_root();
    let path = root.join("core/src/main/java/com/shop/core/Priced.java");
    let mut file = index_file(&root, &path, &AtomicBool::new(false));
    assert_eq!(
        file.relative,
        "core/src/main/java/com/shop/core/Priced.java"
    );
    file.facts.symbols.retain(|symbol| symbol.name != "isFree");
    let updated = index.with_file(file);
    assert_eq!(updated.file_count(), index.file_count());
    assert_eq!(updated.symbols_named("isFree").len(), 0);
    assert_eq!(
        index.symbols_named("isFree").len(),
        1,
        "the old snapshot is untouched"
    );
    assert_eq!(updated.symbol_count(), index.symbol_count() - 1);

    let removed = index.without_file(&path);
    assert_eq!(removed.file_count(), index.file_count() - 1);
    assert_eq!(removed.symbols_named("Priced").len(), 0);
    assert_eq!(removed.file_by_path(&path), None);
}

#[test]
fn reports_progress_and_can_be_cancelled() {
    let done = AtomicUsize::new(0);
    let index = index_project(&shop_root(), &AtomicBool::new(false), |count| {
        done.fetch_max(count, Ordering::Relaxed);
    })
    .unwrap();
    assert_eq!(done.load(Ordering::Relaxed), index.file_count());
    assert!(index_project(&shop_root(), &AtomicBool::new(true), |_| {}).is_none());
}

#[test]
fn discovers_modules_and_source_roots() {
    let index = shop_index();
    let modules: Vec<String> = index
        .project()
        .modules()
        .iter()
        .map(|module| {
            let roots: Vec<&str> = module
                .source_roots
                .iter()
                .map(|root| root.path.as_str())
                .collect();
            format!("{} [{}] {}", module.name, module.dir, roots.join(" "))
        })
        .collect();
    assert_eq!(
        modules,
        [
            ": [] ",
            ":app [app] app/src/main/java app/src/main/kotlin",
            ":core [core] core/src/main/java core/src/main/kotlin",
        ],
        "names come from settings.gradle.kts, roots from package declarations"
    );
    let cart = index
        .file_by_path(&shop_root().join("app/src/main/java/com/shop/app/Cart.java"))
        .unwrap();
    let money = index
        .file_by_path(&shop_root().join("core/src/main/java/com/shop/core/Money.java"))
        .unwrap();
    assert_ne!(index.module_of(cart), index.module_of(money));
    assert_eq!(
        index
            .project()
            .module(index.module_of(cart).unwrap())
            .unwrap()
            .name,
        ":app"
    );
}
