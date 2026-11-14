//! DumbJ's text model: a rope [`Buffer`] addressed with typed positions, plus plain-text
//! search. No UI and no I/O.

mod buffer;
mod position;
pub mod search;

pub use buffer::{Buffer, Edit, TextError};
pub use position::{ByteOffset, ByteRange, LineIndex, Point};
