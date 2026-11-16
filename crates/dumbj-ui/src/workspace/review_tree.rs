//! The changed files of a review as a tree: folders first, chains of single folders joined.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    Folder {
        path: PathBuf,
        label: String,
        collapsed: bool,
    },
    /// Index into the review's changes.
    File(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Row {
    pub(super) depth: usize,
    pub(super) node: Node,
}

fn parts(path: &Path) -> Vec<String> {
    path.iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect()
}

/// The order of a file tree: at each level folders come before files, names compare
/// without case first.
pub(super) fn tree_order(a: &Path, b: &Path) -> Ordering {
    let (a, b) = (parts(a), parts(b));
    for (at, (x, y)) in a.iter().zip(&b).enumerate() {
        if x == y {
            continue;
        }
        let (x_is_folder, y_is_folder) = (a.len() > at + 1, b.len() > at + 1);
        return y_is_folder
            .cmp(&x_is_folder)
            .then_with(|| x.to_lowercase().cmp(&y.to_lowercase()))
            .then_with(|| x.cmp(y));
    }
    a.len().cmp(&b.len())
}

/// Rows for `paths`, which must be in [`tree_order`]. The children of the folders in
/// `collapsed` are left out.
pub(super) fn rows(paths: &[&Path], collapsed: &HashSet<PathBuf>) -> Vec<Row> {
    let files: Vec<(usize, Vec<String>)> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| (index, parts(path)))
        .collect();
    let mut out = Vec::new();
    level(&files, 0, 0, collapsed, &mut out);
    out
}

fn level(
    files: &[(usize, Vec<String>)],
    at: usize,
    depth: usize,
    collapsed: &HashSet<PathBuf>,
    out: &mut Vec<Row>,
) {
    let mut rest = files;
    while let Some((index, first)) = rest.first() {
        let Some(name) = first.get(at) else {
            rest = &rest[1..];
            continue;
        };
        if first.len() == at + 1 {
            out.push(Row {
                depth,
                node: Node::File(*index),
            });
            rest = &rest[1..];
            continue;
        }
        let inside = |file: &(usize, Vec<String>), at: usize, name: &String| {
            file.1.len() > at + 1 && file.1.get(at) == Some(name)
        };
        let count = rest.iter().take_while(|f| inside(f, at, name)).count();
        let (group, after) = rest.split_at(count);
        let mut end = at + 1;
        while let Some(next) = first.get(end)
            && group.iter().all(|file| inside(file, end, next))
        {
            end += 1;
        }
        let path: PathBuf = first.iter().take(end).collect();
        let label = first.get(at..end).unwrap_or_default().join("/");
        let folded = collapsed.contains(&path);
        out.push(Row {
            depth,
            node: Node::Folder {
                path,
                label,
                collapsed: folded,
            },
        });
        if !folded {
            level(group, end, depth + 1, collapsed, out);
        }
        rest = after;
    }
}

/// The row of the folder that holds the row at `index`.
pub(super) fn parent_of(rows: &[Row], index: usize) -> Option<usize> {
    let depth = rows.get(index)?.depth;
    (0..index).rev().find(|at| rows[*at].depth < depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(paths: &[&str], collapsed: &[&str]) -> Vec<String> {
        let mut paths: Vec<&Path> = paths.iter().map(Path::new).collect();
        paths.sort_by(|a, b| tree_order(a, b));
        let collapsed = collapsed.iter().map(PathBuf::from).collect();
        rows(&paths, &collapsed)
            .into_iter()
            .map(|row| {
                let text = match row.node {
                    Node::Folder { label, .. } => format!("{label}/"),
                    Node::File(index) => paths[index].file_name().unwrap().display().to_string(),
                };
                format!("{}{text}", "  ".repeat(row.depth))
            })
            .collect()
    }

    #[test]
    fn folders_come_first_and_single_chains_are_joined() {
        assert_eq!(
            shown(
                &[
                    "README.md",
                    "src/main/kotlin/B.kt",
                    "src/main/kotlin/a/A.kt",
                    "src/test/kotlin/T.kt",
                    "build.gradle.kts",
                ],
                &[]
            ),
            [
                "src/",
                "  main/kotlin/",
                "    a/",
                "      A.kt",
                "    B.kt",
                "  test/kotlin/",
                "    T.kt",
                "build.gradle.kts",
                "README.md",
            ]
        );
    }

    #[test]
    fn a_collapsed_folder_hides_its_files() {
        assert_eq!(
            shown(&["src/main/A.kt", "src/test/T.kt", "x.txt"], &["src/main"]),
            ["src/", "  main/", "  test/", "    T.kt", "x.txt"]
        );
    }

    #[test]
    fn a_folder_named_like_a_file_elsewhere_stays_apart() {
        assert_eq!(shown(&["a/b", "b"], &[]), ["a/", "  b", "b"]);
    }

    #[test]
    fn finds_the_parent_folder() {
        let paths = [Path::new("src/a/A.kt"), Path::new("src/B.kt")];
        let rows = rows(&paths, &HashSet::new());
        assert_eq!(parent_of(&rows, 2), Some(1));
        assert_eq!(parent_of(&rows, 3), Some(0));
        assert_eq!(parent_of(&rows, 0), None);
    }
}
