use std::sync::Arc;

use gpui::Context;

use super::EditorView;

/// How many characters the annotation column holds.
pub(super) const ANNOTATION_CHARS: usize = 26;

impl EditorView {
    /// Sets what the gutter shows beside each line of the HEAD text (a date and an
    /// author); `None` hides the column. Lines that differ from HEAD stay blank.
    pub(crate) fn set_annotations(
        &mut self,
        labels: Option<Arc<Vec<String>>>,
        cx: &mut Context<Self>,
    ) {
        self.annotations = labels;
        cx.notify();
    }

    /// The width of the annotation column in characters; 0 when it is hidden.
    pub(super) fn annotation_width(&self) -> usize {
        self.annotations.as_ref().map_or(0, |labels| {
            let longest = labels.iter().map(|label| label.chars().count()).max();
            longest.unwrap_or(0).min(ANNOTATION_CHARS) + 2
        })
    }

    pub(crate) fn is_annotated(&self) -> bool {
        self.annotations.is_some()
    }

    pub(super) fn annotation(&self, line: usize) -> Option<&str> {
        let labels = self.annotations.as_ref()?;
        let label = labels.get(self.head_line(line)?)?;
        Some(label.as_str())
    }
}
