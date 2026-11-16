//! Per-project state that is not part of the project: recent files and locations, and
//! bookmarks. It lives in `$XDG_STATE_HOME/dumbj/projects/<id>/`, never inside the project.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_RECENT_FILES: usize = 50;
pub const MAX_RECENT_LOCATIONS: usize = 100;

const STATE_FILE: &str = "state.toml";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("cannot read or write the project state: {0}")]
    Io(#[from] std::io::Error),
    #[error("the project state file is corrupt: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot encode the project state: {0}")]
    Encode(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentLocation {
    pub path: PathBuf,
    /// One-based.
    pub line: usize,
    /// One-based character column.
    pub column: usize,
    /// Byte offset in the file, so the place reopens exactly while the file is unchanged.
    pub offset: usize,
    /// The line of code at the location, as it was when the caret left it.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub path: PathBuf,
    /// One-based.
    pub line: usize,
    /// The line of code when the bookmark was set, to list it without reading the file.
    #[serde(default)]
    pub text: String,
    /// A digit or letter that jumps to the bookmark; anonymous bookmarks have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mnemonic: Option<char>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectState {
    /// Most recent first.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Most recent first.
    #[serde(default)]
    pub recent_locations: Vec<RecentLocation>,
    /// In creation order.
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
}

impl ProjectState {
    /// Reads the state in `dir`; a missing file is an empty state.
    pub fn load(dir: &Path) -> Result<Self, StateError> {
        match std::fs::read_to_string(dir.join(STATE_FILE)) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes the state to `dir` through a temporary file, so a crash cannot leave half of it.
    pub fn save(&self, dir: &Path) -> Result<(), StateError> {
        std::fs::create_dir_all(dir)?;
        let temporary = dir.join(format!("{STATE_FILE}.tmp"));
        std::fs::write(&temporary, toml::to_string(self)?)?;
        std::fs::rename(&temporary, dir.join(STATE_FILE))?;
        Ok(())
    }

    pub fn record_file(&mut self, path: &Path) {
        self.recent_files.retain(|recent| recent != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(MAX_RECENT_FILES);
    }

    /// A place on the same line replaces the older entry, so the list holds distinct lines.
    pub fn record_location(&mut self, location: RecentLocation) {
        self.recent_locations
            .retain(|recent| recent.path != location.path || recent.line != location.line);
        self.recent_locations.insert(0, location);
        self.recent_locations.truncate(MAX_RECENT_LOCATIONS);
    }

    /// Follows an edit of `path` that replaced the zero-based lines `start..=old_end` with
    /// `start..=new_end`, so bookmarks and recent locations stay on their code. Returns
    /// whether a bookmark moved.
    pub fn shift_lines(
        &mut self,
        path: &Path,
        start: usize,
        old_end: usize,
        new_end: usize,
    ) -> bool {
        if old_end == new_end {
            return false;
        }
        let shift = |line: &mut usize| {
            let zero_based = line.saturating_sub(1);
            let moved = if zero_based > old_end {
                zero_based - old_end + new_end
            } else if zero_based > start {
                zero_based.min(new_end)
            } else {
                zero_based
            };
            let changed = moved != zero_based;
            *line = moved + 1;
            changed
        };
        for location in &mut self.recent_locations {
            if location.path == path {
                shift(&mut location.line);
            }
        }
        let mut moved = false;
        for bookmark in &mut self.bookmarks {
            if bookmark.path == path {
                moved |= shift(&mut bookmark.line);
            }
        }
        moved
    }

    pub fn bookmark_at(&self, path: &Path, line: usize) -> Option<&Bookmark> {
        self.bookmarks
            .iter()
            .find(|bookmark| bookmark.path == path && bookmark.line == line)
    }

    pub fn bookmark_with_mnemonic(&self, mnemonic: char) -> Option<&Bookmark> {
        let mnemonic = mnemonic.to_ascii_uppercase();
        self.bookmarks
            .iter()
            .find(|bookmark| bookmark.mnemonic == Some(mnemonic))
    }

    /// Adds an anonymous bookmark, or removes whatever bookmark the line has.
    /// Returns whether the line is bookmarked afterwards.
    pub fn toggle_bookmark(&mut self, path: &Path, line: usize, text: &str) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks
            .retain(|bookmark| bookmark.path != path || bookmark.line != line);
        if self.bookmarks.len() < before {
            return false;
        }
        self.bookmarks.push(Bookmark {
            path: path.to_path_buf(),
            line,
            text: text.to_owned(),
            mnemonic: None,
        });
        true
    }

    /// Gives the line's bookmark `mnemonic`, creating the bookmark when there is none. A
    /// mnemonic is unique in the project, so it moves from wherever it was; setting the
    /// mnemonic the line already has removes the bookmark. Letters are case-insensitive.
    /// Returns whether the line is bookmarked afterwards.
    pub fn toggle_bookmark_with_mnemonic(
        &mut self,
        path: &Path,
        line: usize,
        mnemonic: char,
        text: &str,
    ) -> bool {
        let mnemonic = mnemonic.to_ascii_uppercase();
        if self
            .bookmark_at(path, line)
            .map(|bookmark| bookmark.mnemonic)
            == Some(Some(mnemonic))
        {
            return self.toggle_bookmark(path, line, text);
        }
        self.bookmarks
            .retain(|bookmark| bookmark.mnemonic != Some(mnemonic));
        match self
            .bookmarks
            .iter_mut()
            .find(|bookmark| bookmark.path == path && bookmark.line == line)
        {
            Some(bookmark) => bookmark.mnemonic = Some(mnemonic),
            None => self.bookmarks.push(Bookmark {
                path: path.to_path_buf(),
                line,
                text: text.to_owned(),
                mnemonic: Some(mnemonic),
            }),
        }
        true
    }
}

/// The state directory of the project at `root`: `$XDG_STATE_HOME/dumbj/projects/<id>`, or
/// under `$HOME/.local/state` without it. `None` when neither variable is set.
///
/// Runs `git` to find the root commit when `root` is a repository, so call it off the UI
/// thread.
#[must_use]
pub fn state_dir(root: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(state_dir_in(&base, root))
}

fn state_dir_in(base: &Path, root: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut identity = canonical.as_os_str().as_encoded_bytes().to_vec();
    if let Some(commit) = root_commit(&canonical) {
        identity.push(0);
        identity.extend_from_slice(commit.as_bytes());
    }
    base.join("dumbj")
        .join("projects")
        .join(format!("{:016x}", fnv1a(&identity)))
}

fn root_commit(root: &Path) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut commits: Vec<&str> = text.lines().map(str::trim).collect();
    commits.sort_unstable();
    commits.first().map(|commit| (*commit).to_owned())
}

/// 64-bit FNV-1a: specified, so ids stay stable across Rust versions.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dumbj-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn location(path: &str, line: usize) -> RecentLocation {
        RecentLocation {
            path: PathBuf::from(path),
            line,
            column: 1,
            offset: line * 10,
            text: format!("line {line}"),
        }
    }

    #[test]
    fn recent_files_are_most_recent_first_without_duplicates() {
        let mut state = ProjectState::default();
        for name in ["a", "b", "c", "a"] {
            state.record_file(Path::new(name));
        }
        assert_eq!(
            state.recent_files,
            [PathBuf::from("a"), PathBuf::from("c"), PathBuf::from("b")]
        );
    }

    #[test]
    fn bookmarks_and_locations_follow_edits() {
        let mut state = ProjectState::default();
        for line in [2, 5, 9] {
            state.toggle_bookmark(Path::new("a"), line, "");
            state.record_location(location("a", line));
        }
        state.toggle_bookmark(Path::new("b"), 9, "");
        assert!(state.shift_lines(Path::new("a"), 2, 2, 4));
        assert!(state.shift_lines(Path::new("a"), 6, 12, 6));
        let lines = |state: &ProjectState, path: &str| -> Vec<usize> {
            state
                .bookmarks
                .iter()
                .filter(|bookmark| bookmark.path == Path::new(path))
                .map(|bookmark| bookmark.line)
                .collect()
        };
        assert_eq!(lines(&state, "a"), [2, 7, 7]);
        assert_eq!(lines(&state, "b"), [9]);
        let recent: Vec<usize> = state.recent_locations.iter().map(|l| l.line).collect();
        assert_eq!(recent, [7, 7, 2]);
        assert!(!state.shift_lines(Path::new("a"), 0, 0, 0));
    }

    #[test]
    fn recent_files_are_capped() {
        let mut state = ProjectState::default();
        for number in 0..MAX_RECENT_FILES + 20 {
            state.record_file(Path::new(&format!("f{number}")));
        }
        assert_eq!(state.recent_files.len(), MAX_RECENT_FILES);
        assert_eq!(
            state.recent_files[0],
            PathBuf::from(format!("f{}", MAX_RECENT_FILES + 19))
        );
    }

    #[test]
    fn a_recent_location_replaces_the_older_one_on_the_same_line() {
        let mut state = ProjectState::default();
        state.record_location(location("a", 1));
        state.record_location(location("a", 2));
        state.record_location(location("b", 1));
        let mut again = location("a", 1);
        again.text = "edited".to_owned();
        state.record_location(again.clone());
        let lines: Vec<_> = state
            .recent_locations
            .iter()
            .map(|place| (place.path.to_string_lossy().into_owned(), place.line))
            .collect();
        assert_eq!(
            lines,
            [
                ("a".to_owned(), 1),
                ("b".to_owned(), 1),
                ("a".to_owned(), 2)
            ]
        );
        assert_eq!(state.recent_locations[0], again);
    }

    #[test]
    fn recent_locations_are_capped() {
        let mut state = ProjectState::default();
        for line in 1..=MAX_RECENT_LOCATIONS + 30 {
            state.record_location(location("a", line));
        }
        assert_eq!(state.recent_locations.len(), MAX_RECENT_LOCATIONS);
        assert_eq!(state.recent_locations[0].line, MAX_RECENT_LOCATIONS + 30);
    }

    #[test]
    fn toggling_a_bookmark_adds_then_removes_it() {
        let mut state = ProjectState::default();
        let path = Path::new("a.java");
        assert!(state.toggle_bookmark(path, 3, ""));
        assert!(state.toggle_bookmark(path, 7, ""));
        assert!(state.bookmark_at(path, 3).is_some());
        assert!(!state.toggle_bookmark(path, 3, ""));
        assert!(state.bookmark_at(path, 3).is_none());
        assert_eq!(state.bookmarks.len(), 1);
    }

    #[test]
    fn a_mnemonic_is_unique_and_moves_between_lines() {
        let mut state = ProjectState::default();
        let (a, b) = (Path::new("a"), Path::new("b"));
        assert!(state.toggle_bookmark_with_mnemonic(a, 1, '1', ""));
        assert!(state.toggle_bookmark_with_mnemonic(b, 5, '1', ""));
        assert!(state.bookmark_at(a, 1).is_none(), "1 moved to b:5");
        assert_eq!(state.bookmark_with_mnemonic('1').map(|m| m.line), Some(5));
        assert!(!state.toggle_bookmark_with_mnemonic(b, 5, '1', ""));
        assert_eq!(state.bookmarks, Vec::<Bookmark>::new());
    }

    #[test]
    fn a_mnemonic_can_be_added_to_or_changed_on_a_bookmarked_line() {
        let mut state = ProjectState::default();
        let path = Path::new("a");
        state.toggle_bookmark(path, 2, "");
        assert!(state.toggle_bookmark_with_mnemonic(path, 2, 'q', ""));
        assert_eq!(state.bookmarks.len(), 1);
        assert_eq!(state.bookmark_at(path, 2).unwrap().mnemonic, Some('Q'));
        assert!(state.toggle_bookmark_with_mnemonic(path, 2, 'Z', ""));
        assert_eq!(state.bookmark_at(path, 2).unwrap().mnemonic, Some('Z'));
        assert!(state.bookmark_with_mnemonic('q').is_none());
        assert!(state.bookmark_with_mnemonic('z').is_some());
    }

    #[test]
    fn the_state_round_trips_through_a_directory() {
        let dir = temp_dir("round-trip");
        let mut state = ProjectState::default();
        state.record_file(Path::new("/p/A.java"));
        state.record_location(location("/p/A.java", 12));
        state.toggle_bookmark(Path::new("/p/A.java"), 12, "int x;");
        state.toggle_bookmark_with_mnemonic(Path::new("/p/B.kt"), 4, '3', "");
        state.save(&dir.join("nested")).unwrap();
        assert_eq!(ProjectState::load(&dir.join("nested")).unwrap(), state);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_empty_state_and_a_corrupt_one_an_error() {
        let dir = temp_dir("missing");
        assert_eq!(ProjectState::load(&dir).unwrap(), ProjectState::default());
        std::fs::write(dir.join(STATE_FILE), "recent_files = 3").unwrap();
        assert!(matches!(
            ProjectState::load(&dir),
            Err(StateError::Parse(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_directory_depends_on_the_project_path() {
        let dir = temp_dir("id");
        let (one, two) = (dir.join("one"), dir.join("two"));
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        let base = dir.join("state");
        let first = state_dir_in(&base, &one);
        assert_eq!(first, state_dir_in(&base, &one));
        assert_ne!(first, state_dir_in(&base, &two));
        assert!(first.starts_with(base.join("dumbj").join("projects")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_git_repository_adds_its_root_commit_to_the_id() {
        let dir = temp_dir("git-id");
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .is_ok_and(|output| output.status.success())
        };
        if !git(&["init", "-q"])
            || !git(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "root",
            ])
        {
            return;
        }
        let base = dir.join("state");
        let with_commit = state_dir_in(&base, &dir);
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let without = base.join("dumbj").join("projects").join(format!(
            "{:016x}",
            fnv1a(canonical.as_os_str().as_encoded_bytes())
        ));
        assert_ne!(with_commit, without);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
