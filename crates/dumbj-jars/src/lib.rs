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

const SEPARATOR: &str = "!/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceJar {
    pub path: PathBuf,
    pub artifact: String,
    pub version: String,
}

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

#[must_use]
pub fn discover_jdk() -> Option<SourceJar> {
    let source_zip = |home: &Path| {
        let zip = home.join("lib/src.zip");
        zip.is_file().then(|| {
            let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
            SourceJar {
                path: zip,
                artifact: "jdk".to_owned(),
                version: home
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            }
        })
    };
    if let Some(found) = std::env::var_os("JAVA_HOME").and_then(|home| source_zip(Path::new(&home)))
    {
        return Some(found);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let parents = [
        home.as_ref()
            .map(|home| home.join(".sdkman/candidates/java")),
        home.as_ref().map(|home| home.join(".jdks")),
        Some(PathBuf::from("/usr/lib/jvm")),
    ];
    parents
        .into_iter()
        .flatten()
        .filter_map(|parent| std::fs::read_dir(parent).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| source_zip(&entry.path()))
        .max_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)))
}

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

/// Multiplatform source jars put each source set in its own top directory, and may leave
/// the package directories out: `commonMain/Await.kt` is in `kotlinx.coroutines`.
fn source_set_of(entry: &str) -> Option<(&str, &str)> {
    let (first, rest) = entry.split_once('/')?;
    first.ends_with("Main").then_some((first, rest))
}

/// The JDK's `src.zip` puts each module in a top directory: `java.base/java/util/List.java`.
/// Package roots never contain a dot, module names do.
fn without_module(stem: &str) -> &str {
    match stem.split_once('/') {
        Some((first, rest)) if first.contains('.') => rest,
        _ => stem,
    }
}

fn stem_of(entry: &str) -> Option<&str> {
    let stem = entry
        .strip_suffix(".java")
        .or_else(|| entry.strip_suffix(".kt"))?;
    let name = stem.rsplit('/').next()?;
    let ignored = name.contains('-') || name == "package-info" || name == "module-info";
    (!ignored).then_some(stem)
}

fn class_from_source(head: &str, stem: &str) -> Option<String> {
    let name = stem.rsplit('/').next()?;
    let package = head.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("package ")?;
        Some(rest.trim_end_matches(';').trim().to_owned())
    });
    Some(match package {
        Some(package) if !package.is_empty() => format!("{package}.{name}"),
        _ => name.to_owned(),
    })
}

fn read_head(archive: &mut zip::ZipArchive<File>, entry: &str) -> Option<String> {
    const HEAD: u64 = 4096;
    let file = archive.by_name(entry).ok()?;
    let mut bytes = Vec::new();
    file.take(HEAD).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
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
            let mut archive = match open(&jar.path) {
                Ok(archive) => archive,
                Err(error) => {
                    tracing::debug!(%error, "skipping source jar");
                    continue;
                }
            };
            let entries: Vec<String> = archive
                .file_names()
                .filter_map(Result::ok)
                .map(std::borrow::Cow::into_owned)
                .collect();
            for entry in entries {
                let Some(stem) = stem_of(&entry) else {
                    continue;
                };
                let class = match source_set_of(stem) {
                    Some((_, rest)) => read_head(&mut archive, &entry)
                        .and_then(|head| class_from_source(&head, rest)),
                    None => Some(without_module(stem).replace('/', ".")),
                };
                if let Some(class) = class {
                    classes.entry(class).or_default().push((position, entry));
                }
            }
        }
        Some(Self { jars, classes })
    }

    #[must_use]
    pub fn jar_count(&self) -> usize {
        self.jars.len()
    }

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
        // One entry per jar: the JVM source set's over common code.
        found.sort_by_key(|source| {
            let common = source_set_of(&source.entry).is_some_and(|(set, _)| set != "jvmMain");
            (
                std::cmp::Reverse(version_key(&source.jar.version)),
                source.jar.path.clone(),
                common,
            )
        });
        found.dedup_by(|later, earlier| later.jar.path == earlier.jar.path);
        found
    }
}

#[must_use]
pub fn named_by_build(jar: &SourceJar, build_text: &str) -> bool {
    let version = &jar.version;
    let pinned = [
        format!("\"{version}\""),
        format!(":{version}\""),
        format!("<version>{version}</version>"),
    ]
    .iter()
    .any(|form| build_text.contains(form.as_str()));
    let group = jar
        .path
        .ancestors()
        .filter_map(|dir| dir.file_name()?.to_str())
        .skip(1)
        .find(|name| name.contains('.') && *name != version.as_str());
    pinned
        && (build_text.contains(&jar.artifact)
            || group.is_some_and(|group| build_text.contains(group)))
}

#[must_use]
pub fn virtual_path(jar: &Path, entry: &str) -> PathBuf {
    PathBuf::from(format!("{}{SEPARATOR}{entry}", jar.display()))
}

#[must_use]
pub fn split_virtual_path(path: &Path) -> Option<(PathBuf, String)> {
    let text = path.to_str()?;
    let at = [".jar", ".zip"]
        .iter()
        .filter_map(|extension| {
            text.find(&format!("{extension}{SEPARATOR}"))
                .map(|at| at + extension.len())
        })
        .min()?;
    let (jar, rest) = text.split_at(at);
    Some((PathBuf::from(jar), rest.strip_prefix(SEPARATOR)?.to_owned()))
}

#[must_use]
pub fn is_virtual_path(path: &Path) -> bool {
    split_virtual_path(path).is_some()
}

fn read_entry(jar: &Path, entry: &str) -> Result<String, JarError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_entries_name_classes_and_skip_package_and_module_info() {
        assert_eq!(stem_of("okio/Buffer.kt"), Some("okio/Buffer"));
        assert_eq!(stem_of("java/util/List.java"), Some("java/util/List"));
        assert_eq!(stem_of("okio/package-info.java"), None);
        assert_eq!(stem_of("module-info.java"), None);
        assert_eq!(stem_of("okio/-Platform.kt"), None);
        assert_eq!(stem_of("okio/Buffer.class"), None);
    }

    #[test]
    fn the_class_name_comes_from_the_package_line() {
        let java = "// licence\npackage okio.internal;\n\nclass Buffer {}";
        assert_eq!(
            class_from_source(java, "x/Buffer").as_deref(),
            Some("okio.internal.Buffer")
        );
        assert_eq!(
            class_from_source("class Buffer {}", "Buffer").as_deref(),
            Some("Buffer")
        );
        assert_eq!(
            class_from_source("package ;\n", "x/Buffer").as_deref(),
            Some("Buffer")
        );
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(version_key("1.10.0") > version_key("1.9.3"));
        assert!(version_key("2.0") > version_key("1.99"));
        assert!(version_key("3.2.1") > version_key("3.2.0"));
        assert_eq!(
            version_key("1.0-rc1"),
            [
                (1, String::new()),
                (0, String::new()),
                (0, "rc1".to_owned())
            ]
        );
    }

    #[test]
    fn a_member_is_looked_up_in_its_own_class() {
        let source = "package p\nclass First {\n    fun read() {}\n}\nclass Second {\n    fun read() {}\n}\n";
        let offset = locate(source, Language::Kotlin, "Second", Some("read")).get();
        assert_eq!(offset, source.rfind("read").unwrap_or_default());
    }
}
