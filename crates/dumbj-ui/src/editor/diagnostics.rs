use std::ops::Range;
use std::sync::Arc;

use dumbj_settings::UiRole;
use dumbj_text::{Buffer, ByteOffset};
use gpui::Context;
use lsp_types::DiagnosticSeverity;

use super::EditorView;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) range: Range<ByteOffset>,
    pub(crate) role: UiRole,
    pub(crate) message: String,
}

/// Errors, warnings and information; hints are left out, as IDEA shows them only on hover.
fn problems(buffer: &Buffer, diagnostics: &[lsp_types::Diagnostic]) -> Vec<Problem> {
    let mut problems: Vec<Problem> = diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let role = match diagnostic.severity {
                Some(DiagnosticSeverity::WARNING) => UiRole::Warning,
                Some(DiagnosticSeverity::INFORMATION) => UiRole::Info,
                Some(DiagnosticSeverity::HINT) => return None,
                _ => UiRole::Error,
            };
            Some(Problem {
                range: dumbj_lsp::from_lsp_range(buffer, diagnostic.range),
                role,
                message: diagnostic.message.lines().next().unwrap_or("").to_owned(),
            })
        })
        .collect();
    problems.sort_by_key(|problem| (problem.range.start, problem.range.end));
    problems
}

fn next_problem(problems: &[Problem], caret: ByteOffset, forward: bool) -> Option<ByteOffset> {
    let starts = problems.iter().map(|problem| problem.range.start);
    if forward {
        starts
            .clone()
            .find(|start| *start > caret)
            .or_else(|| starts.clone().next())
    } else {
        starts
            .clone()
            .rfind(|start| *start < caret)
            .or_else(|| starts.clone().next_back())
    }
}

impl EditorView {
    pub(crate) fn set_diagnostics(
        &mut self,
        diagnostics: &[lsp_types::Diagnostic],
        cx: &mut Context<Self>,
    ) {
        if !self.is_ready() {
            return;
        }
        let problems = problems(self.model.buffer(), diagnostics);
        if *self.problems != problems {
            self.problems = Arc::new(problems);
            cx.notify();
        }
    }

    pub(crate) fn problem_at_caret(&self) -> Option<&Problem> {
        let caret = self.model.caret();
        self.problems
            .iter()
            .filter(|problem| problem.range.start <= caret && caret <= problem.range.end)
            .min_by_key(|problem| problem.role != UiRole::Error)
    }

    pub(crate) fn go_to_problem(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(offset) = next_problem(&self.problems, self.model.caret(), forward) else {
            return false;
        };
        self.go_to_offset(offset, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::{Diagnostic, Position};

    use super::*;

    fn diagnostic(line: u32, from: u32, to: u32, severity: DiagnosticSeverity) -> Diagnostic {
        Diagnostic {
            range: lsp_types::Range::new(Position::new(line, from), Position::new(line, to)),
            severity: Some(severity),
            message: format!("problem on {line}\nsecond line"),
            ..Diagnostic::default()
        }
    }

    #[test]
    fn hints_are_dropped_and_the_rest_sorted_with_one_line_messages() {
        let buffer = Buffer::new("aaa\nbbb\nccc\n");
        let found = problems(
            &buffer,
            &[
                diagnostic(2, 0, 1, DiagnosticSeverity::WARNING),
                diagnostic(0, 1, 2, DiagnosticSeverity::HINT),
                diagnostic(1, 1, 3, DiagnosticSeverity::ERROR),
            ],
        );
        let seen: Vec<_> = found
            .iter()
            .map(|p| (p.range.start.get(), p.role, p.message.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                (5, UiRole::Error, "problem on 1"),
                (8, UiRole::Warning, "problem on 2")
            ]
        );
    }

    #[test]
    fn navigation_wraps_in_both_directions() {
        let buffer = Buffer::new("aaa\nbbb\nccc\n");
        let found = problems(
            &buffer,
            &[
                diagnostic(1, 0, 1, DiagnosticSeverity::ERROR),
                diagnostic(2, 0, 1, DiagnosticSeverity::ERROR),
            ],
        );
        let at = |offset| ByteOffset(offset);
        assert_eq!(next_problem(&found, at(0), true), Some(at(4)));
        assert_eq!(next_problem(&found, at(4), true), Some(at(8)));
        assert_eq!(next_problem(&found, at(9), true), Some(at(4)));
        assert_eq!(next_problem(&found, at(8), false), Some(at(4)));
        assert_eq!(next_problem(&found, at(2), false), Some(at(8)));
        assert_eq!(next_problem(&[], at(2), false), None);
    }
}
