//! Shared helpers for tests that run against the `shop` fixture project.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the helpers"
)]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use dumbj_index::{Index, index_project};

pub(crate) fn shop_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shop")
}

pub(crate) fn shop_index() -> Index {
    index_project(&shop_root(), &AtomicBool::new(false), |_| {}).unwrap()
}
