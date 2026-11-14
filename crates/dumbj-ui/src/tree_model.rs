//! The data behind the Project tool window: a flat list of the visible rows of a lazily
//! loaded directory tree. No GPUI types, so it can be tested headless.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) is_dir: bool,
}

/// Lists a directory: folders first, then files, each sorted case-insensitively.
/// The `.git` directory is left out. Entries that cannot be read are skipped.
pub(crate) fn read_dir_sorted(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `metadata` follows symlinks, so a link to a directory can be expanded.
            let is_dir = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
            (name != ".git").then(|| Entry {
                path: entry.path(),
                name,
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            let (x, y) = (a.name.to_lowercase(), b.name.to_lowercase());
            x.cmp(&y).then_with(|| a.name.cmp(&b.name))
        })
    });
    Ok(entries)
}

/// A visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) entry: Entry,
    pub(crate) depth: usize,
    pub(crate) expanded: bool,
}

/// The visible rows of a directory tree, in display order.
#[derive(Debug, Default)]
pub(crate) struct TreeModel {
    rows: Vec<Row>,
}

impl TreeModel {
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Replaces the whole tree with the children of the project root.
    pub(crate) fn set_root_children(&mut self, children: Vec<Entry>) {
        self.rows = children
            .into_iter()
            .map(|entry| Row {
                entry,
                depth: 0,
                expanded: false,
            })
            .collect();
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|row| row.entry.path == path)
    }

    /// The index one past the last descendant of the row at `index`.
    fn subtree_end(&self, index: usize) -> usize {
        let Some(row) = self.rows.get(index) else {
            return index;
        };
        let rest = self.rows.iter().skip(index + 1);
        index + 1 + rest.take_while(|other| other.depth > row.depth).count()
    }

    /// Shows `children` under the directory at `path`. Does nothing if the directory is no
    /// longer visible (it was collapsed away while its listing was loading).
    pub(crate) fn expand(&mut self, path: &Path, children: Vec<Entry>) {
        let Some(index) = self.index_of(path) else {
            return;
        };
        let end = self.subtree_end(index);
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        if !row.entry.is_dir {
            return;
        }
        row.expanded = true;
        let depth = row.depth + 1;
        let children = children.into_iter().map(|entry| Row {
            entry,
            depth,
            expanded: false,
        });
        self.rows.splice(index + 1..end, children);
    }

    /// Hides everything under the directory at `index`.
    pub(crate) fn collapse(&mut self, index: usize) {
        let end = self.subtree_end(index);
        if let Some(row) = self.rows.get_mut(index) {
            row.expanded = false;
            self.rows.drain(index + 1..end);
        }
    }

    /// The index of the parent directory of the row at `index`, if it has one.
    pub(crate) fn parent_of(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        (0..index).rev().find(|candidate| {
            self.rows
                .get(*candidate)
                .is_some_and(|row| row.depth.cmp(&depth) == Ordering::Less)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> Entry {
        let path = PathBuf::from(path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        Entry { path, name, is_dir }
    }

    fn shown(model: &TreeModel) -> Vec<String> {
        model
            .rows()
            .iter()
            .map(|row| {
                let marker = match (row.entry.is_dir, row.expanded) {
                    (true, true) => "v ",
                    (true, false) => "> ",
                    (false, _) => "",
                };
                format!("{}{marker}{}", "  ".repeat(row.depth), row.entry.name)
            })
            .collect()
    }

    fn model() -> TreeModel {
        let mut model = TreeModel::default();
        model.set_root_children(vec![entry("/p/src", true), entry("/p/README.md", false)]);
        model.expand(
            Path::new("/p/src"),
            vec![entry("/p/src/main", true), entry("/p/src/a.kt", false)],
        );
        model.expand(
            Path::new("/p/src/main"),
            vec![entry("/p/src/main/App.java", false)],
        );
        model
    }

    #[test]
    fn expanding_inserts_children_in_place() {
        assert_eq!(
            shown(&model()),
            ["v src", "  v main", "    App.java", "  a.kt", "README.md"]
        );
    }

    #[test]
    fn collapsing_hides_the_whole_subtree() {
        let mut model = model();
        model.collapse(0);
        assert_eq!(shown(&model), ["> src", "README.md"]);
        model.collapse(1);
        assert_eq!(
            shown(&model),
            ["> src", "README.md"],
            "collapsing a file is a no-op"
        );
    }

    #[test]
    fn re_expanding_replaces_stale_children() {
        let mut model = model();
        model.expand(Path::new("/p/src"), vec![entry("/p/src/b.kt", false)]);
        assert_eq!(shown(&model), ["v src", "  b.kt", "README.md"]);
    }

    #[test]
    fn late_listings_for_hidden_directories_are_dropped() {
        let mut model = model();
        model.collapse(0);
        model.expand(
            Path::new("/p/src/main"),
            vec![entry("/p/src/main/X.java", false)],
        );
        model.expand(
            Path::new("/p/README.md"),
            vec![entry("/p/README.md/x", false)],
        );
        assert_eq!(shown(&model), ["> src", "README.md"]);
    }

    #[test]
    fn finds_parents() {
        let model = model();
        assert_eq!(model.parent_of(2), Some(1));
        assert_eq!(model.parent_of(3), Some(0));
        assert_eq!(model.parent_of(4), None);
        assert_eq!(model.parent_of(99), None);
    }

    #[test]
    fn lists_directories_first_and_skips_git() {
        let dir = std::env::temp_dir().join(format!("dumbj-tree-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["zeta", ".git", "Alpha"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in ["b.txt", "A.txt", ".hidden"] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        let names: Vec<String> = read_dir_sorted(&dir)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["Alpha", "zeta", ".hidden", "A.txt", "b.txt"]);
        assert!(read_dir_sorted(&dir).is_err());
    }
}
