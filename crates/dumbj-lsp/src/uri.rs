use std::ffi::OsString;
use std::fmt::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

#[must_use]
pub fn path_to_uri(path: &Path) -> Uri {
    let mut text = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            text.push(char::from(byte));
        } else {
            let _ = write!(text, "%{byte:02X}");
        }
    }
    Uri::from_str(&text).unwrap_or_else(|_| root())
}

#[expect(clippy::expect_used, reason = "a constant that is a valid URI")]
fn root() -> Uri {
    Uri::from_str("file:///").expect("valid constant URI")
}

#[must_use]
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.scheme()?.as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    let raw = uri.path().as_str().as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = std::str::from_utf8(raw.get(i + 1..i + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(path: &str) {
        let uri = path_to_uri(Path::new(path));
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from(path)), "{uri:?}");
    }

    #[test]
    fn plain_path() {
        assert_eq!(
            path_to_uri(Path::new("/home/a/b.kt")).as_str(),
            "file:///home/a/b.kt"
        );
    }

    #[test]
    fn encodes_special_characters() {
        let uri = path_to_uri(Path::new("/tmp/my dir/a#b?c%d.kt"));
        assert_eq!(uri.as_str(), "file:///tmp/my%20dir/a%23b%3Fc%25d.kt");
        round_trip("/tmp/my dir/a#b?c%d.kt");
    }

    #[test]
    fn unicode_round_trips() {
        round_trip("/tmp/caf\u{e9}/\u{1f600}/\u{65e5}\u{672c}.java");
    }

    #[test]
    fn decodes_lowercase_escapes() {
        let uri = Uri::from_str("file:///a%2fb%c3%a9").unwrap();
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from("/a/b\u{e9}")));
    }

    #[test]
    fn other_schemes_are_none() {
        let uri = Uri::from_str("jdt://contents/rt.jar/java.lang/String.class").unwrap();
        assert_eq!(uri_to_path(&uri), None);
    }
}
