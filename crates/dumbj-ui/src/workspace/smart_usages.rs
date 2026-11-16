use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dumbj_lsp::{Place, from_lsp, references, uri_to_path};
use dumbj_text::Buffer;
use gpui::{AppContext, Context, Task};

use super::Workspace;
use super::navigation::{Chooser, ChooserItem, Location};
use super::usages::{FindRow, FindWindow, summary};

const GIVE_UP_AFTER: Duration = Duration::from_secs(30);

pub(super) struct ExactUsage {
    location: Location,
    line: usize,
    text: String,
}

/// The places as usages in file and line order; `None` when one is not a readable file.
fn exact_usages(places: &[Place]) -> Option<Vec<ExactUsage>> {
    let mut buffers: HashMap<PathBuf, Buffer> = HashMap::new();
    let mut usages = Vec::with_capacity(places.len());
    for place in places {
        let path = uri_to_path(&place.uri)?;
        if !buffers.contains_key(&path) {
            let text = std::fs::read_to_string(&path).ok()?;
            buffers.insert(path.clone(), Buffer::new(&text));
        }
        let buffer = buffers.get(&path)?;
        usages.push(usage_in(path, buffer, place));
    }
    usages.sort_by(|a, b| {
        (&a.location.path, a.location.offset).cmp(&(&b.location.path, b.location.offset))
    });
    Some(usages)
}

fn usage_in(path: PathBuf, buffer: &Buffer, place: &Place) -> ExactUsage {
    let offset = from_lsp(buffer, place.range.start);
    let line = buffer.point_at(offset).line;
    ExactUsage {
        text: buffer.line(line).trim().to_owned(),
        line: line.display_number(),
        location: Location { path, offset },
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn rows(root: Option<&Path>, usages: Vec<ExactUsage>) -> Vec<FindRow> {
    let mut rows = Vec::new();
    let mut usages = usages.into_iter().peekable();
    while let Some(first) = usages.peek() {
        let path = first.location.path.clone();
        let relative = root
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(&path)
            .display()
            .to_string();
        let heading = rows.len();
        rows.push(FindRow::File { relative, count: 0 });
        let mut found = 0;
        while let Some(usage) = usages.next_if(|usage| usage.location.path == path) {
            found += 1;
            rows.push(FindRow::Usage {
                location: usage.location,
                line: usage.line,
                text: usage.text,
                maybe: false,
                ranges: Vec::new(),
            });
        }
        if let Some(FindRow::File { count, .. }) = rows.get_mut(heading) {
            *count = found;
        }
    }
    rows
}

impl Workspace {
    /// Asks the language server of the active file for the references of the symbol at
    /// the caret; `None` when there is no server to ask.
    pub(super) fn ask_exact_usages(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<Task<Option<Vec<ExactUsage>>>> {
        let origin = self.current_location(cx)?;
        let server = self.smart_server(&origin.path)?;
        let position = self.active_editor()?.read(cx).lsp_caret()?;
        Some(cx.background_spawn(async move {
            match references(&server, &origin.path, position, GIVE_UP_AFTER) {
                Ok(places) => exact_usages(&places).filter(|usages| !usages.is_empty()),
                Err(error) => {
                    tracing::debug!(%error, "the language server did not list references");
                    None
                }
            }
        }))
    }

    pub(super) fn show_exact_usages(
        &mut self,
        name: &str,
        usages: Vec<ExactUsage>,
        in_popup: bool,
        cx: &mut Context<Self>,
    ) {
        let files = usages
            .chunk_by(|a, b| a.location.path == b.location.path)
            .count();
        let title = summary(name, usages.len(), files);
        self.status_message = None;
        if in_popup {
            let items = usages
                .into_iter()
                .map(|usage| {
                    let label = format!("{}:{}", file_name(&usage.location.path), usage.line);
                    ChooserItem::new(label, usage.text, usage.location)
                })
                .collect();
            self.chooser = Some(Chooser::smart(title, items));
        } else {
            let root = self.project.as_ref().map(|project| project.root.as_path());
            self.find_window = Some(FindWindow::new(title, rows(root, usages), true));
        }
        self.pending_focus = Some(super::PendingFocus::Results);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use dumbj_lsp::path_to_uri;
    use lsp_types::{Position, Range};

    use super::*;

    fn usage(path: &str, line: usize) -> ExactUsage {
        ExactUsage {
            location: Location {
                path: PathBuf::from(path),
                offset: dumbj_text::ByteOffset(line * 10),
            },
            line,
            text: String::new(),
        }
    }

    #[test]
    fn rows_are_grouped_by_file_under_a_relative_heading() {
        let usages = vec![
            usage("/p/a/A.java", 1),
            usage("/p/a/A.java", 4),
            usage("/q/B.java", 2),
        ];
        let rows = rows(Some(Path::new("/p")), usages);
        let headings: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                FindRow::File { relative, count } => Some((relative.as_str(), *count)),
                FindRow::Usage { .. } => None,
            })
            .collect();
        assert_eq!(headings, [("a/A.java", 2), ("/q/B.java", 1)]);
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn a_usage_has_its_line_number_and_trimmed_text() {
        let buffer = Buffer::new("a\n    b.c()\n");
        let at = Position::new(1, 6);
        let place = Place {
            uri: path_to_uri(Path::new("/p/A.kt")),
            range: Range::new(at, at),
        };
        let usage = usage_in(PathBuf::from("/p/A.kt"), &buffer, &place);
        assert_eq!((usage.line, usage.text.as_str()), (2, "b.c()"));
        assert_eq!(usage.location.offset.get(), 8);
    }
}
