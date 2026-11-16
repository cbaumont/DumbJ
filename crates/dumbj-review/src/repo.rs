#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoId {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoId {
    /// Parses a git remote URL (scp-like, ssh, https, git). `None` for local paths or anything else.
    pub fn from_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        let (host, path) = if let Some((_, rest)) = url.split_once("://") {
            let (authority, path) = rest.split_once('/')?;
            let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let host = authority.split(':').next()?;
            (host, path)
        } else {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            (host, path)
        };
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        let valid = |s: &str| !s.is_empty() && !s.contains('/') && s != "." && s != "..";
        if host.is_empty() || !valid(owner) || !valid(name) {
            return None;
        }
        Some(Self {
            host: host.to_string(),
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(host: &str, owner: &str, name: &str) -> RepoId {
        RepoId {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        }
    }

    #[test]
    fn parses_remote_forms() {
        for (url, expected) in [
            ("git@github.com:o/r.git", id("github.com", "o", "r")),
            ("git@github.com:o/r", id("github.com", "o", "r")),
            (
                "ssh://git@host.example:2222/o/r.git",
                id("host.example", "o", "r"),
            ),
            ("ssh://git@github.com/o/r.git", id("github.com", "o", "r")),
            ("https://github.com/o/r", id("github.com", "o", "r")),
            ("https://github.com/o/r.git/", id("github.com", "o", "r")),
            ("https://user:pw@ghe.corp/o/r.git", id("ghe.corp", "o", "r")),
            ("git://github.com/o/r.git", id("github.com", "o", "r")),
            (
                "  https://github.com/o/my.repo.git\n",
                id("github.com", "o", "my.repo"),
            ),
        ] {
            assert_eq!(RepoId::from_remote_url(url), Some(expected), "{url}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for url in [
            "/home/x/repo",
            "../repo",
            "repo",
            "",
            "file:///home/x/repo",
            "https://github.com/onlyowner",
            "https://github.com/",
            "https://github.com/o/r/extra",
        ] {
            assert_eq!(RepoId::from_remote_url(url), None, "{url}");
        }
    }
}
