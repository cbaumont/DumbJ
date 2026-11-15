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

pub type ByteRange = Range<ByteOffset>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Point {
    pub line: LineIndex,
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
