use std::ffi::OsStr;
use std::io::ErrorKind;
use std::process::{Command, Stdio};

use crate::ReviewError;

pub fn token(host: &str) -> Result<String, ReviewError> {
    token_with(OsStr::new("gh"), host)
}

pub fn token_with(program: &OsStr, host: &str) -> Result<String, ReviewError> {
    let output = Command::new(program)
        .args(["auth", "token", "--hostname", host])
        .stdin(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => ReviewError::GhMissing,
            _ => ReviewError::Io(e),
        })?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !token.is_empty() {
        Ok(token)
    } else {
        Err(ReviewError::NotSignedIn {
            host: host.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "dumbj-review-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn script(&self, body: &str) -> PathBuf {
            let path = self.0.join("gh");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn prints_the_trimmed_token_and_passes_the_host() {
        let dir = Dir::new();
        let args = dir.0.join("args");
        let gh = dir.script(&format!(
            "echo \"$@\" > '{}'\necho ghp_secret",
            args.display()
        ));
        assert_eq!(
            token_with(gh.as_os_str(), "github.com").unwrap(),
            "ghp_secret"
        );
        assert_eq!(
            fs::read_to_string(args).unwrap().trim(),
            "auth token --hostname github.com"
        );
    }

    #[test]
    fn a_failing_gh_means_not_signed_in() {
        let dir = Dir::new();
        let gh = dir.script("echo 'no oauth token found' >&2\nexit 1");
        let err = token_with(gh.as_os_str(), "example.com").unwrap_err();
        assert!(matches!(&err, ReviewError::NotSignedIn { host } if host == "example.com"));
        assert!(
            err.to_string()
                .contains("gh auth login --hostname example.com")
        );
    }

    #[test]
    fn an_empty_token_means_not_signed_in() {
        let dir = Dir::new();
        let gh = dir.script("exit 0");
        assert!(matches!(
            token_with(gh.as_os_str(), "github.com"),
            Err(ReviewError::NotSignedIn { .. })
        ));
    }

    #[test]
    fn a_missing_program_means_gh_missing() {
        let dir = Dir::new();
        let missing = dir.0.join("nope");
        assert!(matches!(
            token_with(missing.as_os_str(), "github.com"),
            Err(ReviewError::GhMissing)
        ));
    }
}
