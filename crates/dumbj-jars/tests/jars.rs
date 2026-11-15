#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use dumbj_jars::{
    JarIndex, default_roots, discover, locate, read_entry, read_virtual, split_virtual_path,
    virtual_path,
};
use dumbj_syntax::Language;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("dumbj-jars-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_jar(path: &Path, entries: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

const BUFFER_KT: &str = "package okio\n\nclass Buffer {\n    val size: Long = 0\n    fun readUtf8(): String = \"\"\n}\n";

#[test]
fn discovers_indexes_and_reads_gradle_and_maven_jars() {
    let scratch = Scratch::new("discover");
    let gradle = scratch.0.join("gradle");
    let maven = scratch.0.join("m2");
    write_jar(
        &gradle.join("com.squareup.okio/okio/3.9.0/abc123/okio-3.9.0-sources.jar"),
        &[
            ("okio/Buffer.kt", BUFFER_KT),
            ("okio/package-info.java", ""),
        ],
    );
    write_jar(
        &gradle.join("com.squareup.okio/okio/3.10.2/def456/okio-3.10.2-sources.jar"),
        &[("okio/Buffer.kt", BUFFER_KT)],
    );
    write_jar(
        &maven.join("org/example/lib/1.0/lib-1.0-sources.jar"),
        &[(
            "org/example/Thing.java",
            "package org.example;\nclass Thing {}\n",
        )],
    );
    std::fs::write(gradle.join("not-a-jar.txt"), "x").unwrap();

    let jars = discover(&[gradle, maven], &AtomicBool::new(false)).unwrap();
    let mut seen: Vec<String> = jars
        .iter()
        .map(|jar| format!("{} {}", jar.artifact, jar.version))
        .collect();
    seen.sort();
    assert_eq!(seen, ["lib 1.0", "okio 3.10.2", "okio 3.9.0"]);

    let index = JarIndex::build(jars, &AtomicBool::new(false)).unwrap();
    let found = index.find("okio.Buffer");
    let versions: Vec<&str> = found.iter().map(|s| s.jar.version.as_str()).collect();
    assert_eq!(
        versions,
        ["3.10.2", "3.9.0"],
        "newest first, 1.10 after 1.9"
    );
    assert_eq!(index.find("okio.package-info"), []);
    assert_eq!(index.find("org.example.Thing").len(), 1);

    let newest = &found[0];
    let path = newest.virtual_path();
    assert_eq!(
        split_virtual_path(&path),
        Some((newest.jar.path.clone(), "okio/Buffer.kt".to_owned()))
    );
    assert_eq!(read_virtual(&path).unwrap(), BUFFER_KT);
    assert!(read_entry(&newest.jar.path, "okio/Missing.kt").is_err());
    assert_eq!(split_virtual_path(Path::new("/plain/File.kt")), None);
    assert_eq!(
        virtual_path(Path::new("/a/b.jar"), "c/D.java"),
        PathBuf::from("/a/b.jar!/c/D.java")
    );
}

#[test]
fn locates_classes_members_and_getters() {
    let at = |member: Option<&str>| {
        let offset = locate(BUFFER_KT, Language::Kotlin, "Buffer", member).get();
        BUFFER_KT[offset..]
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap()
            .to_owned()
    };
    assert_eq!(at(None), "Buffer");
    assert_eq!(at(Some("readUtf8")), "readUtf8");
    assert_eq!(
        at(Some("getSize")),
        "size",
        "a Java getter lands on the property"
    );
    assert_eq!(at(Some("missing")), "Buffer");
    assert_eq!(locate("not code", Language::Kotlin, "Nope", None).get(), 0);
}

#[test]
fn default_roots_are_the_gradle_and_maven_caches() {
    let roots = default_roots();
    assert!(
        roots
            .iter()
            .any(|root| root.ends_with(".gradle/caches/modules-2/files-2.1"))
    );
    assert!(roots.iter().any(|root| root.ends_with(".m2/repository")));
}
