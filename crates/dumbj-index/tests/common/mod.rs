//! Shared helpers for tests that run against the fixture projects.

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

/// A small project of overloads, constructors and Kotlin called from Java.
pub(crate) fn freight_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/freight")
}

pub(crate) fn freight_index() -> Index {
    index_project(&freight_root(), &AtomicBool::new(false), |_| {}).unwrap()
}
