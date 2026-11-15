//! Reference classification and local-scope resolution. A `§` in the source marks the
//! caret; it is removed before parsing.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use dumbj_syntax::{Language, Qualifier, ReferenceRole, SyntaxTree, parse};
use dumbj_text::{Buffer, ByteOffset};

fn at_caret(language: Language, marked: &str) -> (Buffer, SyntaxTree, ByteOffset) {
    let caret = marked.find('§').unwrap();
    let buffer = Buffer::new(&marked.replacen('§', "", 1));
    let tree = parse(language, &buffer, || false).unwrap().unwrap();
    (buffer, tree, ByteOffset(caret))
}

/// The role and qualifier of the identifier at the caret, as `name role qualifier`.
fn reference(language: Language, marked: &str) -> String {
    let (buffer, tree, caret) = at_caret(language, marked);
    match tree.reference_at(&buffer, caret) {
        Some(found) => {
            let import = if found.in_import { " import" } else { "" };
            format!(
                "{} {:?} {:?}{import}",
                found.name, found.role, found.qualifier
            )
        }
        None => "none".to_owned(),
    }
}

/// Resolves the identifier at the caret locally: `line:column type`.
fn local(language: Language, marked: &str) -> String {
    let (buffer, tree, caret) = at_caret(language, marked);
    let found = tree.reference_at(&buffer, caret).unwrap();
    match tree.resolve_local(&buffer, caret, &found.name, found.role) {
        Some(definition) => {
            let point = buffer.point_at(definition.name_range.start);
            assert_eq!(buffer.text(definition.name_range).unwrap(), found.name);
            format!(
                "{}:{} {:?}",
                point.line.display_number(),
                point.column + 1,
                definition.type_name
            )
        }
        None => "none".to_owned(),
    }
}

fn java(body: &str) -> String {
    format!("class C<T> {{\n int field;\n <U> void m(int p, String... rest) {{\n{body}\n }}\n}}")
}

fn kotlin(body: &str) -> String {
    format!(
        "class C<T>(seed: Int) {{\n val field = 1\n fun <U> m(p: Int, vararg rest: String) {{\n{body}\n }}\n}}"
    )
}

use Language::{Java, Kotlin};

#[test]
fn java_reference_roles() {
    let role = |body: &str| reference(Java, &java(body));
    assert_eq!(role("fo§o(1);"), "foo Call None");
    assert_eq!(role("x.fo§o(1);"), "foo Call Some(Name(\"x\"))");
    assert_eq!(role("this.fo§o(1);"), "foo Call Some(This)");
    assert_eq!(role("super.fo§o(1);"), "foo Call Some(Super)");
    assert_eq!(role("a.b.fo§o();"), "foo Call Some(Path(\"a.b\"))");
    assert_eq!(role("make().fo§o();"), "foo Call Some(Other)");
    assert_eq!(role("int v = x.fie§ld;"), "field Member Some(Name(\"x\"))");
    assert_eq!(role("int v = fie§ld;"), "field Value None");
    assert_eq!(role("Ma§th.abs(1);"), "Math Value None");
    assert_eq!(role("Fo§o v = null;"), "Foo Type None");
    assert_eq!(role("Object v = new Fo§o();"), "Foo Type None");
    assert_eq!(
        role("Outer.Inn§er v = null;"),
        "Inner Type Some(Name(\"Outer\"))"
    );
    assert_eq!(
        role("java.util.Li§st<String> v = null;"),
        "List Type Some(Path(\"java.util\"))"
    );
    assert_eq!(
        role("Runnable r = Foo::ba§r;"),
        "bar Call Some(Name(\"Foo\"))"
    );
    assert_eq!(role("@Overr§ide int v;"), "Override Type None");
    assert_eq!(role("int loc§al = 1;"), "local Declaration None");
    assert_eq!(
        role("foo§(1);"),
        "foo Call None",
        "caret at the end of the word"
    );
    assert_eq!(role("foo(§ 1);"), "none");
    assert_eq!(role("// fo§o"), "none");
    assert_eq!(role("String s = \"fo§o\";"), "none");
    assert_eq!(
        reference(Java, "import java.util.Li§st; class C {}"),
        "List Type Some(Path(\"java.util\")) import"
    );
    assert_eq!(
        reference(Java, "class C { void me§thod() {} }"),
        "method Declaration None"
    );
}

#[test]
fn java_locals() {
    let find = |body: &str| local(Java, &java(body));
    assert_eq!(find("int a = 1; int b = §a;"), "4:5 None");
    assert_eq!(
        find("String s = null; s§.length();"),
        "4:8 Some(\"String\")"
    );
    assert_eq!(
        find("java.util.List<String> xs = null; x§s.size();"),
        "4:24 Some(\"java.util.List\")"
    );
    assert_eq!(
        find("var made = new Foo(); mad§e.go();"),
        "4:5 Some(\"Foo\")"
    );
    assert_eq!(find("int v = §p;"), "3:17 None");
    assert_eq!(find("Object o = res§t;"), "3:30 None");
    assert_eq!(
        find("for (String s : rest) { s§.length(); }"),
        "4:13 Some(\"String\")"
    );
    assert_eq!(
        find("for (int i = 0; i < 3; i++) { int j = §i; }"),
        "4:10 None"
    );
    assert_eq!(
        find("Runnable r = () -> { int z = 1; int y = §z; };"),
        "4:26 None"
    );
    assert_eq!(find("F f = x -> §x + 1;"), "4:7 None");
    assert_eq!(find("F g = (a, b) -> a + §b;"), "4:11 None");
    assert_eq!(
        find("try (Reader in = open()) { i§n.read(); } catch (Exception e) { }"),
        "4:13 Some(\"Reader\")"
    );
    assert_eq!(
        find("try { } catch (IOException | Error e) { §e.printStackTrace(); }"),
        "4:36 Some(\"IOException\")"
    );
    assert_eq!(
        find("if (rest instanceof Object o) { §o.hashCode(); }"),
        "4:28 Some(\"Object\")"
    );
    assert_eq!(find("§T v = null;"), "1:9 None", "class type parameter");
    assert_eq!(find("§U v = null;"), "3:3 None", "method type parameter");
}

#[test]
fn java_scoping_rules() {
    let find = |body: &str| local(Java, &java(body));
    assert_eq!(find("int v = fie§ld;"), "none", "fields are not locals");
    assert_eq!(
        find("int v = lat§er; int later = 1;"),
        "none",
        "declared after use"
    );
    assert_eq!(
        find("{ int inner = 1; } int v = inn§er;"),
        "none",
        "out of scope"
    );
    assert_eq!(
        find("int a = 1; { int a2 = 2; int v = §a; }"),
        "4:5 None",
        "outer block is visible"
    );
    assert_eq!(
        find("int x = 1; { String x2 = null; } F f = x -> §x;"),
        "4:40 None",
        "innermost wins"
    );
    assert_eq!(find("Strin§g s;"), "none", "a type is not a local");
}

#[test]
fn kotlin_reference_roles() {
    let role = |body: &str| reference(Kotlin, &kotlin(body));
    assert_eq!(role("fo§o(1)"), "foo Call None");
    assert_eq!(role("x.fo§o(1)"), "foo Call Some(Name(\"x\"))");
    assert_eq!(role("this.fo§o(1)"), "foo Call Some(This)");
    assert_eq!(role("super.fo§o(1)"), "foo Call Some(Super)");
    assert_eq!(role("a.b.fo§o()"), "foo Call Some(Path(\"a.b\"))");
    assert_eq!(role("make().fo§o()"), "foo Call Some(Other)");
    assert_eq!(role("val v = x.na§me"), "name Member Some(Name(\"x\"))");
    assert_eq!(role("val v = fie§ld"), "field Value None");
    assert_eq!(role("val v: Fo§o? = null"), "Foo Type None");
    assert_eq!(role("val v = Fo§o()"), "Foo Call None");
    assert_eq!(
        role("val v: java.io.Fi§le? = null"),
        "File Type Some(Path(\"java.io\"))"
    );
    assert_eq!(role("val r = Foo::ba§r"), "bar Call Some(Name(\"Foo\"))");
    assert_eq!(role("val loc§al = 1"), "local Declaration None");
    assert_eq!(role("// fo§o"), "none");
    assert_eq!(role("val s = \"fo§o\""), "none");
    assert_eq!(
        reference(Kotlin, "import kotlin.math.ma§x\n"),
        "max Type Some(Path(\"kotlin.math\")) import"
    );
    assert_eq!(
        reference(Kotlin, "fun na§med() {}"),
        "named Declaration None"
    );
    assert_eq!(reference(Kotlin, "class Na§med"), "Named Declaration None");
}

#[test]
fn kotlin_locals() {
    let find = |body: &str| local(Kotlin, &kotlin(body));
    assert_eq!(find("val a = 1\nval b = §a"), "4:5 None");
    assert_eq!(
        find("val s: String = \"\"\ns§.length"),
        "4:5 Some(\"String\")"
    );
    assert_eq!(find("val made = Foo()\nmad§e.go()"), "4:5 Some(\"Foo\")");
    assert_eq!(find("val n = compute()\n§n.go()"), "4:5 None");
    assert_eq!(find("val v = §p"), "3:12 Some(\"Int\")");
    assert_eq!(find("val v = res§t"), "3:27 Some(\"String\")");
    assert_eq!(
        find("val v = se§ed"),
        "1:12 Some(\"Int\")",
        "constructor parameter"
    );
    assert_eq!(find("val (x, y) = pair\nval v = §y"), "4:9 None");
    assert_eq!(find("rest.forEach { item -> println(it§em) }"), "4:16 None");
    assert_eq!(find("for ((k, v) in map) { §v.go() }"), "4:10 None");
    assert_eq!(find("for (i in 0..3) { val j = §i }"), "4:6 None");
    assert_eq!(find("when (val w = p) { else -> §w }"), "4:11 None");
    assert_eq!(
        find("try { } catch (e: Exception) { §e.message }"),
        "4:16 Some(\"Exception\")"
    );
    assert_eq!(
        find("val fn = fun(q: Int): Int { return §q }"),
        "4:14 Some(\"Int\")"
    );
    assert_eq!(
        find("fun inner() {}\ninn§er()"),
        "4:5 None",
        "local function"
    );
    assert_eq!(
        find("val v: §T? = null"),
        "1:9 None",
        "class type parameter"
    );
    assert_eq!(
        find("val v: §U? = null"),
        "3:7 None",
        "function type parameter"
    );
}

#[test]
fn kotlin_scoping_rules() {
    let find = |body: &str| local(Kotlin, &kotlin(body));
    assert_eq!(find("val v = fie§ld"), "none", "properties are not locals");
    assert_eq!(
        find("rest.map { i§t.length }"),
        "none",
        "implicit `it` has no declaration"
    );
    assert_eq!(find("val v = lat§er\nval later = 1"), "none");
    assert_eq!(
        local(Kotlin, "class C(val kept: Int) { fun m() = ke§pt }"),
        "none",
        "a val constructor parameter is a property, not a local"
    );
    assert_eq!(find("run { val inner = 1 }\nval v = inn§er"), "none");
    assert_eq!(
        find("val x = 1\nrest.forEach { x -> println(§x) }"),
        "5:16 None",
        "innermost wins"
    );
}

#[test]
fn finds_every_identifier_with_a_name() {
    let source = "class C { int count; void m(int n) { count = n; /* count */ String s = \"count\"; this.count++; } }";
    let buffer = Buffer::new(source);
    let tree = parse(Java, &buffer, || false).unwrap().unwrap();
    let found = tree.references_named(&buffer, "count");
    let roles: Vec<_> = found
        .iter()
        .map(|r| (r.range.start.get(), r.role))
        .collect();
    assert_eq!(
        roles,
        [
            (14, ReferenceRole::Declaration),
            (37, ReferenceRole::Value),
            (85, ReferenceRole::Member),
        ]
    );
    assert_eq!(found[2].qualifier, Some(Qualifier::This));
    assert_eq!(tree.references_named(&buffer, "nope"), []);
}
