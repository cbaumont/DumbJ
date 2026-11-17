use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

fn resolve_target(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path),
        _ => Ok(path.to_path_buf()),
    }
}

fn write_temp(temp: &Path, bytes: &[u8], permissions: Option<fs::Permissions>) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;
    file.write_all(bytes)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.sync_all()
}

/// Writes `bytes` to `path` so that a crash leaves either the old or the new content: a
/// temporary file in the same directory, fsync, rename over the target, fsync of the
/// directory. A symlink is followed so the link itself survives.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<FileStamp> {
    let target = resolve_target(path)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy()
        .into_owned();
    let dir = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let temp = dir.join(format!(
        ".{file_name}.dumbj-{}-{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let permissions = fs::metadata(&target).ok().map(|m| m.permissions());
    let result = write_temp(&temp, bytes, permissions)
        .and_then(|()| fs::rename(&temp, &target))
        .and_then(|()| File::open(&dir)?.sync_all());
    match result {
        Ok(()) => FileStamp::of(&target),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    /// The ending most lines of `text` use; LF when there are none or on a tie.
    #[must_use]
    pub fn detect(text: &str) -> Self {
        let (mut lf, mut crlf) = (0usize, 0usize);
        let mut previous = 0u8;
        for byte in text.bytes() {
            if byte == b'\n' {
                if previous == b'\r' {
                    crlf += 1;
                } else {
                    lf += 1;
                }
            }
            previous = byte;
        }
        if crlf > lf { Self::CrLf } else { Self::Lf }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dumbj-disk-{name}-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn replaces_content_and_returns_the_stamp() {
        let dir = TempDir::new("replace");
        let path = dir.0.join("a.txt");
        fs::write(&path, "old").unwrap();
        let stamp = write_atomic(&path, b"new content").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");
        assert_eq!(stamp, FileStamp::of(&path).unwrap());
        assert_eq!(stamp.len, 11);
        assert_eq!(dir.entries(), ["a.txt"]);
    }

    #[test]
    fn creates_a_new_file() {
        let dir = TempDir::new("create");
        let path = dir.0.join("fresh.txt");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
        assert_eq!(dir.entries(), ["fresh.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("perms");
        let path = dir.0.join("run.sh");
        fs::write(&path, "x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        write_atomic(&path, b"y").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750);
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink() {
        let dir = TempDir::new("symlink");
        let target = dir.0.join("target.txt");
        let link = dir.0.join("link.txt");
        fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_atomic(&link, b"through").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(fs::read_to_string(&target).unwrap(), "through");
        assert_eq!(dir.entries(), ["link.txt", "target.txt"]);
    }

    #[test]
    fn failures_leave_no_temporary_file() {
        let dir = TempDir::new("failure");
        let missing = dir.0.join("no-such-dir").join("a.txt");
        assert!(write_atomic(&missing, b"x").is_err());
        assert!(!dir.0.join("no-such-dir").exists());

        let occupied = dir.0.join("occupied");
        fs::create_dir(&occupied).unwrap();
        assert!(write_atomic(&occupied, b"x").is_err());
        assert_eq!(dir.entries(), ["occupied"]);
    }

    #[test]
    fn detects_line_endings() {
        assert_eq!(LineEnding::detect(""), LineEnding::Lf);
        assert_eq!(LineEnding::detect("no breaks"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\nb\nc"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\nc\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("a\r\nb\n"), LineEnding::Lf, "tie is LF");
        assert_eq!(
            LineEnding::detect("a\rb"),
            LineEnding::Lf,
            "lone CR is not a break"
        );
        assert_eq!(LineEnding::CrLf.as_str(), "\r\n");
        assert_eq!(LineEnding::Lf.as_str(), "\n");
    }
}
