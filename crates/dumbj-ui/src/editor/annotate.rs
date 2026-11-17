use std::sync::Arc;

use gpui::Context;

use super::EditorView;

pub(super) const ANNOTATION_CHARS: usize = 26;

impl EditorView {
    pub(crate) fn set_annotations(
        &mut self,
        labels: Option<Arc<Vec<String>>>,
        cx: &mut Context<Self>,
    ) {
        self.annotations = labels;
        cx.notify();
    }

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
