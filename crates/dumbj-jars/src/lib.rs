//! Library sources from Gradle and Maven `-sources.jar` files, read in place and never extracted.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_syntax::Language;
use dumbj_text::{Buffer, ByteOffset};

#[derive(Debug, thiserror::Error)]
pub enum JarError {
    #[error("cannot read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("cannot read the jar {0}: {1}")]
    Zip(PathBuf, zip::result::ZipError),
    #[error("{0} is not a path inside a jar")]
    NotInJar(PathBuf),
}

/// Separates a jar's path from an entry inside it, as in `lib.jar!/okio/Buffer.kt`.
const SEPARATOR: &str = "!/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceJar {
    pub path: PathBuf,
    pub artifact: String,
    pub version: String,
}

/// Gradle's and Maven's local caches.
#[must_use]
pub fn default_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    vec![
        home.join(".gradle/caches/modules-2/files-2.1"),
        home.join(".m2/repository"),
    ]
}

/// Maven: `<artifact>/<version>/<artifact>-<version>-sources.jar`. Gradle adds a hash
/// directory below the version.
fn coordinates(path: &Path) -> Option<(String, String)> {
    let stem = path.file_name()?.to_str()?.strip_suffix("-sources.jar")?;
    let names: Vec<&str> = path
        .ancestors()
        .skip(1)
        .take(3)
        .filter_map(|dir| dir.file_name()?.to_str())
        .collect();
    let matches = |artifact: &str, version: &str| stem == format!("{artifact}-{version}");
    match names.as_slice() {
        [version, artifact, ..] | [_, version, artifact] if matches(artifact, version) => {
            Some(((*artifact).to_owned(), (*version).to_owned()))
        }
        _ => None,
    }
}

/// Every source jar under `roots`. Returns `None` when cancelled.
#[must_use]
pub fn discover(roots: &[PathBuf], cancelled: &AtomicBool) -> Option<Vec<SourceJar>> {
    let mut found = Vec::new();
    for root in roots.iter().filter(|root| root.is_dir()) {
        let walk = ignore::WalkBuilder::new(root)
            .standard_filters(false)
            .follow_links(false)
            .build();
        for entry in walk.filter_map(Result::ok) {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let path = entry.path();
            if let Some((artifact, version)) = coordinates(path) {
                found.push(SourceJar {
                    path: path.to_path_buf(),
                    artifact,
                    version,
                });
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Some(found)
}

fn open(jar: &Path) -> Result<zip::ZipArchive<File>, JarError> {
    let file = File::open(jar).map_err(|error| JarError::Io(jar.to_path_buf(), error))?;
    zip::ZipArchive::new(file).map_err(|error| JarError::Zip(jar.to_path_buf(), error))
}

/// The fully qualified name a source entry declares by convention: `okio/Buffer.kt` is
/// `okio.Buffer`.
fn class_of_entry(entry: &str) -> Option<String> {
    let stem = entry
        .strip_suffix(".java")
        .or_else(|| entry.strip_suffix(".kt"))?;
    let name = stem.rsplit('/').next()?;
    if name.contains('-') || name == "package-info" || name == "module-info" {
        return None;
    }
    Some(stem.replace('/', "."))
}

/// Numeric parts compare as numbers, so 1.10 is newer than 1.9.
fn version_key(version: &str) -> Vec<(u64, String)> {
    version
        .split(['.', '-', '_'])
        .map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            (
                digits.parse().unwrap_or(0),
                part.get(digits.len()..).unwrap_or("").to_owned(),
            )
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassSource {
    pub jar: SourceJar,
    pub entry: String,
}

impl ClassSource {
    #[must_use]
    pub fn virtual_path(&self) -> PathBuf {
        virtual_path(&self.jar.path, &self.entry)
    }
}

#[derive(Debug, Default)]
pub struct JarIndex {
    jars: Vec<SourceJar>,
    classes: HashMap<String, Vec<(usize, String)>>,
}

impl JarIndex {
    /// Lists the entries of every jar; contents are only read on demand. A jar that
    /// cannot be read is skipped. Returns `None` when cancelled.
    #[must_use]
    pub fn build(jars: Vec<SourceJar>, cancelled: &AtomicBool) -> Option<Self> {
        let mut classes: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        for (position, jar) in jars.iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let archive = match open(&jar.path) {
                Ok(archive) => archive,
                Err(error) => {
                    tracing::debug!(%error, "skipping source jar");
                    continue;
                }
            };
            for entry in archive.file_names().filter_map(Result::ok) {
                if let Some(class) = class_of_entry(&entry) {
                    classes
                        .entry(class)
                        .or_default()
                        .push((position, entry.into_owned()));
                }
            }
        }
        Some(Self { jars, classes })
    }

    #[must_use]
    pub fn jar_count(&self) -> usize {
        self.jars.len()
    }

    /// The sources of the class `fqn`, newest version first.
    #[must_use]
    pub fn find(&self, fqn: &str) -> Vec<ClassSource> {
        let mut found: Vec<ClassSource> = self
            .classes
            .get(fqn)
            .into_iter()
            .flatten()
            .filter_map(|(jar, entry)| {
                Some(ClassSource {
                    jar: self.jars.get(*jar)?.clone(),
                    entry: entry.clone(),
                })
            })
            .collect();
        found.sort_by_key(|source| std::cmp::Reverse(version_key(&source.jar.version)));
        found
    }
}

#[must_use]
pub fn virtual_path(jar: &Path, entry: &str) -> PathBuf {
    PathBuf::from(format!("{}{SEPARATOR}{entry}", jar.display()))
}

/// Splits `lib.jar!/okio/Buffer.kt` into the jar and the entry.
#[must_use]
pub fn split_virtual_path(path: &Path) -> Option<(PathBuf, String)> {
    let text = path.to_str()?;
    let at = text.find(&format!(".jar{SEPARATOR}"))? + ".jar".len();
    let (jar, rest) = text.split_at(at);
    Some((PathBuf::from(jar), rest.strip_prefix(SEPARATOR)?.to_owned()))
}

#[must_use]
pub fn is_virtual_path(path: &Path) -> bool {
    split_virtual_path(path).is_some()
}

pub fn read_entry(jar: &Path, entry: &str) -> Result<String, JarError> {
    let mut archive = open(jar)?;
    let mut file = archive
        .by_name(entry)
        .map_err(|error| JarError::Zip(jar.to_path_buf(), error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| JarError::Io(virtual_path(jar, entry), error))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn read_virtual(path: &Path) -> Result<String, JarError> {
    let (jar, entry) =
        split_virtual_path(path).ok_or_else(|| JarError::NotInJar(path.to_path_buf()))?;
    read_entry(&jar, &entry)
}

/// Where to land in a library source: the member `member` of the class `class` (or a
/// Kotlin property behind a Java-style getter), else the class, else the file start.
#[must_use]
pub fn locate(source: &str, language: Language, class: &str, member: Option<&str>) -> ByteOffset {
    let buffer = Buffer::new(source);
    let Some(tree) = dumbj_syntax::parse(language, &buffer, || false)
        .ok()
        .flatten()
    else {
        return ByteOffset(0);
    };
    let facts = tree.facts(&buffer);
    let owner = facts
        .symbols
        .iter()
        .position(|symbol| symbol.name == class && symbol.kind.is_type());
    let property = member.and_then(|member| {
        let rest = member
            .strip_prefix("get")
            .or_else(|| member.strip_prefix("set"))?;
        let mut chars = rest.chars();
        let first = chars.next()?.to_lowercase();
        Some(first.chain(chars).collect::<String>())
    });
    let found = member.and_then(|member| {
        facts.symbols.iter().find(|symbol| {
            let name_fits =
                symbol.name == member || property.as_deref() == Some(symbol.name.as_str());
            name_fits && (owner.is_none() || symbol.parent == owner)
        })
    });
    found
        .or_else(|| owner.and_then(|owner| facts.symbols.get(owner)))
        .map_or(ByteOffset(0), |symbol| symbol.name_range.start)
}
