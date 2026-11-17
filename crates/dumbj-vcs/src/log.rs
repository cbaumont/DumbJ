use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::git;
use crate::revision::check_revision;
use crate::{Repository, VcsError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full hex id.
    pub id: String,
    /// Full hex ids.
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
    /// Branch and tag names pointing at the commit, in git's order.
    pub refs: Vec<String>,
    /// The message after the subject line, trimmed; may be empty.
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogOptions {
    /// Where to start; `None` is HEAD.
    pub revision: Option<String>,
    /// Only commits touching this file or directory (absolute or relative to the workdir),
    /// following renames for a single file.
    pub path: Option<PathBuf>,
    /// Only commits whose author name or email contains this (case-insensitive).
    pub author: Option<String>,
    /// Only commits whose message contains this (case-insensitive, fixed string).
    pub text: Option<String>,
    /// Only the history of these lines of `path` (zero-based, end exclusive), as
    /// `git log -L` gives it.
    pub lines: Option<Range<usize>>,
    /// Only commits committed after this date, in git's date syntax.
    pub since: Option<String>,
    /// Only commits committed before this date, in git's date syntax.
    pub until: Option<String>,
    pub skip: usize,
    /// At most this many commits; 0 means no limit.
    pub limit: usize,
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D%x1f%b";

impl Repository {
    /// Commits newest first, in `git log` order. Polls `cancelled`; `Ok(None)` when
    /// cancelled.
    pub fn log(
        &self,
        options: &LogOptions,
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<Commit>>, VcsError> {
        let revision = options.revision.as_deref().unwrap_or("HEAD");
        check_revision(revision)?;
        if options.revision.is_none() && self.head_id()?.is_none() {
            return Ok(Some(Vec::new()));
        }
        let path = match &options.path {
            Some(path) => {
                let relative = self.relative(path).ok_or_else(|| {
                    VcsError::Git(format!("{} is outside the repository", path.display()))
                })?;
                let text = relative
                    .to_str()
                    .ok_or_else(|| VcsError::Git("path is not valid UTF-8".to_owned()))?
                    .to_owned();
                let is_dir = self.workdir.join(&relative).is_dir();
                Some((text, is_dir))
            }
            None => None,
        };

        let mut args: Vec<String> = [
            "log",
            "-z",
            "--no-color",
            "--encoding=UTF-8",
            "--no-ext-diff",
            FORMAT,
        ]
        .map(str::to_owned)
        .into();
        if options.skip > 0 {
            args.push(format!("--skip={}", options.skip));
        }
        if options.limit > 0 {
            args.push(format!("--max-count={}", options.limit));
        }
        if options.author.is_some() || options.text.is_some() {
            args.push("--regexp-ignore-case".to_owned());
            args.push("--fixed-strings".to_owned());
        }
        for (flag, value) in [("since", &options.since), ("until", &options.until)] {
            if let Some(value) = value {
                if value.starts_with('-') {
                    return Err(VcsError::Git(format!("invalid date {value:?}")));
                }
                args.push(format!("--{flag}={value}"));
            }
        }
        if let Some(author) = &options.author {
            args.push(format!("--author={author}"));
        }
        if let Some(text) = &options.text {
            args.push(format!("--grep={text}"));
        }
        match (&path, &options.lines) {
            (Some((file, _)), Some(lines)) => {
                if lines.is_empty() {
                    return Ok(Some(Vec::new()));
                }
                args.push("--no-patch".to_owned());
                args.push(format!("-L{},{}:{file}", lines.start + 1, lines.end));
                args.push(revision.to_owned());
            }
            (Some((file, is_dir)), None) => {
                if !is_dir {
                    args.push("--follow".to_owned());
                }
                args.push(revision.to_owned());
                args.push("--".to_owned());
                args.push(file.clone());
            }
            _ => {
                args.push(revision.to_owned());
                args.push("--".to_owned());
            }
        }

        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(git::run(&self.workdir, &args, cancelled)?.map(|out| parse_log(&out)))
    }

    pub fn commit(&self, revision: &str) -> Result<Option<Commit>, VcsError> {
        let Some(id) = self.resolve(revision)? else {
            return Ok(None);
        };
        let options = LogOptions {
            revision: Some(id),
            limit: 1,
            ..LogOptions::default()
        };
        let commits = self.log(&options, &git::never())?;
        Ok(commits.and_then(|commits| commits.into_iter().next()))
    }
}

/// Parses `git log -z` output in the `FORMAT` layout: commits separated by NUL, fields by
/// the unit separator (0x1f). A trailing field separator inside a message stays in the body.
pub(crate) fn parse_log(raw: &[u8]) -> Vec<Commit> {
    let mut out = Vec::new();
    for record in raw.split(|&b| b == 0) {
        let text = String::from_utf8_lossy(record);
        let text = text.trim_start_matches('\n');
        let mut fields = text.splitn(8, '\x1f');
        let (Some(id), Some(parents), Some(author), Some(email), Some(time), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let Ok(time) = time.parse() else {
            continue;
        };
        let refs = fields.next().map_or_default(parse_refs);
        out.push(Commit {
            id: id.to_owned(),
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            author: author.to_owned(),
            email: email.to_owned(),
            time,
            subject: subject.to_owned(),
            refs,
            body: fields.next().unwrap_or_default().trim().to_owned(),
        });
    }
    out
}

fn parse_refs(decoration: &str) -> Vec<String> {
    decoration
        .split(", ")
        .filter_map(|entry| {
            let name = entry.strip_prefix("HEAD -> ").unwrap_or(entry);
            let name = name.strip_prefix("tag: ").unwrap_or(name);
            (!name.is_empty() && name != "HEAD" && !name.ends_with("/HEAD"))
                .then(|| name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commits_with_bodies_and_merges() {
        let raw = "aaa\x1fbbb ccc\x1fAnn\x1fa@x\x1f1700000000\x1fMerge it\x1fHEAD -> main, tag: v1, origin/HEAD, origin/main\x1f\0\
            bbb\x1f\x1fBob B\x1fb@x\x1f-5\x1fSubject\x1f\x1fFirst para.\n\nSecond line\n\n\0"
            .as_bytes();
        let commits = parse_log(raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["bbb", "ccc"]);
        assert_eq!(commits[0].body, "");
        assert_eq!(commits[0].refs, ["main", "v1", "origin/main"]);
        assert_eq!(commits[1].refs, Vec::<String>::new());
        assert_eq!(commits[0].time, 1_700_000_000);
        assert_eq!(commits[1].parents, Vec::<String>::new());
        assert_eq!(commits[1].author, "Bob B");
        assert_eq!(commits[1].time, -5);
        assert_eq!(commits[1].body, "First para.\n\nSecond line");
    }

    #[test]
    fn a_unit_separator_in_the_body_is_kept() {
        let raw = "a\x1f\x1fn\x1fe\x1f1\x1fs\x1f\x1fx\x1fy\0".as_bytes();
        assert_eq!(parse_log(raw)[0].body, "x\x1fy");
    }

    #[test]
    fn empty_and_malformed_input() {
        assert_eq!(parse_log(b""), []);
        assert_eq!(parse_log(b"\0\0"), []);
        assert_eq!(parse_log(b"only one field\0"), []);
        assert_eq!(parse_log(b"a\x1f\x1fn\x1fe\x1fnot-a-number\x1fs\x1f\0"), []);
    }
}
