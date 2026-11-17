use std::sync::atomic::AtomicBool;

use crate::git::{self, run_raw, run_raw_env};
use crate::revision::check_revision;
use crate::{Branch, Repository, VcsError};

impl Repository {
    /// A remote-tracking branch (`origin/topic`) checks
    /// out the local branch of the same name, creating it to track the remote one when it
    /// does not exist. Fails, leaving everything as it was, when local changes would be
    /// overwritten; the error carries git's message.
    pub fn checkout(&self, branch: &Branch) -> Result<(), VcsError> {
        check_revision(&branch.name)?;
        let never = git::never();
        let args: Vec<&str> = if branch.remote {
            let Some((_, local)) = branch.name.split_once('/') else {
                return Err(VcsError::Git(format!("invalid branch {:?}", branch.name)));
            };
            check_revision(local)?;
            let exists = self
                .branches()?
                .iter()
                .any(|b| !b.remote && b.name == local);
            if exists {
                vec!["switch", local]
            } else {
                vec!["switch", "--track", &branch.name]
            }
        } else {
            vec!["switch", &branch.name]
        };
        match run_raw(&self.workdir, &args, &never)? {
            Some(out) if !out.success => Err(out.failure()),
            _ => Ok(()),
        }
    }

    /// Never prompts for credentials. Polls `cancelled`; `Ok(None)` when cancelled.
    pub fn fetch(&self, cancelled: &AtomicBool) -> Result<Option<()>, VcsError> {
        let args = ["fetch", "--all", "--prune"];
        let env = [("GIT_TERMINAL_PROMPT", "0")];
        match run_raw_env(&self.workdir, &args, &env, cancelled)? {
            None => Ok(None),
            Some(out) if out.success => Ok(Some(())),
            Some(out) => Err(out.failure()),
        }
    }
}
