//! The pull request's overview page, permalinks and pruning the fetched refs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use dumbj_review::{CheckState, PrState, PullRequestDetails, RepoId, ReviewDecision};
use dumbj_vcs::split_revision_path;
use gpui::{AppContext, ClipboardItem, Context, Window};

use super::pulls::connect;
use super::{Tab, Workspace};
use crate::editor::EditorView;

fn overview_text(pull: &PullRequestDetails) -> String {
    let state = match pull.state {
        PrState::Open if pull.is_draft => "draft",
        PrState::Open => "open",
        PrState::Closed => "closed",
        PrState::Merged => "merged",
    };
    let decision = match pull.review_decision {
        Some(ReviewDecision::Approved) => " · approved",
        Some(ReviewDecision::ChangesRequested) => " · changes requested",
        Some(ReviewDecision::ReviewRequired) => " · review required",
        None => "",
    };
    let mut text = format!(
        "# #{} {}\n\n{} wants to merge `{}` into `{}` · {state}{decision}\n{}\n\n## Checks\n\n",
        pull.number, pull.title, pull.author, pull.head_ref, pull.base_ref, pull.url
    );
    if pull.checks.is_empty() {
        text.push_str("No checks.\n");
    }
    for check in &pull.checks {
        let mark = match check.state {
            CheckState::Success => "✓",
            CheckState::Failure => "✗",
            CheckState::Pending => "…",
            CheckState::Neutral => "–",
        };
        let _ = writeln!(text, "- {mark} {}", check.name);
    }
    text.push_str("\n## Commits\n\n");
    for commit in &pull.commits {
        let (short, day): (String, String) = (
            commit.oid.chars().take(7).collect(),
            commit.date.chars().take(10).collect(),
        );
        let _ = writeln!(
            text,
            "- `{short}` {} ({}, {day})",
            commit.headline, commit.author
        );
    }
    text.push_str("\n## Description\n\n");
    let body = pull.body.trim();
    text.push_str(if body.is_empty() {
        "No description."
    } else {
        body
    });
    text.push('\n');
    text
}

/// The address of lines of a file at a commit on a GitHub host; lines are one-based.
fn permalink(repo: &RepoId, commit: &str, path: &Path, first: usize, last: usize) -> String {
    let path = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let lines = if last > first {
        format!("#L{first}-L{last}")
    } else {
        format!("#L{first}")
    };
    format!(
        "https://{}/{}/{}/blob/{commit}/{path}{lines}",
        repo.host, repo.owner, repo.name
    )
}

impl Workspace {
    fn pull_details(&self) -> Option<&PullRequestDetails> {
        self.session.pull_threads.as_ref()?.details.as_ref()
    }

    /// Opens, or refreshes, the page with the pull request's state, checks, commits and
    /// description.
    pub(super) fn open_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(details) = self.pull_details() else {
            self.status_message = Some("The pull request is not loaded yet".to_owned());
            return;
        };
        let text = overview_text(details);
        let path = PathBuf::from(format!("#{} overview.md", details.number));
        let title = format!("#{} overview", details.number);
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.path == path)
            .unwrap_or_else(|| {
                let editor = cx.new(|cx| EditorView::scratch("overview.md", cx));
                self.subscriptions
                    .push(cx.observe(&editor, |_, _, cx| cx.notify()));
                self.tabs.push(Tab {
                    path,
                    title,
                    editor,
                    diff: None,
                });
                self.tabs.len() - 1
            });
        if let Some(tab) = self.tabs.get(index) {
            tab.editor.update(cx, |editor, cx| {
                editor.set_scratch_text(&text, cx);
                editor.set_read_only(true);
                editor.go_to(1, None, cx);
            });
        }
        self.activate_tab(index, window, cx);
    }

    /// Copies the GitHub address of the caret line, or the selected lines, of the active
    /// file: at the commit a revision tab shows, otherwise at HEAD.
    pub(super) fn copy_permalink(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(editor)) = (self.repository.clone(), self.active_editor())
        else {
            return;
        };
        let editor = editor.read(cx);
        let (first, last) = editor.selected_lines();
        let (commit, relative) = match split_revision_path(editor.path()) {
            Some((_, commit, relative)) => (Some(commit), relative),
            None => match editor.path().strip_prefix(repository.workdir()) {
                Ok(relative) => (None, relative.to_path_buf()),
                Err(_) => return,
            },
        };
        let known = self.session.github.clone();
        cx.spawn(async move |this, cx| {
            let link = cx
                .background_spawn(async move {
                    let commit = match commit {
                        Some(commit) => commit,
                        None => repository
                            .head_id()
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| "The repository has no commits".to_owned())?,
                    };
                    let github = match known {
                        Some(github) => github,
                        None => std::sync::Arc::new(connect(&repository)?),
                    };
                    let link = permalink(github.repo(), &commit, &relative, first, last);
                    Ok::<_, String>((github, link))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status_message = Some(match link {
                    Ok((github, link)) => {
                        this.session.github = Some(github);
                        cx.write_to_clipboard(ClipboardItem::new_string(link));
                        "Permalink copied".to_owned()
                    }
                    Err(message) => message,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Deletes the refs of fetched pull requests, except the one under review.
    pub(super) fn prune_review_refs(&mut self, cx: &mut Context<Self>) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        let keep = self.review_pull().map(|pull| pull.number);
        cx.spawn(async move |this, cx| {
            let pruned = cx
                .background_spawn(async move { repository.prune_pull_refs(keep) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status_message = Some(match pruned {
                    Ok(count) => format!("Pruned {count} review refs"),
                    Err(error) => format!("Cannot prune the review refs: {error}"),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use dumbj_review::{Check, PullCommit};

    use super::*;

    fn details() -> PullRequestDetails {
        PullRequestDetails {
            id: "PR_1".to_owned(),
            number: 7,
            title: "Add a summary".to_owned(),
            body: "Adds it.\n\nSee #3.\n".to_owned(),
            url: "https://github.com/o/r/pull/7".to_owned(),
            author: "ana".to_owned(),
            state: PrState::Open,
            is_draft: false,
            head_ref: "topic".to_owned(),
            base_ref: "main".to_owned(),
            head_oid: "h".repeat(40),
            base_oid: "b".repeat(40),
            review_decision: Some(ReviewDecision::ChangesRequested),
            pending_review: None,
            checks: vec![
                Check {
                    name: "build".to_owned(),
                    state: CheckState::Success,
                    url: None,
                },
                Check {
                    name: "lint".to_owned(),
                    state: CheckState::Failure,
                    url: None,
                },
            ],
            commits: vec![PullCommit {
                oid: "0123456789abcdef".to_owned(),
                headline: "Add it".to_owned(),
                author: "ana".to_owned(),
                date: "2026-11-16T10:00:00Z".to_owned(),
            }],
        }
    }

    #[test]
    fn the_overview_lists_state_checks_commits_and_description() {
        assert_eq!(
            overview_text(&details()),
            "# #7 Add a summary\n\n\
             ana wants to merge `topic` into `main` · open · changes requested\n\
             https://github.com/o/r/pull/7\n\n\
             ## Checks\n\n- ✓ build\n- ✗ lint\n\n\
             ## Commits\n\n- `0123456` Add it (ana, 2026-11-16)\n\n\
             ## Description\n\nAdds it.\n\nSee #3.\n"
        );
    }

    #[test]
    fn an_empty_pull_request_says_so() {
        let mut pull = details();
        pull.checks.clear();
        pull.body = "  \n".to_owned();
        pull.is_draft = true;
        pull.review_decision = None;
        let text = overview_text(&pull);
        assert!(text.contains("· draft\n"));
        assert!(text.contains("## Checks\n\nNo checks.\n"));
        assert!(text.ends_with("## Description\n\nNo description.\n"));
    }

    #[test]
    fn permalinks_point_at_lines_of_a_commit() {
        let repo = RepoId {
            host: "github.com".to_owned(),
            owner: "o".to_owned(),
            name: "r".to_owned(),
        };
        assert_eq!(
            permalink(&repo, "abc", Path::new("src/A.kt"), 4, 4),
            "https://github.com/o/r/blob/abc/src/A.kt#L4"
        );
        assert_eq!(
            permalink(&repo, "abc", Path::new("src/A.kt"), 4, 9),
            "https://github.com/o/r/blob/abc/src/A.kt#L4-L9"
        );
    }
}
