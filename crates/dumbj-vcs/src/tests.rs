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
