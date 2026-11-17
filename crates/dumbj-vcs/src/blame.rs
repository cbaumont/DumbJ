use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::git::run_raw;
use crate::revision::check_revision;
use crate::{Repository, VcsError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub commit: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
}

impl Repository {
    /// Empty when the revision or the file does not exist there. Polls `cancelled`; `Ok(None)` when
    /// cancelled.
    pub fn blame(
        &self,
        revision: &str,
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<BlameLine>>, VcsError> {
        check_revision(revision)?;
        let Some(relative) = self.relative(path) else {
            return Ok(Some(Vec::new()));
        };
        let Some(relative) = relative.to_str() else {
            return Ok(Some(Vec::new()));
        };
        let Some(id) = self.resolve(revision)? else {
            return Ok(Some(Vec::new()));
        };
        let spec = format!("{id}:{relative}");
        let exists = run_raw(&self.workdir, &["cat-file", "-e", &spec], cancelled)?;
        match exists {
            None => return Ok(None),
            Some(out) if !out.success => return Ok(Some(Vec::new())),
            Some(_) => {}
        }
        let args = ["blame", "--porcelain", &id, "--", relative];
        let Some(out) = run_raw(&self.workdir, &args, cancelled)? else {
            return Ok(None);
        };
        if !out.success {
            return Err(out.failure());
        }
        Ok(Some(parse_porcelain(&out.stdout)))
    }
}

#[derive(Default)]
struct Meta {
    author: String,
    time: i64,
    summary: String,
}

fn group_commit(line: &str) -> Option<&str> {
    let mut fields = line.split(' ');
    let id = fields.next()?;
    let hex = id.len() >= 40 && id.bytes().all(|b| b.is_ascii_hexdigit());
    (hex && fields.next().is_some()).then_some(id)
}

/// Parses `git blame --porcelain` output. Metadata appears only the first time a commit
/// does, so it is remembered per commit.
pub(crate) fn parse_porcelain(raw: &[u8]) -> Vec<BlameLine> {
    let text = String::from_utf8_lossy(raw);
    let mut metas: HashMap<&str, Meta> = HashMap::new();
    let mut current: Option<&str> = None;
    let mut lines = Vec::new();
    for line in text.split('\n') {
        if line.starts_with('\t') {
            if let Some(id) = current {
                let meta = metas.entry(id).or_default();
                lines.push(BlameLine {
                    commit: id.to_owned(),
                    author: meta.author.clone(),
                    time: meta.time,
                    summary: meta.summary.clone(),
                });
            }
        } else if let Some(id) = group_commit(line) {
            current = Some(id);
            metas.entry(id).or_default();
        } else if let Some(meta) = current.and_then(|id| metas.get_mut(id)) {
            if let Some(author) = line.strip_prefix("author ") {
                author.clone_into(&mut meta.author);
            } else if let Some(time) = line.strip_prefix("author-time ") {
                meta.time = time.trim().parse().unwrap_or(0);
            } else if let Some(summary) = line.strip_prefix("summary ") {
                summary.clone_into(&mut meta.summary);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn reuses_metadata_of_a_commit_seen_before() {
        let raw = format!(
            "{A} 1 1 1\nauthor Ann\nauthor-mail <a@x>\nauthor-time 100\nsummary first\nfilename f\n\tone\n\
             {B} 1 2 1\nauthor Bob\nauthor-time 200\nsummary second: fix\nfilename f\n\ttwo\n\
             {A} 2 3 1\n\tthree\n"
        );
        let got = parse_porcelain(raw.as_bytes());
        let line = |c: &str, a: &str, t, s: &str| BlameLine {
            commit: c.to_owned(),
            author: a.to_owned(),
            time: t,
            summary: s.to_owned(),
        };
        assert_eq!(
            got,
            [
                line(A, "Ann", 100, "first"),
                line(B, "Bob", 200, "second: fix"),
                line(A, "Ann", 100, "first"),
            ]
        );
    }

    #[test]
    fn content_that_looks_like_a_group_header_is_still_content() {
        let raw = format!("{A} 1 1 1\nauthor Ann\n\t{B} 1 1 1\n");
        assert_eq!(parse_porcelain(raw.as_bytes()).len(), 1);
    }

    #[test]
    fn empty_output() {
        assert_eq!(parse_porcelain(b""), []);
    }
}
