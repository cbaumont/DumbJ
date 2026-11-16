use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

pub(crate) struct TempRepo(pub(crate) PathBuf);

impl TempRepo {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dumbj-vcs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let repo = Self(dir);
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    pub(crate) fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub(crate) fn file(&self, relative: &str, content: impl AsRef<[u8]>) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub(crate) fn commit_all(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "c"]);
    }

    pub(crate) fn repo(&self) -> Repository {
        Repository::discover(&self.0).unwrap().unwrap()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn discover_from_a_nested_directory() {
    let t = TempRepo::new();
    t.file("a/b/c.txt", "x");
    let repo = Repository::discover(&t.0.join("a/b")).unwrap().unwrap();
    assert_eq!(
        repo.workdir().canonicalize().unwrap(),
        t.0.canonicalize().unwrap()
    );
}

#[test]
fn discover_outside_any_repository() {
    let dir = std::env::temp_dir().join(format!("dumbj-vcs-none-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let result = Repository::discover(&dir);
    let _ = fs::remove_dir_all(&dir);
    if let Some(repo) = result.unwrap() {
        panic!("unexpected repository at {}", repo.workdir().display());
    }
}

#[test]
fn branch_and_head_id() {
    let t = TempRepo::new();
    let repo = t.repo();
    assert_eq!(repo.branch().unwrap().as_deref(), Some("main"));
    assert_eq!(repo.head_id().unwrap(), None);
    t.file("a.txt", "1\n");
    t.commit_all();
    let id = repo.head_id().unwrap().unwrap();
    assert_eq!(id.len(), 40);
    t.git(&["checkout", "-q", "-b", "feature/x"]);
    assert_eq!(repo.branch().unwrap().as_deref(), Some("feature/x"));
    t.git(&["checkout", "-q", "--detach"]);
    assert_eq!(repo.branch().unwrap(), None);
    assert_eq!(repo.head_id().unwrap().unwrap(), id);
}

#[test]
fn head_text_of_tracked_files() {
    let t = TempRepo::new();
    t.file("a.txt", "one\n");
    t.file("src/deep/b.txt", "two\r\n");
    t.commit_all();
    t.file("a.txt", "changed\n");
    let repo = t.repo();
    assert_eq!(
        repo.head_text(Path::new("a.txt")).unwrap().as_deref(),
        Some("one\n")
    );
    assert_eq!(
        repo.head_text(Path::new("src/deep/b.txt"))
            .unwrap()
            .as_deref(),
        Some("two\r\n")
    );
    assert_eq!(
        repo.head_text(&t.0.join("src/deep/b.txt"))
            .unwrap()
            .as_deref(),
        Some("two\r\n")
    );
}

#[test]
fn head_text_is_none_when_there_is_nothing_to_show() {
    let t = TempRepo::new();
    let repo = t.repo();
    assert_eq!(repo.head_text(Path::new("a.txt")).unwrap(), None);
    t.file("a.txt", "one\n");
    t.file("bin.dat", [0xff, 0xfe, 0x00, 0x80]);
    t.commit_all();
    t.file("new.txt", "x");
    assert_eq!(repo.head_text(Path::new("new.txt")).unwrap(), None);
    assert_eq!(repo.head_text(Path::new("bin.dat")).unwrap(), None);
    assert_eq!(repo.head_text(Path::new("missing/x")).unwrap(), None);
    assert_eq!(
        repo.head_text(Path::new("/definitely/elsewhere")).unwrap(),
        None
    );
    assert_eq!(repo.head_text(Path::new("")).unwrap(), None);
}

impl TempRepo {
    pub(crate) fn commit_msg(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    pub(crate) fn rev(&self, revision: &str) -> String {
        self.repo().resolve(revision).unwrap().unwrap()
    }
}

#[test]
fn resolve_names_and_garbage() {
    let t = TempRepo::new();
    let repo = t.repo();
    assert_eq!(repo.resolve("HEAD").unwrap(), None);
    t.file("a.txt", "1\n");
    t.commit_msg("one");
    let first = repo.head_id().unwrap().unwrap();
    t.file("a.txt", "2\n");
    t.commit_msg("two");
    let second = repo.head_id().unwrap().unwrap();
    t.git(&["tag", "v1", &first]);
    assert_eq!(repo.resolve("main").unwrap().as_deref(), Some(&*second));
    assert_eq!(repo.resolve("HEAD~1").unwrap().as_deref(), Some(&*first));
    assert_eq!(repo.resolve("v1").unwrap().as_deref(), Some(&*first));
    assert_eq!(repo.resolve(&first[..8]).unwrap().as_deref(), Some(&*first));
    assert_eq!(repo.resolve("no-such-thing").unwrap(), None);
    assert_eq!(repo.resolve("HEAD~9").unwrap(), None);
    assert_eq!(repo.resolve("--help").unwrap(), None);
    assert_eq!(repo.resolve("").unwrap(), None);
}

#[test]
fn text_at_older_revisions() {
    let t = TempRepo::new();
    t.file("a.txt", "old\n");
    t.file("gone.txt", "bye\n");
    t.file("bin.dat", [0xff, 0xfe]);
    t.commit_msg("one");
    t.file("a.txt", "new\n");
    fs::remove_file(t.0.join("gone.txt")).unwrap();
    t.commit_msg("two");
    let repo = t.repo();
    let a = Path::new("a.txt");
    assert_eq!(repo.text_at("HEAD~1", a).unwrap().as_deref(), Some("old\n"));
    assert_eq!(repo.text_at("HEAD", a).unwrap().as_deref(), Some("new\n"));
    assert_eq!(
        repo.text_at("main", &t.0.join("a.txt")).unwrap().as_deref(),
        Some("new\n")
    );
    let gone = Path::new("gone.txt");
    assert_eq!(repo.text_at("HEAD", gone).unwrap(), None);
    assert_eq!(
        repo.text_at("HEAD~1", gone).unwrap().as_deref(),
        Some("bye\n")
    );
    assert_eq!(repo.text_at("nope", a).unwrap(), None);
    assert_eq!(repo.text_at("HEAD~1", Path::new("bin.dat")).unwrap(), None);
    assert_eq!(repo.text_at("HEAD~1", Path::new("")).unwrap(), None);
    assert_eq!(repo.text_at("HEAD~1", Path::new("missing")).unwrap(), None);
}

#[test]
fn merge_base_of_diverged_and_unrelated_histories() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    t.commit_msg("root");
    let root = t.rev("HEAD");
    t.git(&["checkout", "-q", "-b", "side"]);
    t.file("s.txt", "s\n");
    t.commit_msg("side");
    t.git(&["checkout", "-q", "main"]);
    t.file("m.txt", "m\n");
    t.commit_msg("main");
    let repo = t.repo();
    assert_eq!(
        repo.merge_base("main", "side").unwrap().as_deref(),
        Some(&*root)
    );
    assert_eq!(repo.merge_base("main", "nope").unwrap(), None);

    t.git(&["checkout", "-q", "--orphan", "other"]);
    t.git(&["rm", "-rfq", "."]);
    t.file("o.txt", "o\n");
    t.commit_msg("orphan");
    assert_eq!(repo.merge_base("main", "other").unwrap(), None);
}

#[test]
fn branches_with_remote_tracking_refs() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    t.commit_msg("one");
    let id = t.rev("HEAD");
    t.git(&["branch", "alpha"]);
    t.git(&["update-ref", "refs/remotes/origin/x", &id]);
    t.git(&["update-ref", "refs/remotes/origin/main", &id]);
    t.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let got: Vec<_> = t
        .repo()
        .branches()
        .unwrap()
        .into_iter()
        .map(|b| (b.name, b.remote, b.current, b.tip == id))
        .collect();
    let expected = [
        ("alpha", false, false),
        ("main", false, true),
        ("origin/main", true, false),
        ("origin/x", true, false),
    ]
    .map(|(n, r, c)| (n.to_owned(), r, c, true));
    assert_eq!(got, expected);
    t.git(&["checkout", "-q", "alpha"]);
    let current: Vec<_> = t
        .repo()
        .branches()
        .unwrap()
        .into_iter()
        .filter(|b| b.current)
        .map(|b| b.name)
        .collect();
    assert_eq!(current, ["alpha"]);
}

#[test]
fn branches_of_an_empty_repository() {
    assert_eq!(TempRepo::new().repo().branches().unwrap(), []);
}

fn change(path: &str, old: Option<&str>, status: FileStatus) -> Change {
    Change {
        path: path.into(),
        old_path: old.map(Into::into),
        status,
    }
}

#[test]
fn changed_files_between_commits() {
    let t = TempRepo::new();
    t.file("keep.txt", "keep\n");
    t.file("edit.txt", "one\n");
    t.file("remove.txt", "bye\n");
    t.file(
        "old.txt",
        "some content that is long enough to be detected as a rename\n",
    );
    t.commit_msg("base");
    let base = t.rev("HEAD");
    t.file("edit.txt", "two\n");
    fs::remove_file(t.0.join("remove.txt")).unwrap();
    fs::create_dir(t.0.join("dir")).unwrap();
    t.git(&["mv", "old.txt", "dir/moved.txt"]);
    t.file("added.txt", "new\n");
    t.commit_msg("change");
    let repo = t.repo();
    let never = AtomicBool::new(false);
    let got = repo
        .changed_files(&base, Some("HEAD"), &never)
        .unwrap()
        .unwrap();
    assert_eq!(
        got,
        [
            change("added.txt", None, FileStatus::Added),
            change("dir/moved.txt", Some("old.txt"), FileStatus::Renamed),
            change("edit.txt", None, FileStatus::Modified),
            change("remove.txt", None, FileStatus::Deleted),
        ]
    );
    assert_eq!(
        repo.changed_files("HEAD", Some("HEAD"), &never)
            .unwrap()
            .unwrap(),
        []
    );
    assert!(repo.changed_files("nope", Some("HEAD"), &never).is_err());
    assert!(repo.changed_files("--oops", None, &never).is_err());
    assert!(
        repo.changed_files(&base, Some("HEAD"), &AtomicBool::new(true))
            .unwrap()
            .is_none()
    );
}

fn subjects(commits: &[Commit]) -> Vec<&str> {
    commits.iter().map(|c| c.subject.as_str()).collect()
}

fn log(t: &TempRepo, options: &LogOptions) -> Vec<Commit> {
    t.repo()
        .log(options, &AtomicBool::new(false))
        .unwrap()
        .unwrap()
}

#[test]
fn log_of_an_empty_repository_is_empty() {
    let t = TempRepo::new();
    assert_eq!(log(&t, &LogOptions::default()), []);
    assert_eq!(t.repo().commit("HEAD").unwrap(), None);
}

#[test]
fn log_limit_skip_and_commit() {
    let t = TempRepo::new();
    for n in 1..=4 {
        t.file("a.txt", format!("{n}\n"));
        t.commit_msg(&format!("c{n}"));
    }
    let all = log(&t, &LogOptions::default());
    assert_eq!(subjects(&all), ["c4", "c3", "c2", "c1"]);
    assert_eq!(all[0].id, t.rev("HEAD"));
    assert_eq!(all[0].parents, [t.rev("HEAD~1")]);
    assert_eq!(all[3].parents, Vec::<String>::new());
    assert_eq!(
        (all[0].author.as_str(), all[0].email.as_str()),
        ("T", "t@example.com")
    );
    assert!(all[0].time > 1_600_000_000);
    let page = LogOptions {
        skip: 1,
        limit: 2,
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &page)), ["c3", "c2"]);
    let from = LogOptions {
        revision: Some("HEAD~2".to_owned()),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &from)), ["c2", "c1"]);
    let one = t.repo().commit("HEAD~1").unwrap().unwrap();
    assert_eq!(one.subject, "c3");
    assert_eq!(t.repo().commit("nope").unwrap(), None);
    let bad = LogOptions {
        revision: Some("nope".to_owned()),
        ..LogOptions::default()
    };
    assert!(t.repo().log(&bad, &AtomicBool::new(false)).is_err());
    assert!(
        t.repo()
            .log(&LogOptions::default(), &AtomicBool::new(true))
            .unwrap()
            .is_none()
    );
}

fn commit_dated(t: &TempRepo, message: &str, date: &str) {
    t.git(&["add", "-A"]);
    let out = Command::new("git")
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", message])
        .current_dir(&t.0)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn log_lists_branches_and_tags_on_commits() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    t.commit_msg("one");
    t.git(&["tag", "v1"]);
    t.git(&["branch", "side"]);
    t.file("a.txt", "2\n");
    t.commit_msg("two");
    let commits = log(&t, &LogOptions::default());
    assert_eq!(commits[0].refs, ["main"]);
    assert_eq!(commits[1].refs, ["v1", "side"]);
    let one = t.repo().commit("v1").unwrap().unwrap();
    assert_eq!(one.refs, ["v1", "side"]);
    let line = LogOptions {
        path: Some(PathBuf::from("a.txt")),
        lines: Some(0..1),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &line)), ["two", "one"]);
    assert_eq!(log(&t, &line)[0].refs, ["main"]);
}

#[test]
fn log_filters_by_date() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    commit_dated(&t, "old", "2026-01-10T12:00:00Z");
    t.file("a.txt", "2\n");
    commit_dated(&t, "mid", "2026-02-10T12:00:00Z");
    t.file("a.txt", "3\n");
    commit_dated(&t, "new", "2026-03-10T12:00:00Z");
    let range = |since: Option<&str>, until: Option<&str>| LogOptions {
        since: since.map(str::to_owned),
        until: until.map(str::to_owned),
        ..LogOptions::default()
    };
    let after = log(&t, &range(Some("2026-02-01"), None));
    assert_eq!(subjects(&after), ["new", "mid"]);
    let before = log(&t, &range(None, Some("2026-02-01")));
    assert_eq!(subjects(&before), ["old"]);
    let between = log(&t, &range(Some("2026-02-01"), Some("2026-03-01")));
    assert_eq!(subjects(&between), ["mid"]);
    let bad = range(Some("--all"), None);
    assert!(t.repo().log(&bad, &AtomicBool::new(false)).is_err());
    let bad = range(None, Some("-1"));
    assert!(t.repo().log(&bad, &AtomicBool::new(false)).is_err());
}

#[test]
fn log_follows_renames_for_a_file_and_filters_directories() {
    let t = TempRepo::new();
    t.file(
        "old.txt",
        "a long enough file so that the rename is detected\nline two\n",
    );
    t.commit_msg("create");
    t.file("other/x.txt", "x\n");
    t.commit_msg("other");
    t.git(&["mv", "old.txt", "new.txt"]);
    t.commit_msg("rename");
    t.file(
        "new.txt",
        "a long enough file so that the rename is detected\nline two\nthree\n",
    );
    t.commit_msg("edit");
    let file = LogOptions {
        path: Some(PathBuf::from("new.txt")),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &file)), ["edit", "rename", "create"]);
    let absolute = LogOptions {
        path: Some(t.0.join("new.txt")),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &absolute)), ["edit", "rename", "create"]);
    let dir = LogOptions {
        path: Some(PathBuf::from("other")),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &dir)), ["other"]);
}

#[test]
fn log_filters_by_author_and_text() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    t.git(&["add", "-A"]);
    t.git(&[
        "commit",
        "-q",
        "-m",
        "Fix the Parser",
        "--author=Zed Zebra <zed@zoo.org>",
    ]);
    t.file("a.txt", "2\n");
    t.commit_msg("Unrelated.* change");
    let by_author = LogOptions {
        author: Some("ZOO.ORG".to_owned()),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &by_author)), ["Fix the Parser"]);
    let by_text = LogOptions {
        text: Some("the parser".to_owned()),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &by_text)), ["Fix the Parser"]);
    let literal = LogOptions {
        text: Some(".*".to_owned()),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &literal)), ["Unrelated.* change"]);
    let both = LogOptions {
        author: Some("zed".to_owned()),
        text: Some("unrelated".to_owned()),
        ..LogOptions::default()
    };
    assert_eq!(log(&t, &both), []);
}

#[test]
fn log_of_line_ranges() {
    let t = TempRepo::new();
    t.file("a.txt", "one\ntwo\nthree\nfour\n");
    t.commit_msg("create");
    t.file("a.txt", "one\nTWO\nthree\nfour\n");
    t.commit_msg("edit two");
    t.file("a.txt", "one\nTWO\nthree\nFOUR\n");
    t.commit_msg("edit four");
    let lines = |range| LogOptions {
        path: Some(PathBuf::from("a.txt")),
        lines: Some(range),
        ..LogOptions::default()
    };
    assert_eq!(subjects(&log(&t, &lines(1..2))), ["edit two", "create"]);
    assert_eq!(subjects(&log(&t, &lines(3..4))), ["edit four", "create"]);
    assert_eq!(log(&t, &lines(2..2)), []);
}

#[test]
fn log_shows_merges_and_multi_line_bodies() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    t.commit_msg("root");
    t.git(&["checkout", "-q", "-b", "side"]);
    t.file("s.txt", "s\n");
    t.git(&["add", "-A"]);
    t.git(&[
        "commit",
        "-q",
        "-m",
        "side work",
        "-m",
        "First paragraph\nwraps.",
        "-m",
        "Second paragraph.",
    ]);
    t.git(&["checkout", "-q", "main"]);
    t.file("m.txt", "m\n");
    t.commit_msg("main work");
    t.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let commits = log(&t, &LogOptions::default());
    assert_eq!(
        subjects(&commits),
        ["merge side", "main work", "side work", "root"]
    );
    assert_eq!(commits[0].parents, [t.rev("main~1"), t.rev("side")]);
    let side = t.repo().commit("side").unwrap().unwrap();
    assert_eq!(side.subject, "side work");
    assert_eq!(side.body, "First paragraph\nwraps.\n\nSecond paragraph.");
    assert_eq!(commits[0].body, "");
}

#[test]
fn changed_files_against_the_working_tree() {
    let t = TempRepo::new();
    t.file("edit.txt", "one\n");
    t.file("staged.txt", "s\n");
    t.commit_msg("base");
    t.file("edit.txt", "two\n");
    t.file("staged.txt", "s2\n");
    t.git(&["add", "staged.txt"]);
    t.file("fresh.txt", "u\n");
    let got = t
        .repo()
        .changed_files("HEAD", None, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(
        got,
        [
            change("edit.txt", None, FileStatus::Modified),
            change("fresh.txt", None, FileStatus::Untracked),
            change("staged.txt", None, FileStatus::Modified),
        ]
    );
}

fn rev_parse(t: &TempRepo, spec: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", spec])
        .current_dir(&t.0)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn content_ids_at_a_revision() {
    let t = TempRepo::new();
    t.file("a.txt", "one\n");
    t.file("dir/b.txt", "two\n");
    t.commit_all();
    t.file("a.txt", "changed\n");
    let ids = t
        .repo()
        .content_ids(
            Some("HEAD"),
            &[
                PathBuf::from("a.txt"),
                PathBuf::from("missing.txt"),
                PathBuf::from("dir/b.txt"),
                PathBuf::from("dir"),
            ],
            &git::never(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        ids,
        [
            Some(rev_parse(&t, "HEAD:a.txt")),
            None,
            Some(rev_parse(&t, "HEAD:dir/b.txt")),
            None,
        ]
    );
    assert!(
        t.repo()
            .content_ids(Some("-x"), &[PathBuf::from("a.txt")], &git::never())
            .is_err()
    );
}

#[test]
fn content_ids_of_the_working_tree() {
    let t = TempRepo::new();
    t.file("a.txt", "one\n");
    t.file("b.txt", "two\n");
    t.commit_all();
    let paths = [
        PathBuf::from("a.txt"),
        PathBuf::from("b.txt"),
        PathBuf::from("none.txt"),
    ];
    let repo = t.repo();
    let before = repo
        .content_ids(None, &paths, &git::never())
        .unwrap()
        .unwrap();
    assert_eq!(
        before,
        [
            Some(rev_parse(&t, "HEAD:a.txt")),
            Some(rev_parse(&t, "HEAD:b.txt")),
            None
        ]
    );
    t.file("a.txt", "edited\n");
    fs::remove_file(t.0.join("b.txt")).unwrap();
    let after = repo
        .content_ids(None, &paths, &git::never())
        .unwrap()
        .unwrap();
    assert!(after[0].is_some());
    assert_ne!(after[0], before[0]);
    assert_eq!(after[1], None);
    assert_eq!(after[2], None);
}

#[test]
fn content_ids_of_nothing_runs_no_git() {
    let t = TempRepo::new();
    let repo = t.repo();
    assert_eq!(
        repo.content_ids(None, &[], &git::never()).unwrap(),
        Some(vec![])
    );
    assert_eq!(
        repo.content_ids(Some("no-such-revision"), &[], &git::never())
            .unwrap(),
        Some(vec![])
    );
}

fn commit_as(t: &TempRepo, author: &str, message: &str) {
    t.git(&["add", "-A"]);
    let name = format!("user.name={author}");
    let out = Command::new("git")
        .args(["-c", &name, "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", message])
        .current_dir(&t.0)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn blame_attributes_lines_to_their_last_commit() {
    let t = TempRepo::new();
    t.file("a.txt", "one\ntwo\nthree\n");
    commit_as(&t, "Ann", "first");
    let first = t.rev("HEAD");
    t.file("a.txt", "one\nTWO\nthree\nfour\n");
    commit_as(&t, "Bob", "second");
    let second = t.rev("HEAD");
    let repo = t.repo();
    let lines = repo
        .blame("HEAD", &t.0.join("a.txt"), &git::never())
        .unwrap()
        .unwrap();
    let got: Vec<_> = lines
        .iter()
        .map(|l| (l.commit.as_str(), l.author.as_str(), l.summary.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (first.as_str(), "Ann", "first"),
            (second.as_str(), "Bob", "second"),
            (first.as_str(), "Ann", "first"),
            (second.as_str(), "Bob", "second"),
        ]
    );
    assert!(lines.iter().all(|l| l.time > 0));
    let old = repo
        .blame(&first, Path::new("a.txt"), &git::never())
        .unwrap()
        .unwrap();
    assert_eq!(old.len(), 3);
    assert!(old.iter().all(|l| l.author == "Ann"));
}

#[test]
fn blame_is_empty_for_a_missing_file_or_revision() {
    let t = TempRepo::new();
    t.file("a.txt", "1\n");
    t.commit_msg("one");
    let repo = t.repo();
    let blame = |rev: &str, path: &str| {
        repo.blame(rev, Path::new(path), &git::never())
            .unwrap()
            .unwrap()
    };
    assert_eq!(blame("HEAD", "missing.txt"), []);
    assert_eq!(blame("nope", "a.txt"), []);
    assert!(
        repo.blame("--x", Path::new("a.txt"), &git::never())
            .is_err()
    );
}
