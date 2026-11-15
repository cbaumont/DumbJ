#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use std::fmt::Write as _;
use std::path::Path;

use dumbj_syntax::{FileFacts, Language, Platform, SymbolKind, parse};
use dumbj_text::Buffer;

fn facts_of(language: Language, source: &str) -> (Buffer, FileFacts) {
    let buffer = Buffer::new(source);
    let tree = parse(language, &buffer, || false).unwrap().unwrap();
    let facts = tree.facts(&buffer);
    (buffer, facts)
}

fn render(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let language = Language::from_path(&path).unwrap();
    let (buffer, facts) = facts_of(language, &std::fs::read_to_string(path).unwrap());
    let mut out = format!("package {:?}\n", facts.package);
    for import in &facts.imports {
        let flags = [(import.wildcard, ".*"), (import.is_static, " static")]
            .iter()
            .filter(|(set, _)| *set)
            .map(|(_, text)| *text)
            .collect::<String>();
        let alias = import
            .alias
            .as_ref()
            .map_or_else(String::new, |alias| format!(" as {alias}"));
        writeln!(
            out,
            "import {}{flags}{alias} -> {:?}",
            import.path,
            import.local_name()
        )
        .unwrap();
    }
    for (index, symbol) in facts.symbols.iter().enumerate() {
        let point = buffer.point_at(symbol.name_range.start);
        assert_eq!(
            buffer.text(symbol.name_range.clone()).unwrap(),
            if symbol.name_range.is_empty() {
                String::new()
            } else {
                symbol.name.clone()
            },
            "name range of {}",
            symbol.name
        );
        assert!(
            symbol.range.start <= symbol.name_range.start
                && symbol.name_range.end <= symbol.range.end
        );
        let supers = if symbol.supertypes.is_empty() {
            String::new()
        } else {
            format!(" : {}", symbol.supertypes.join(", "))
        };
        let receiver = symbol
            .receiver
            .as_ref()
            .map_or_else(String::new, |r| format!(" on {r}"));
        let type_name = symbol
            .type_name
            .as_ref()
            .map_or_else(String::new, |t| format!(" -> {t}"));
        let parameters = symbol.parameters.as_ref().map_or_else(String::new, |all| {
            let shown: Vec<String> = all
                .iter()
                .map(|parameter| {
                    format!(
                        "{}{}: {}{}",
                        if parameter.variadic { "*" } else { "" },
                        parameter.name,
                        parameter.type_name.as_deref().unwrap_or("?"),
                        if parameter.optional { " =" } else { "" },
                    )
                })
                .collect();
            format!("({})", shown.join(", "))
        });
        writeln!(
            out,
            "{}:{} {}{:?} {}{parameters}{supers}{receiver}{type_name}",
            point.line.display_number(),
            point.column + 1,
            match symbol.platform {
                Some(Platform::Expect) => "expect ",
                Some(Platform::Actual) => "actual ",
                None => "",
            },
            symbol.kind,
            facts.qualified_name(index),
        )
        .unwrap();
    }
    out
}

#[test]
fn java_structure() {
    insta::assert_snapshot!(render("Structure.java"));
}

#[test]
fn kotlin_structure() {
    insta::assert_snapshot!(render("Structure.kt"));
}

#[test]
fn markdown_structure() {
    insta::assert_snapshot!(render("Structure.md"));
}

#[test]
fn toml_structure() {
    insta::assert_snapshot!(render("libs.versions.toml"));
}

#[test]
fn xml_structure() {
    insta::assert_snapshot!(render("pom.xml"));
}

#[test]
fn yaml_structure() {
    insta::assert_snapshot!(render("sample.yaml"));
}

#[test]
fn declarations_survive_syntax_errors() {
    let (_, facts) = facts_of(
        Language::Java,
        "package p; class A { void broken( { int x = ; } void ok() {} int field; }",
    );
    let names: Vec<_> = facts
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert!(names.contains(&"A"), "{names:?}");
    assert_eq!(facts.package.as_deref(), Some("p"));

    // The Kotlin grammar gap: the function with the annotated function type is damaged,
    // but declarations around it are still found.
    let (_, facts) = facts_of(
        Language::Kotlin,
        "class Before\nfun Button(text: @Composable () -> Unit) {}\nfun after() {}\nclass After { val x = 1 }\n",
    );
    let names: Vec<_> = facts
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    for expected in ["Before", "after", "After", "x"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
}

#[test]
fn kotlin_class_kinds() {
    let (_, facts) = facts_of(
        Language::Kotlin,
        "enum class E { A }\nannotation class Ann\nfun interface F { fun f() }\ndata class D(val x: Int)\nobject O\n",
    );
    let kinds: Vec<_> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("E", SymbolKind::Enum),
            ("A", SymbolKind::EnumConstant),
            ("Ann", SymbolKind::Annotation),
            ("F", SymbolKind::Interface),
            ("f", SymbolKind::Method),
            ("D", SymbolKind::Class),
            ("x", SymbolKind::Property),
            ("O", SymbolKind::Object),
        ]
    );
}

#[test]
fn empty_files_and_languages_without_an_outline_have_no_symbols() {
    assert_eq!(
        facts_of(Language::Json, "{\"a\": 1}").1,
        FileFacts::default()
    );
    assert_eq!(facts_of(Language::Yaml, "").1, FileFacts::default());
    assert_eq!(facts_of(Language::Java, "").1, FileFacts::default());
    assert_eq!(facts_of(Language::Kotlin, "").1, FileFacts::default());
}
