//! DumbJ's text model: a rope [`Buffer`] addressed with typed positions, plus plain-text
//! search. No UI and no I/O.

mod buffer;
mod history;
mod position;
pub mod search;

pub use buffer::{Buffer, Edit, TextError};
pub use history::{Carets, EditKind, History};
pub use position::{ByteOffset, ByteRange, LineIndex, Point};
