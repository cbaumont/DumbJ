//! A `§` in the source marks the caret; it is removed before parsing.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use dumbj_syntax::{Argument, Language, Qualifier, ReferenceRole, SyntaxTree, parse};
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

/// An expression in a compact form: `a.b`, `f(x)`, a bare type name for a typed value,
/// `{}` for a lambda and `?` for anything the engine does not follow.
fn show(expression: &Qualifier) -> String {
    match expression {
        Qualifier::This => "this".to_owned(),
        Qualifier::Super => "super".to_owned(),
        Qualifier::Name(name) | Qualifier::Path(name) | Qualifier::Typed(name) => name.clone(),
        Qualifier::Member(receiver, name) => format!("{}.{name}", show(receiver)),
        Qualifier::Call(call) => {
            let receiver = call
                .receiver
                .as_ref()
                .map_or_else(String::new, |receiver| format!("{}.", show(receiver)));
            format!(
                "{receiver}{}({})",
                call.name,
                show_arguments(&call.arguments)
            )
        }
        Qualifier::Lambda => "{}".to_owned(),
        Qualifier::Other => "?".to_owned(),
    }
}

fn show_arguments(arguments: &[Argument]) -> String {
    let shown: Vec<String> = arguments
        .iter()
        .map(|argument| match &argument.name {
            Some(name) => format!("{name}={}", show(&argument.value)),
            None if argument.trailing => format!("trailing {}", show(&argument.value)),
            None => show(&argument.value),
        })
        .collect();
    shown.join(", ")
}

/// The call under the caret as `qualifier.name(arguments)`.
fn call(language: Language, marked: &str) -> String {
    let (buffer, tree, caret) = at_caret(language, marked);
    let found = tree.reference_at(&buffer, caret).unwrap();
    let qualifier = found
        .qualifier
        .map_or_else(String::new, |qualifier| format!("{}.", show(&qualifier)));
    let arguments = found
        .arguments
        .map_or_else(|| "no call".to_owned(), |all| show_arguments(&all));
    format!("{qualifier}{}({arguments})", found.name)
}

#[test]
fn java_calls_and_their_arguments() {
    let call = |body: &str| call(Language::Java, &java(body));
    assert_eq!(call("make().fo§o();"), "make().foo()");
    assert_eq!(call("a.make(1).b.fo§o();"), "a.make(int).b.foo()");
    assert_eq!(call("new Foo<>(x).fo§o();"), "Foo.foo()");
    assert_eq!(call("((Bar) x).fo§o();"), "Bar.foo()");
    assert_eq!(call("((T) x).fo§o();"), "?.foo()", "a type parameter");
    assert_eq!(
        call("fo§o(\"s\", 'c', true, 1.5, x, a.b, this, y -> y, C::m, a + b, null);"),
        "foo(String, char, boolean, double, x, a.b, this, {}, {}, ?, ?)"
    );
    assert_eq!(call("int v = x.fie§ld;"), "x.field(no call)");
}

#[test]
fn kotlin_calls_and_their_arguments() {
    let call = |body: &str| call(Language::Kotlin, &kotlin(body));
    assert_eq!(call("make().fo§o()"), "make().foo()");
    assert_eq!(call("a.make(1)!!.b?.fo§o()"), "a.make(Int).b.foo()");
    assert_eq!(call("(x as Bar).fo§o()"), "Bar.foo()");
    assert_eq!(call("(x as T).fo§o()"), "?.foo()", "a type parameter");
    assert_eq!(call("this.state.fo§o()"), "this.state.foo()");
    assert_eq!(
        call("fo§o(\"s\", 'c', true, 1.5, 2L, x, a.b, limit = 3, block = { it })"),
        "foo(String, Char, Boolean, Double, Long, x, a.b, limit=Int, block={})"
    );
    assert_eq!(call("fo§o(1) { it }"), "foo(Int, trailing {})");
    assert_eq!(call("x.fo§o { it }"), "x.foo(trailing {})");
    assert_eq!(call("fo§o(::m, a + b, null)"), "foo({}, ?, ?)");
    assert_eq!(call("val v = x.na§me"), "x.name(no call)");
}

#[test]
fn type_parameters_are_not_declared_types() {
    assert_eq!(
        local(
            Language::Java,
            &java("T t = null; U u = null; §t.go(); u.go();")
        ),
        "4:3 None"
    );
    assert_eq!(
        local(
            Language::Kotlin,
            &kotlin("val t: T = make()\nval u: U? = null\n§u.go()")
        ),
        "5:5 None"
    );
}

fn initialiser(language: Language, marked: &str) -> String {
    let (buffer, tree, caret) = at_caret(language, marked);
    let found = tree.reference_at(&buffer, caret).unwrap();
    tree.resolve_local(&buffer, caret, &found.name, found.role)
        .and_then(|definition| definition.initialiser)
        .map_or_else(|| "none".to_owned(), |value| show(&value))
}

#[test]
fn locals_carry_their_initialiser() {
    let java = |body: &str| initialiser(Language::Java, &java(body));
    assert_eq!(java("var made = new Foo(); mad§e.go();"), "Foo");
    assert_eq!(java("var n = x.compute(1); §n.go();"), "x.compute(int)");
    assert_eq!(java("Foo f; §f.go();"), "none");
    let kotlin = |body: &str| initialiser(Language::Kotlin, &kotlin(body));
    assert_eq!(kotlin("val made = Foo()\nmad§e.go()"), "Foo()");
    assert_eq!(kotlin("val n = x.compute(1)\n§n.go()"), "x.compute(Int)");
    assert_eq!(kotlin("val (a, b) = pair()\n§a.go()"), "none");
    assert_eq!(kotlin("val f: Foo\n§f.go()"), "none");
    assert_eq!(kotlin("§p.go()"), "none", "a parameter");
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
    assert_eq!(role("(a + b).fo§o();"), "foo Call Some(Other)");
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
    assert_eq!(role("(a + b).fo§o()"), "foo Call Some(Other)");
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
    assert_eq!(find("val made = Foo()\nmad§e.go()"), "4:5 None");
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

#[test]
fn enclosing_ranges_grow_one_node_at_a_time() {
    let source = "class A { void m() { int x = f(1, 2); } }";
    let buffer = Buffer::new(source);
    let tree = parse(Language::Java, &buffer, || false).unwrap().unwrap();
    let text = |range: dumbj_text::ByteRange| source[range.start.get()..range.end.get()].to_owned();
    let start = source.find("1, 2").unwrap();
    let one = ByteOffset(start)..ByteOffset(start + 1);
    let arguments = tree.enclosing_range(one).unwrap();
    assert_eq!(text(arguments.clone()), "(1, 2)");
    let call = tree.enclosing_range(arguments).unwrap();
    assert_eq!(text(call.clone()), "f(1, 2)");
    let whole = ByteOffset(0)..ByteOffset(source.len());
    assert_eq!(tree.enclosing_range(whole), None);
}

#[test]
fn writes_are_assignment_targets() {
    let write = |language: Language, source: &str, at: &str| {
        let buffer = Buffer::new(source);
        let tree = parse(language, &buffer, || false).unwrap().unwrap();
        tree.is_write(ByteOffset(source.find(at).unwrap()))
    };
    let java = "class A { int x; void m(int y) { x = y; this.x += 1; x++; f(x); } }";
    assert!(write(Language::Java, java, "x = y"));
    assert!(!write(Language::Java, java, "y; this"));
    assert!(write(Language::Java, java, "x += 1"));
    assert!(write(Language::Java, java, "x++"));
    assert!(!write(Language::Java, java, "x); }"));
    let kotlin = "class A { var x = 0\n fun m(y: Int) { x = y\n this.x += 1\n x++\n f(x) } }";
    assert!(write(Language::Kotlin, kotlin, "x = y"));
    assert!(!write(Language::Kotlin, kotlin, "y\n this"));
    assert!(write(Language::Kotlin, kotlin, "x += 1"));
    assert!(write(Language::Kotlin, kotlin, "x++"));
    assert!(!write(Language::Kotlin, kotlin, "x) }"));
}
