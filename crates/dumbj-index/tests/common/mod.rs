#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the helpers"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use dumbj_index::{Index, Site, index_project};
use dumbj_syntax::{Language, SyntaxTree, parse};
use dumbj_text::{Buffer, ByteOffset};

pub(crate) const CART: &str = "app/src/main/java/com/shop/app/Cart.java";
pub(crate) const CHECKOUT: &str = "app/src/main/kotlin/com/shop/app/Checkout.kt";
pub(crate) const MONEY: &str = "core/src/main/java/com/shop/core/Money.java";
pub(crate) const PRICED: &str = "core/src/main/java/com/shop/core/Priced.java";
pub(crate) const RECEIPT: &str = "core/src/main/java/com/shop/core/Receipt.java";
pub(crate) const PRODUCT: &str = "core/src/main/kotlin/com/shop/core/Product.kt";

pub(crate) struct Caret {
    pub(crate) path: PathBuf,
    pub(crate) offset: ByteOffset,
    buffer: Buffer,
    tree: SyntaxTree,
}

impl Caret {
    pub(crate) fn site<'a>(&'a self, index: &'a Index) -> Site<'a> {
        Site {
            index,
            file: index.file_by_path(&self.path).unwrap(),
            tree: &self.tree,
            buffer: &self.buffer,
        }
    }
}

/// A `§` in `snippet` marks the caret; the snippet without it must occur once in `file`.
pub(crate) fn caret_in(root: &Path, file: &str, snippet: &str) -> Caret {
    let path = root.join(file);
    let source = std::fs::read_to_string(&path).unwrap();
    let plain = snippet.replacen('§', "", 1);
    assert_eq!(
        source.matches(&plain).count(),
        1,
        "`{plain}` must occur once in {file}"
    );
    let offset = ByteOffset(source.find(&plain).unwrap() + snippet.find('§').unwrap());
    let buffer = Buffer::new(&source);
    let tree = parse(Language::from_path(&path).unwrap(), &buffer, || false)
        .unwrap()
        .unwrap();
    Caret {
        path,
        offset,
        buffer,
        tree,
    }
}

pub(crate) fn shop_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shop")
}

pub(crate) fn shop_index() -> Index {
    index_project(&shop_root(), &AtomicBool::new(false), |_| {}).unwrap()
}

pub(crate) fn freight_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/freight")
}

pub(crate) fn freight_index() -> Index {
    index_project(&freight_root(), &AtomicBool::new(false), |_| {}).unwrap()
}
