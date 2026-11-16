use std::path::Path;

use crate::git::{self, run_raw};
use crate::{Repository, VcsError, git_err};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote-tracking branch.
    pub name: String,
    pub remote: bool,
    /// Whether HEAD is on this branch.
    pub current: bool,
    /// Full hex id of the tip commit.
    pub tip: String,
}

/// Revisions that start with `-` would be read as options by the CLI.
pub(crate) fn check_revision(revision: &str) -> Result<(), VcsError> {
    if revision.is_empty() || revision.starts_with('-') {
        return Err(VcsError::Git(format!("invalid revision {revision:?}")));
    }
    Ok(())
}

impl Repository {
    /// The full hex id of the commit `revision` names (a branch, tag, hex prefix, `HEAD~2`,
    /// ...); `Ok(None)` when it names nothing.
    pub fn resolve(&self, revision: &str) -> Result<Option<String>, VcsError> {
        if check_revision(revision).is_err() {
            return Ok(None);
        }
        let spec = format!("{revision}^{{commit}}");
        let args = ["rev-parse", "--verify", "--quiet", &spec];
        let Some(out) = run_raw(&self.workdir, &args, &git::never())? else {
            return Ok(None);
        };
        if !out.success {
            return Ok(None);
        }
        Ok(parse_id(&out.stdout))
    }

    /// The content of `path` (absolute or relative to the workdir) in `revision`. `Ok(None)`
    /// when the revision or the file does not exist there, or the file is not valid UTF-8.
    pub fn text_at(&self, revision: &str, path: &Path) -> Result<Option<String>, VcsError> {
        let Some(relative) = self.relative(path) else {
            return Ok(None);
        };
        if revision == "HEAD" {
            return self.head_blob_text(&relative);
        }
        let Some(relative) = relative.to_str() else {
            return Ok(None);
        };
        let Some(id) = self.resolve(revision)? else {
            return Ok(None);
        };
        let spec = format!("{id}:{relative}");
        let args = ["cat-file", "blob", &spec];
        match run_raw(&self.workdir, &args, &git::never())? {
            Some(out) if out.success => Ok(String::from_utf8(out.stdout).ok()),
            _ => Ok(None),
        }
    }

    /// The best common ancestor of two revisions, as a full hex id; `Ok(None)` when there is
    /// none.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>, VcsError> {
        let (Some(a), Some(b)) = (self.resolve(a)?, self.resolve(b)?) else {
            return Ok(None);
        };
        let args = ["merge-base", &a, &b];
        match run_raw(&self.workdir, &args, &git::never())? {
            Some(out) if out.success => Ok(parse_id(&out.stdout)),
            _ => Ok(None),
        }
    }

    /// Local and remote-tracking branches, local first, each group sorted by name. `HEAD`
    /// pseudo-refs of remotes (`origin/HEAD`) are left out.
    pub fn branches(&self) -> Result<Vec<Branch>, VcsError> {
        let args = [
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ];
        let out = git::run(&self.workdir, &args, &git::never())?.unwrap_or_default();
        Ok(parse_branches(&out))
    }

    fn head_blob_text(&self, relative: &Path) -> Result<Option<String>, VcsError> {
        let repo = self.open()?;
        let Ok(commit) = repo.head_commit() else {
            return Ok(None);
        };
        let tree = commit.tree().map_err(git_err)?;
        let Some(entry) = tree.lookup_entry_by_path(relative).map_err(git_err)? else {
            return Ok(None);
        };
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = entry.object().map_err(git_err)?;
        Ok(String::from_utf8(object.detach().data).ok())
    }
}

fn parse_id(raw: &[u8]) -> Option<String> {
    let id = std::str::from_utf8(raw).ok()?.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// Parses `for-each-ref` output of `refname NUL objectname NUL HEAD-marker` lines.
pub(crate) fn parse_branches(raw: &[u8]) -> Vec<Branch> {
    let text = String::from_utf8_lossy(raw);
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\0');
        let (Some(refname), Some(tip), Some(marker)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            local.push(Branch {
                name: name.to_owned(),
                remote: false,
                current: marker == "*",
                tip: tip.to_owned(),
            });
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") || !name.contains('/') {
                continue;
            }
            remote.push(Branch {
                name: name.to_owned(),
                remote: true,
                current: false,
                tip: tip.to_owned(),
            });
        }
    }
    local.sort_by(|a, b| a.name.cmp(&b.name));
    remote.sort_by(|a, b| a.name.cmp(&b.name));
    local.extend(remote);
    local
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote_branches() {
        let raw = b"refs/heads/zeta\0aaa\0 \n\
            refs/heads/main\0bbb\0*\n\
            refs/remotes/origin/HEAD\0bbb\0 \n\
            refs/remotes/origin/main\0ccc\0 \n\
            refs/remotes/origin\0ddd\0 \n\
            refs/heads/feature/x\0eee\0 \n\
            refs/tags/v1\0fff\0 \n\
            garbage\n";
        let names: Vec<_> = parse_branches(raw)
            .into_iter()
            .map(|b| (b.name, b.remote, b.current, b.tip))
            .collect();
        let expected = [
            ("feature/x", false, false, "eee"),
            ("main", false, true, "bbb"),
            ("zeta", false, false, "aaa"),
            ("origin/main", true, false, "ccc"),
        ]
        .map(|(n, r, c, t)| (n.to_owned(), r, c, t.to_owned()));
        assert_eq!(names, expected);
    }

    #[test]
    fn empty_branch_output() {
        assert_eq!(parse_branches(b""), []);
    }

    #[test]
    fn dash_revisions_are_rejected() {
        assert!(check_revision("--output=x").is_err());
        assert!(check_revision("").is_err());
        assert!(check_revision("main").is_ok());
    }
}
