//! Typed positions, so byte, line and UTF-16 offsets cannot be mixed up.

use std::ops::Range;

/// A byte offset into a buffer's UTF-8 text. Always on a `char` boundary when produced by
/// a [`Buffer`](crate::Buffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ByteOffset(pub usize);

impl ByteOffset {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub fn get(self) -> usize {
        self.0
    }
}

/// A half-open range of byte offsets.
pub type ByteRange = Range<ByteOffset>;

/// A zero-based line number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct LineIndex(pub usize);

impl LineIndex {
    #[must_use]
    pub fn get(self) -> usize {
        self.0
    }

    /// The one-based number shown in the gutter.
    #[must_use]
    pub fn display_number(self) -> usize {
        self.0 + 1
    }
}

/// A line and a byte column within that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Point {
    pub line: LineIndex,
    /// Bytes from the start of the line.
    pub column: usize,
}

impl Point {
    #[must_use]
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            line: LineIndex(line),
            column,
        }
    }
}
