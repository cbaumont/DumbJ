#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use dumbj_jars::{
    JarIndex, SourceJar, discover, locate, named_by_build, read_virtual,
    split_virtual_path, virtual_path,
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
        &gradle.join(
            "org.jetbrains.kotlinx/coroutines-jvm/1.9.0/f00/coroutines-jvm-1.9.0-sources.jar",
        ),
        &[
            (
                "commonMain/Await.kt",
                "package kotlinx.coroutines\n\nfun await() {}\n",
            ),
            (
                "commonMain/kotlin/Job.kt",
                "package kotlinx.coroutines\n\ninterface Job\n",
            ),
            (
                "jvmMain/kotlin/Job.kt",
                "package kotlinx.coroutines\n\ninterface Job\n",
            ),
        ],
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
    assert_eq!(
        seen,
        [
            "coroutines-jvm 1.9.0",
            "lib 1.0",
            "okio 3.10.2",
            "okio 3.9.0"
        ]
    );

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

    assert_eq!(
        index.find("kotlinx.coroutines.Await")[0].entry,
        "commonMain/Await.kt",
        "a source set without package directories: the package line decides"
    );
    let job = index.find("kotlinx.coroutines.Job");
    assert_eq!(job.len(), 1, "one entry per jar");
    assert_eq!(
        job[0].entry, "jvmMain/kotlin/Job.kt",
        "the JVM source set first"
    );
    let newest = &found[0];
    let path = newest.virtual_path();
    assert_eq!(
        split_virtual_path(&path),
        Some((newest.jar.path.clone(), "okio/Buffer.kt".to_owned()))
    );
    assert_eq!(read_virtual(&path).unwrap(), BUFFER_KT);
    assert!(read_virtual(&virtual_path(&newest.jar.path, "okio/Missing.kt")).is_err());
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
fn build_files_pick_the_version() {
    let jar = |version: &str| SourceJar {
        path: PathBuf::from(format!(
            "/g/org.jetbrains.kotlin/kotlin-stdlib/{version}/h/kotlin-stdlib-{version}-sources.jar"
        )),
        artifact: "kotlin-stdlib".to_owned(),
        version: version.to_owned(),
    };
    let toml = "[versions]\nkotlin = \"2.3.21\"\n[libraries]\nx = { module = \"org.jetbrains.kotlin:kotlin-reflect\", version.ref = \"kotlin\" }\n";
    assert!(
        named_by_build(&jar("2.3.21"), toml),
        "by group and quoted version"
    );
    assert!(!named_by_build(&jar("2.4.10"), toml));
    assert!(named_by_build(
        &jar("1.9.0"),
        "implementation(\"x:kotlin-stdlib:1.9.0\")"
    ));
    assert!(!named_by_build(
        &jar("2.3.21"),
        "version = \"2.3.21\" of something else"
    ));
}

#[test]
fn jdk_source_zips_drop_the_module_directory() {
    let scratch = Scratch::new("jdk");
    let zip = scratch.0.join("jdk-21/lib/src.zip");
    write_jar(
        &zip,
        &[(
            "java.base/java/util/List.java",
            "package java.util;\npublic interface List<E> {}\n",
        )],
    );
    let jar = SourceJar {
        path: zip.clone(),
        artifact: "jdk".to_owned(),
        version: "jdk-21".to_owned(),
    };
    let index = JarIndex::build(vec![jar], &AtomicBool::new(false)).unwrap();
    let found = index.find("java.util.List");
    assert_eq!(found.len(), 1);
    let path = found[0].virtual_path();
    assert_eq!(
        split_virtual_path(&path),
        Some((zip, "java.base/java/util/List.java".to_owned()))
    );
    assert!(read_virtual(&path).unwrap().contains("interface List"));
}
