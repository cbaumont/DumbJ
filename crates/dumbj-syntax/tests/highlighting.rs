//! Golden tests: every highlight span of the fixture files.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use dumbj_syntax::{Language, parse};
use dumbj_text::{Buffer, ByteOffset};

fn fixture(name: &str) -> (Language, Buffer) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let language = Language::from_path(&path).unwrap();
    (
        language,
        Buffer::new(&std::fs::read_to_string(path).unwrap()),
    )
}

/// One line per span: `line:column Kind "text"`.
fn render(name: &str) -> String {
    let (language, buffer) = fixture(name);
    let tree = parse(language, &buffer, || false).unwrap().unwrap();
    assert!(!tree.has_errors(), "{name} should parse cleanly");
    let spans = tree
        .highlights(&buffer, ByteOffset::ZERO..buffer.len())
        .unwrap();
    let mut out = String::new();
    let mut previous_end = ByteOffset::ZERO;
    for span in spans {
        assert!(
            span.range.start >= previous_end,
            "spans overlap or are out of order"
        );
        assert!(span.range.start < span.range.end, "empty span");
        previous_end = span.range.end;
        let point = buffer.point_at(span.range.start);
        let text = buffer.text(span.range).unwrap();
        writeln!(
            out,
            "{}:{} {:?} {text:?}",
            point.line.display_number(),
            point.column + 1,
            span.kind
        )
        .unwrap();
    }
    out
}

#[test]
fn java_highlights() {
    insta::assert_snapshot!(render("Sample.java"));
}

#[test]
fn kotlin_highlights() {
    insta::assert_snapshot!(render("Sample.kt"));
}

#[test]
fn yaml_highlights() {
    insta::assert_snapshot!(render("sample.yaml"));
}

#[test]
fn xml_highlights() {
    insta::assert_snapshot!(render("pom.xml"));
}

#[test]
fn properties_highlights() {
    insta::assert_snapshot!(render("application.properties"));
}

#[test]
fn json_highlights() {
    insta::assert_snapshot!(render("package.json"));
}

#[test]
fn toml_highlights() {
    insta::assert_snapshot!(render("libs.versions.toml"));
}

#[test]
fn groovy_highlights() {
    insta::assert_snapshot!(render("build.gradle"));
}

#[test]
fn markdown_highlights() {
    insta::assert_snapshot!(render("README.md"));
}

#[test]
fn shell_highlights() {
    insta::assert_snapshot!(render("run.sh"));
}

#[test]
fn dockerfile_highlights() {
    insta::assert_snapshot!(render("Dockerfile"));
}

#[test]
fn a_range_query_matches_the_same_part_of_a_full_query() {
    for name in [
        "Sample.java",
        "Sample.kt",
        "sample.yaml",
        "pom.xml",
        "application.properties",
        "package.json",
        "libs.versions.toml",
        "build.gradle",
        "README.md",
        "run.sh",
        "Dockerfile",
    ] {
        let (language, buffer) = fixture(name);
        let tree = parse(language, &buffer, || false).unwrap().unwrap();
        let all = tree
            .highlights(&buffer, ByteOffset::ZERO..buffer.len())
            .unwrap();
        let window =
            buffer.line_start(dumbj_text::LineIndex(5))..buffer.line_end(dumbj_text::LineIndex(12));
        let part = tree.highlights(&buffer, window.clone()).unwrap();
        let expected: Vec<_> = all
            .into_iter()
            .filter(|span| span.range.end > window.start && span.range.start < window.end)
            .map(|mut span| {
                span.range.start = span.range.start.max(window.start);
                span.range.end = span.range.end.min(window.end);
                span
            })
            .collect();
        assert_eq!(part, expected, "{name}");
    }
}

#[test]
fn broken_source_still_highlights() {
    let buffer = Buffer::new("class Broken { void f( { int x = ; } ");
    let tree = parse(Language::Java, &buffer, || false).unwrap().unwrap();
    assert!(tree.has_errors());
    let spans = tree
        .highlights(&buffer, ByteOffset::ZERO..buffer.len())
        .unwrap();
    assert!(spans.len() > 3);
}

/// A known gap in the Kotlin grammar (ARCHITECTURE, Kotlin grammar decision). When this
/// test fails the grammar has been fixed upstream: delete it and update the decision.
#[test]
fn kotlin_annotated_function_type_parameter_is_a_known_grammar_gap() {
    let source = "fun Button(text: @Composable () -> Unit, enabled: Boolean) {}\nfun after() {}\n";
    let buffer = Buffer::new(source);
    let tree = parse(Language::Kotlin, &buffer, || false).unwrap().unwrap();
    assert!(
        tree.has_errors(),
        "the grammar now parses annotated function types"
    );
    // The damage stays local: the following declaration is still highlighted.
    let spans = tree
        .highlights(&buffer, ByteOffset::ZERO..buffer.len())
        .unwrap();
    let after = source.find("after").unwrap();
    assert!(spans.iter().any(|span| span.range.start.get() == after));
}

#[test]
fn parsing_can_be_cancelled() {
    let source = "class A { void f() { int x = 1; } }\n".repeat(20_000);
    let buffer = Buffer::new(&source);
    let polls = AtomicUsize::new(0);
    let tree = parse(Language::Java, &buffer, || {
        polls.fetch_add(1, Ordering::Relaxed) >= 3
    })
    .unwrap();
    assert!(tree.is_none());
    assert!(polls.load(Ordering::Relaxed) >= 3);
}

#[test]
fn empty_and_out_of_range_queries_are_empty() {
    let (language, buffer) = fixture("Sample.java");
    let tree = parse(language, &buffer, || false).unwrap().unwrap();
    assert_eq!(
        tree.highlights(&buffer, ByteOffset(5)..ByteOffset(5))
            .unwrap(),
        []
    );
    assert_eq!(
        tree.highlights(&buffer, ByteOffset(99_999)..ByteOffset(100_000))
            .unwrap(),
        []
    );
    assert_eq!(tree.buffer_version(), buffer.version());
}
