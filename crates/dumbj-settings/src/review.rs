//! The local review in progress: which files were viewed and the notes written on them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewedFile {
    pub path: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteSide {
    Base,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewNote {
    /// Relative to the repository root.
    pub path: PathBuf,
    pub side: NoteSide,
    /// One-based, inclusive.
    pub first_line: usize,
    /// One-based, inclusive, not before `first_line`.
    pub last_line: usize,
    pub text: String,
    /// The source lines the note is about.
    #[serde(default)]
    pub excerpt: String,
}

impl ReviewNote {
    fn contains(&self, path: &Path, side: NoteSide, line: usize) -> bool {
        self.path == path && self.side == side && (self.first_line..=self.last_line).contains(&line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub base: String,
    /// `None` reviews the working tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default)]
    viewed: Vec<ViewedFile>,
    #[serde(default)]
    notes: Vec<ReviewNote>,
}

impl Review {
    #[must_use]
    pub fn new(base: &str, head: Option<&str>) -> Self {
        Self {
            base: base.to_owned(),
            head: head.map(str::to_owned),
            viewed: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// True only when the file was marked viewed with this same fingerprint, so a file that
    /// changed since is unviewed again.
    #[must_use]
    pub fn is_viewed(&self, path: &Path, fingerprint: &str) -> bool {
        self.viewed
            .iter()
            .any(|file| file.path == path && file.fingerprint == fingerprint)
    }

    /// Marks or unmarks; marking replaces any older fingerprint for the path.
    pub fn set_viewed(&mut self, path: &Path, fingerprint: &str, viewed: bool) {
        self.viewed.retain(|file| file.path != path);
        if viewed {
            self.viewed.push(ViewedFile {
                path: path.to_path_buf(),
                fingerprint: fingerprint.to_owned(),
            });
        }
    }

    /// Sorted by path, then side (base first), then first line.
    #[must_use]
    pub fn notes(&self) -> &[ReviewNote] {
        &self.notes
    }

    pub fn notes_for<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a ReviewNote> {
        self.notes.iter().filter(move |note| note.path == path)
    }

    /// Adds the note, replacing one with the same path, side and range. A note whose text is
    /// blank removes that note instead.
    pub fn set_note(&mut self, mut note: ReviewNote) {
        note.last_line = note.last_line.max(note.first_line);
        self.notes.retain(|other| {
            other.path != note.path
                || other.side != note.side
                || other.first_line != note.first_line
                || other.last_line != note.last_line
        });
        if note.text.trim().is_empty() {
            return;
        }
        self.notes.push(note);
        self.notes
            .sort_by(|a, b| (&a.path, a.side, a.first_line).cmp(&(&b.path, b.side, b.first_line)));
    }

    /// Removes the note on `side` of `path` whose range contains `line`; returns whether one
    /// was removed.
    pub fn remove_note(&mut self, path: &Path, side: NoteSide, line: usize) -> bool {
        let Some(index) = self
            .notes
            .iter()
            .position(|note| note.contains(path, side, line))
        else {
            return false;
        };
        self.notes.remove(index);
        true
    }

    #[must_use]
    pub fn note_at(&self, path: &Path, side: NoteSide, line: usize) -> Option<&ReviewNote> {
        self.notes
            .iter()
            .find(|note| note.contains(path, side, line))
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        let head = self.head.as_deref().unwrap_or("working tree");
        let mut out = format!("# Review notes: {}..{head}\n", self.base);
        if self.notes.is_empty() {
            out.push_str("\nNo notes.\n");
            return out;
        }
        let mut current: Option<&Path> = None;
        for note in &self.notes {
            if current != Some(note.path.as_path()) {
                current = Some(note.path.as_path());
                let _ = write!(out, "\n## {}\n", note.path.display());
            }
            let lines = if note.last_line > note.first_line {
                format!("Lines {}–{}", note.first_line, note.last_line)
            } else {
                format!("Line {}", note.first_line)
            };
            let side = if note.side == NoteSide::Base {
                " (base)"
            } else {
                ""
            };
            let _ = write!(out, "\n### {lines}{side}\n");
            let excerpt = note.excerpt.trim_end_matches(['\n', '\r']);
            if !excerpt.is_empty() {
                let language = note
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("");
                let fence = "`".repeat(longest_backtick_run(excerpt).max(2) + 1);
                let _ = write!(out, "\n{fence}{language}\n{excerpt}\n{fence}\n");
            }
            let _ = write!(out, "\n{}\n", note.text.trim());
        }
        out
    }
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, side: NoteSide, first: usize, last: usize, text: &str) -> ReviewNote {
        ReviewNote {
            path: PathBuf::from(path),
            side,
            first_line: first,
            last_line: last,
            text: text.to_owned(),
            excerpt: String::new(),
        }
    }

    #[test]
    fn a_changed_fingerprint_makes_the_file_unviewed() {
        let mut review = Review::new("main", None);
        let path = Path::new("a/B.java");
        assert!(!review.is_viewed(path, "one"));
        review.set_viewed(path, "one", true);
        assert!(review.is_viewed(path, "one"));
        assert!(!review.is_viewed(path, "two"));
        review.set_viewed(path, "two", true);
        assert!(review.is_viewed(path, "two"));
        assert!(!review.is_viewed(path, "one"));
        assert_eq!(review.viewed.len(), 1);
    }

    #[test]
    fn unmarking_forgets_the_file() {
        let mut review = Review::new("main", None);
        let path = Path::new("a");
        review.set_viewed(path, "x", true);
        review.set_viewed(path, "x", false);
        assert!(!review.is_viewed(path, "x"));
        assert_eq!(review.viewed.len(), 0);
    }

    #[test]
    fn a_note_with_the_same_range_is_replaced_and_blank_text_removes_it() {
        let mut review = Review::new("main", None);
        review.set_note(note("a", NoteSide::Head, 3, 3, "first"));
        review.set_note(note("a", NoteSide::Head, 3, 3, "second"));
        review.set_note(note("a", NoteSide::Head, 3, 5, "wider"));
        assert_eq!(review.notes().len(), 2);
        assert_eq!(review.notes()[0].text, "second");
        review.set_note(note("a", NoteSide::Head, 3, 3, "  \n"));
        assert_eq!(review.notes().len(), 1);
        assert_eq!(review.notes()[0].text, "wider");
        review.set_note(note("a", NoteSide::Head, 9, 9, " "));
        assert_eq!(review.notes().len(), 1);
    }

    #[test]
    fn a_reversed_range_is_normalised() {
        let mut review = Review::new("main", None);
        review.set_note(note("a", NoteSide::Head, 7, 2, "x"));
        assert_eq!(review.notes()[0].last_line, 7);
    }

    #[test]
    fn notes_are_sorted_by_path_side_and_line() {
        let mut review = Review::new("main", None);
        review.set_note(note("b", NoteSide::Base, 1, 1, "1"));
        review.set_note(note("a", NoteSide::Head, 9, 9, "2"));
        review.set_note(note("a", NoteSide::Head, 2, 2, "3"));
        review.set_note(note("a", NoteSide::Base, 20, 20, "4"));
        let order: Vec<&str> = review.notes().iter().map(|n| n.text.as_str()).collect();
        assert_eq!(order, ["4", "3", "2", "1"]);
        let for_a: Vec<_> = review.notes_for(Path::new("a")).collect();
        assert_eq!(for_a.len(), 3);
    }

    #[test]
    fn note_at_and_remove_use_range_containment() {
        let mut review = Review::new("main", None);
        review.set_note(note("a", NoteSide::Head, 4, 6, "x"));
        let path = Path::new("a");
        assert!(review.note_at(path, NoteSide::Head, 3).is_none());
        assert!(review.note_at(path, NoteSide::Head, 4).is_some());
        assert!(review.note_at(path, NoteSide::Head, 6).is_some());
        assert!(review.note_at(path, NoteSide::Head, 7).is_none());
        assert!(review.note_at(path, NoteSide::Base, 5).is_none());
        assert!(!review.remove_note(path, NoteSide::Base, 5));
        assert!(review.remove_note(path, NoteSide::Head, 5));
        assert_eq!(review.notes(), []);
    }

    #[test]
    fn markdown_without_notes() {
        let review = Review::new("main", Some("feature"));
        assert_eq!(
            review.to_markdown(),
            "# Review notes: main..feature\n\nNo notes.\n"
        );
    }

    #[test]
    fn markdown_for_a_single_line_head_note() {
        let mut review = Review::new("main", None);
        let mut one = note("src/File.java", NoteSide::Head, 12, 12, " Rename this. \n");
        one.excerpt = "int x = 1;\n".to_owned();
        review.set_note(one);
        assert_eq!(
            review.to_markdown(),
            "# Review notes: main..working tree\n\
             \n\
             ## src/File.java\n\
             \n\
             ### Line 12\n\
             \n\
             ```java\n\
             int x = 1;\n\
             ```\n\
             \n\
             Rename this.\n"
        );
    }

    #[test]
    fn markdown_for_files_and_a_base_range_note() {
        let mut review = Review::new("a1", Some("b2"));
        let mut base = note("Makefile", NoteSide::Base, 3, 5, "Why removed?");
        base.excerpt = "one\ntwo\nthree".to_owned();
        review.set_note(base);
        review.set_note(note("z.kt", NoteSide::Head, 1, 1, "Second file."));
        assert_eq!(
            review.to_markdown(),
            "# Review notes: a1..b2\n\
             \n\
             ## Makefile\n\
             \n\
             ### Lines 3–5 (base)\n\
             \n\
             ```\n\
             one\n\
             two\n\
             three\n\
             ```\n\
             \n\
             Why removed?\n\
             \n\
             ## z.kt\n\
             \n\
             ### Line 1\n\
             \n\
             Second file.\n"
        );
    }

    #[test]
    fn the_fence_outgrows_backticks_in_the_excerpt() {
        let mut review = Review::new("main", None);
        let mut inner = note("README.md", NoteSide::Head, 1, 2, "Fence.");
        inner.excerpt = "```sh\nls\n```".to_owned();
        review.set_note(inner);
        assert!(
            review
                .to_markdown()
                .contains("\n````md\n```sh\nls\n```\n````\n")
        );
    }

    #[test]
    fn a_review_round_trips_through_toml() {
        let mut review = Review::new("main", Some("feature"));
        review.set_viewed(Path::new("a"), "abc", true);
        let mut one = note("a", NoteSide::Base, 1, 2, "text");
        one.excerpt = "x\ny".to_owned();
        review.set_note(one);
        let text = toml::to_string(&review).unwrap();
        assert_eq!(toml::from_str::<Review>(&text).unwrap(), review);
    }
}
