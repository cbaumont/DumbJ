mod buffer;
pub mod disk;
mod history;
mod position;
pub mod search;

pub use buffer::{Buffer, Edit, TextError, is_word_char};
pub use history::{Carets, EditKind, History};
pub use position::{ByteOffset, ByteRange, LineIndex, Point};
