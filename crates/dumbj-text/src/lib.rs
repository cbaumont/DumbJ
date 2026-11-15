//! DumbJ's text model: a rope [`Buffer`] addressed with typed positions, plus plain-text
//! search, undo history and crash-safe saving. No UI.

mod buffer;
pub mod disk;
mod history;
mod position;
pub mod search;

pub use buffer::{Buffer, Edit, TextError};
pub use history::{Carets, EditKind, History};
pub use position::{ByteOffset, ByteRange, LineIndex, Point};
