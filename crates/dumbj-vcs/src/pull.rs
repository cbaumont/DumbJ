use std::sync::atomic::AtomicBool;

use crate::git::{self, run_raw_env};
use crate::{Repository, VcsError};

const NAMESPACE: &str = "refs/dumbj/pr/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// The commits a fetched pull request is reviewed between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRefs {
    /// `refs/dumbj/pr/<n>/base`: the tip of the branch the pull request targets.
    pub base: String,
    /// `refs/dumbj/pr/<n>/head`.
    pub head: String,
}

#[must_use]
pub fn pull_base_ref(number: u64) -> String {
    format!("{NAMESPACE}{number}/base")
}

#[must_use]
pub fn pull_head_ref(number: u64) -> String {
    format!("{NAMESPACE}{number}/head")
}

fn check_name(name: &str) -> Result<(), VcsError> {
    let bad = |c: char| {
        c.is_whitespace() || c.is_control() || matches!(c, ':' | '^' | '~' | '?' | '*' | '[' | '\\')
    };
    if name.is_empty() || name.starts_with('-') || name.contains("..") || name.contains(bad) {
        return Err(VcsError::Git(format!("invalid name {name:?}")));
    }
    Ok(())
}

impl Repository {
    /// The configured remotes with their fetch URLs, sorted by name.
    pub fn remotes(&self) -> Result<Vec<Remote>, VcsError> {
        let args = ["config", "--get-regexp", r"^remote\..*\.url$"];
        let Some(out) = git::run_raw(&self.workdir, &args, &git::never())? else {
            return Ok(Vec::new());
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut remotes: Vec<Remote> = text
            .lines()
            .filter_map(|line| {
                let (key, url) = line.split_once(' ')?;
                let name = key.strip_prefix("remote.")?.strip_suffix(".url")?;
                Some(Remote {
                    name: name.to_owned(),
                    url: url.to_owned(),
                })
            })
            .collect();
        remotes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(remotes)
    }

    /// Fetches pull request `number` of `remote` into `refs/dumbj/pr/<number>/head` and the
    /// tip of `base_branch` into `refs/dumbj/pr/<number>/base`. Only these refs and their
    /// objects are written. Never prompts for credentials. Polls `cancelled`; `Ok(None)`
    /// when cancelled.
    pub fn fetch_pull(
        &self,
        remote: &str,
        number: u64,
        base_branch: &str,
        cancelled: &AtomicBool,
    ) -> Result<Option<PullRefs>, VcsError> {
        check_name(remote)?;
        check_name(base_branch)?;
        let head = format!("+refs/pull/{number}/head:{}", pull_head_ref(number));
        let base = format!("+refs/heads/{base_branch}:{}", pull_base_ref(number));
        let args = [
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            remote,
            &head,
            &base,
        ];
        let env = [("GIT_TERMINAL_PROMPT", "0")];
        match run_raw_env(&self.workdir, &args, &env, cancelled)? {
            None => return Ok(None),
            Some(out) if !out.success => {
                return Err(VcsError::Git(
                    String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                ));
            }
            Some(_) => {}
        }
        match self.pull_refs(number)? {
            Some(refs) => Ok(Some(refs)),
            None => Err(VcsError::Git(format!(
                "pull request {number} was not fetched"
            ))),
        }
    }

    /// The commits of a pull request fetched earlier; `Ok(None)` when it was not.
    pub fn pull_refs(&self, number: u64) -> Result<Option<PullRefs>, VcsError> {
        let base = self.resolve(&pull_base_ref(number))?;
        let head = self.resolve(&pull_head_ref(number))?;
        Ok(base.zip(head).map(|(base, head)| PullRefs { base, head }))
    }

    /// Deletes every ref under `refs/dumbj/pr/`, except those of pull request `keep`, and
    /// returns how many were deleted. The objects go with git's next garbage collection.
    pub fn prune_pull_refs(&self, keep: Option<u64>) -> Result<usize, VcsError> {
        let kept = keep.map(|number| format!("{NAMESPACE}{number}/"));
        let never = git::never();
        let args = ["for-each-ref", "--format=%(refname)", NAMESPACE];
        let out = git::run(&self.workdir, &args, &never)?.unwrap_or_default();
        let text = String::from_utf8_lossy(&out);
        let mut pruned = 0;
        let pruned_refs = text.lines().filter(|name| {
            name.starts_with(NAMESPACE) && !kept.as_ref().is_some_and(|kept| name.starts_with(kept))
        });
        for name in pruned_refs {
            git::run(&self.workdir, &["update-ref", "-d", name], &never)?;
            pruned += 1;
        }
        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_git_would_misread_are_refused() {
        for name in ["", "-x", "a b", "a:b", "a..b", "a^", "x*"] {
            assert!(check_name(name).is_err(), "{name:?}");
        }
        for name in ["origin", "main", "release/1.2", "feature-x_y"] {
            assert!(check_name(name).is_ok(), "{name:?}");
        }
    }
}
