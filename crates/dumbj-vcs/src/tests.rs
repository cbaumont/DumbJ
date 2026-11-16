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
