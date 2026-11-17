//! Go to Declaration against the `shop` and `freight` fixture projects. A `§` in a snippet marks the
//! caret; the snippet (without the marker) must occur exactly once in the file.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use std::fmt::Write as _;

use std::path::Path;

use common::{
    CART, CHECKOUT, MONEY, PRICED, PRODUCT, RECEIPT, caret_in, freight_index, freight_root,
    shop_index, shop_root,
};
use dumbj_index::{Decision, Index, Site, resolve};
use dumbj_syntax::{Language, parse};
use dumbj_text::{Buffer, ByteOffset};

const SHIPPING: &str = "src/main/kotlin/freight/Shipping.kt";
const DISPATCH: &str = "src/main/java/freight/app/Dispatch.java";

/// Resolves the caret in `snippet` and renders the outcome:
/// `Jump file:line name` or `Choose [file:line name, …]` or `NotFound`.
fn go(index: &Index, file: &str, snippet: &str) -> String {
    go_in(&shop_root(), index, file, snippet)
}

/// Like [`go`], in the project at `root`.
fn go_in(root: &Path, index: &Index, file: &str, snippet: &str) -> String {
    let caret = caret_in(root, file, snippet);
    let site = caret.site(index);
    let Some(resolution) = resolve(site, caret.offset) else {
        return "none".to_owned();
    };
    let describe = |target: &dumbj_index::Target| {
        let text = std::fs::read_to_string(&target.path).unwrap();
        let line = text[..target.range.start.get()].matches('\n').count() + 1;
        let name = target.symbol.map_or_else(
            || text[target.range.start.get()..target.range.end.get()].to_owned(),
            |id| index.qualified_name(id),
        );
        let file = target
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        format!("{file}:{line} {name} {:?}", target.tier)
    };
    match resolution.decision {
        Decision::NotFound => "NotFound".to_owned(),
        Decision::Jump => format!("Jump {}", describe(&resolution.targets[0])),
        Decision::Choose => {
            let mut out = String::from("Choose");
            for target in &resolution.targets {
                write!(out, " [{}]", describe(target)).unwrap();
            }
            out
        }
    }
}

#[test]
fn locals_and_parameters_win() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "products.add(pro§duct);"),
        "Jump Cart.java:13 product Local"
    );
    assert_eq!(
        go(&index, CART, "total.add(co§st);"),
        "Jump Cart.java:15 cost Local"
    );
    assert_eq!(
        go(&index, CART, "return cou§nt;"),
        "Jump Cart.java:25 count Local",
        "the local, not the method"
    );
    assert_eq!(
        go(&index, CART, "best = candi§date;"),
        "Jump Cart.java:31 candidate Local"
    );
    assert_eq!(
        go(&index, MONEY, "return new Money(su§m);"),
        "Jump Money.java:16 sum Local"
    );
    assert_eq!(
        go(&index, PRODUCT, "Product(name, redu§ced)"),
        "Jump Product.kt:7 reduced Local"
    );
    assert_eq!(
        go(&index, CHECKOUT, "Cash.of(amo§unt)"),
        "Jump Checkout.kt:17 amount Local"
    );
}

#[test]
fn members_of_the_enclosing_type() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "return tot§al;"),
        "Jump Cart.java:11 Cart.total Scope"
    );
    assert_eq!(
        go(&index, CART, "pro§ducts.add(product);"),
        "Jump Cart.java:10 Cart.products Scope"
    );
    assert_eq!(
        go(&index, CART, "this.cou§nt() == 0"),
        "Jump Cart.java:24 Cart.count Scope"
    );
    assert_eq!(
        go(&index, MONEY, "this.amo§unt = amount;"),
        "Jump Money.java:5 Money.amount Scope"
    );
    assert_eq!(
        go(&index, MONEY, "int sum = amo§unt + other.amount;"),
        "Jump Money.java:5 Money.amount Scope"
    );
    assert_eq!(
        go(&index, PRODUCT, "override fun price(): Money = co§st"),
        "Jump Product.kt:3 Product.cost Scope"
    );
    assert_eq!(
        go(&index, PRODUCT, "Product(DEFAULT_NA§ME, Money.ZERO)"),
        "Jump Product.kt:12 Product.Companion.DEFAULT_NAME Scope"
    );
}

#[test]
fn inherited_members_resolve_through_supertypes() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "&& isFr§ee();"),
        "Jump Priced.java:6 Priced.isFree Scope",
        "default method of the implemented interface"
    );
    assert_eq!(
        go(&index, PRICED, "return pri§ce().amount() == 0;"),
        "Jump Priced.java:4 Priced.price Scope"
    );
}

#[test]
fn types_follow_imports_and_packages() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "private Mon§ey total"),
        "Jump Money.java:3 Money ExplicitImport",
        "com.shop.core.Money is imported; com.shop.util.Money is not"
    );
    assert_eq!(
        go(&index, CART, "public void add(Prod§uct product)"),
        "Jump Product.kt:3 Product ExplicitImport"
    );
    assert_eq!(
        go(&index, CART, "implements Pri§ced"),
        "Jump Priced.java:3 Priced ExplicitImport"
    );
    assert_eq!(
        go(&index, PRICED, "Mon§ey price();"),
        "Jump Money.java:3 Money SamePackage"
    );
    assert_eq!(
        go(&index, PRODUCT, "val cost: Mon§ey)"),
        "Jump Money.java:3 Money SamePackage"
    );
    assert_eq!(
        go(&index, CHECKOUT, "private val cart: Ca§rt"),
        "Jump Cart.java:9 Cart SamePackage"
    );
    assert_eq!(
        go(&index, CHECKOUT, "val total: Ca§sh ="),
        "Jump Money.java:3 Money ExplicitImport",
        "import alias"
    );
    assert_eq!(
        go(&index, MONEY, "return new Mon§ey(amount);"),
        "Jump Money.java:3 Money SameFile"
    );
}

#[test]
fn import_statements_resolve_by_fully_qualified_name() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "import com.shop.core.Mon§ey;"),
        "Jump Money.java:3 Money ExplicitImport"
    );
    assert_eq!(
        go(&index, CHECKOUT, "import com.shop.core.formatMon§ey"),
        "Jump Product.kt:20 formatMoney ExplicitImport"
    );
    assert_eq!(
        go(&index, CHECKOUT, "import com.shop.core.lab§el"),
        "Jump Product.kt:18 label ExplicitImport"
    );
    assert_eq!(
        go(&index, CART, "import java.util.Li§st;"),
        "NotFound",
        "library types are not indexed yet"
    );
}

#[test]
fn static_and_companion_members_through_a_type_name() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "Money.ZE§RO;"),
        "Jump Money.java:4 Money.ZERO Scope"
    );
    assert_eq!(
        go(&index, PRODUCT, "Money.o§f(cost"),
        "Jump Money.java:11 Money.of Scope",
        "not com.shop.util.Money.of"
    );
    assert_eq!(
        go(&index, CHECKOUT, "Cash.o§f(amount)"),
        "Jump Money.java:11 Money.of Scope",
        "through the alias"
    );
    assert_eq!(
        go(&index, CHECKOUT, "Product.sam§ple()"),
        "Jump Product.kt:14 Product.Companion.sample Scope"
    );
    assert_eq!(
        go(&index, CART, "Product.Companion.sam§ple()"),
        "Jump Product.kt:14 Product.Companion.sample Scope"
    );
    assert_eq!(
        go(&index, CHECKOUT, "Catalog.a§dd(first)"),
        "Jump Product.kt:25 Catalog.add Scope"
    );
}

#[test]
fn members_through_a_receiver_with_a_known_type() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "product.pri§ce();"),
        "Jump Product.kt:4 Product.price Scope",
        "typed parameter"
    );
    assert_eq!(
        go(&index, CART, "total = total.a§dd(cost);"),
        "Jump Money.java:15 Money.add Scope",
        "typed field"
    );
    assert_eq!(
        go(&index, MONEY, "other.amo§unt;"),
        "Jump Money.java:5 Money.amount Scope"
    );
    assert_eq!(
        go(&index, PRODUCT, "cost.amo§unt() *"),
        "Jump Money.java:20 Money.amount Scope",
        "typed property"
    );
    assert_eq!(
        go(&index, CHECKOUT, "val total: Cash = cart.pri§ce()"),
        "Jump Cart.java:20 Cart.price Scope"
    );
    assert_eq!(
        go(&index, CHECKOUT, "return paid.a§dd("),
        "Jump Money.java:15 Money.add Scope",
        "`paid = Cash.of(…)` has the type `of` returns"
    );
    assert_eq!(
        go(&index, CART, "candidate.price().amo§unt() <"),
        "Jump Money.java:20 Money.amount Scope",
        "a call result has the callee's return type"
    );
}

#[test]
fn top_level_functions_and_extensions() {
    let index = shop_index();
    assert_eq!(
        go(&index, CHECKOUT, "first.lab§el()"),
        "Jump Product.kt:18 label ExplicitImport",
        "`first` is a Product and the extension on Product is imported"
    );
    assert_eq!(
        go(&index, PRODUCT, "${formatMon§ey(cost)}"),
        "Jump Product.kt:20 formatMoney SameFile"
    );
    assert_eq!(
        go(&index, CHECKOUT, "return formatMon§ey(total)"),
        "Jump Product.kt:20 formatMoney ExplicitImport"
    );
}

#[test]
fn nothing_to_resolve() {
    let index = shop_index();
    assert_eq!(
        go(&index, CART, "public void a§dd(Product product)"),
        "NotFound",
        "already on the declaration"
    );
    assert_eq!(
        go(&index, CART, "products.si§ze();"),
        "NotFound",
        "java.util.List is a library type"
    );
    assert_eq!(go(&index, CART, "new Array§List<>()"), "NotFound");
    assert_eq!(go(&index, CART, "pub§lic class Cart"), "none", "a keyword");
}

#[test]
fn record_components_are_accessors() {
    let index = shop_index();
    assert_eq!(
        go(&index, RECEIPT, "int amount = receipt.tot§al().amount();"),
        "Jump Receipt.java:6 Receipt.total Scope"
    );
    assert_eq!(
        go(&index, RECEIPT, "String label = receipt.stat§us().name();"),
        "Jump Receipt.java:6 Receipt.status Scope"
    );
}

#[test]
fn members_inherited_from_library_supertypes_are_not_guessed() {
    let index = shop_index();
    assert_eq!(
        go(&index, RECEIPT, "archive.a§dd(receipt);"),
        "NotFound",
        "Archive extends java.util.List; Money.add and Cart.add are unrelated"
    );
    assert_eq!(
        go(&index, RECEIPT, "Receipt late§st();"),
        "NotFound",
        "a declaration"
    );
}

#[test]
fn overloads_are_picked_by_their_arguments() {
    let index = freight_index();
    let go = |snippet: &str| go_in(&freight_root(), &index, SHIPPING, snippet);
    assert_eq!(
        go("ra§te(product)\n"),
        "Jump Shipping.kt:34 Shipping.rate Scope"
    );
    assert_eq!(
        go("ra§te(product.price())"),
        "Jump Shipping.kt:36 Shipping.rate Scope",
        "the argument is a call result"
    );
    assert_eq!(
        go("ra§te(\"EU\")\n"),
        "Jump Shipping.kt:38 Shipping.rate Scope",
        "a default value may be left out"
    );
    assert_eq!(
        go("ra§te(\"EU\", express = true)"),
        "Jump Shipping.kt:38 Shipping.rate Scope"
    );
    assert_eq!(
        go("ra§te(product, product, discount = 5)"),
        "Jump Shipping.kt:40 Shipping.rate Scope",
        "vararg and a named argument"
    );
    assert_eq!(
        go("ra§te(unknown())"),
        "Choose [Shipping.kt:34 Shipping.rate Scope] [Shipping.kt:36 Shipping.rate Scope] \
         [Shipping.kt:38 Shipping.rate Scope]",
        "an argument of unknown type only rules out by count"
    );
    assert_eq!(
        go("tra§ck(\"a\") {"),
        "Jump Shipping.kt:42 Shipping.track Scope",
        "a trailing lambda is not a Parcel"
    );
    assert_eq!(
        go("tra§ck(\"b\", crate)"),
        "Choose [Shipping.kt:42 Shipping.track Scope] [Shipping.kt:44 Shipping.track Scope]",
        "a function type is not recorded, so it cannot be ruled out"
    );
    assert_eq!(
        go("crate.se§nd(\"home\")"),
        "Jump Shipping.kt:24 Parcel.send Scope",
        "the overload that fits is inherited"
    );
    assert_eq!(
        go("crate.se§nd(\"away\", free)"),
        "Jump Shipping.kt:28 Crate.send Scope"
    );
}

#[test]
fn kotlin_calls_to_types_reach_constructors_and_factory_functions() {
    let index = freight_index();
    let go = |snippet: &str| go_in(&freight_root(), &index, SHIPPING, snippet);
    assert_eq!(
        go("Carr§ier(\"fast\")"),
        "Jump Shipping.kt:15 Carrier SameFile",
        "an interface cannot be called; the function of the same name can"
    );
    assert_eq!(
        go("Carrier(\"fast\").quo§te(product)"),
        "Jump Shipping.kt:12 Carrier.quote Scope"
    );
    assert_eq!(
        go("NamedCarr§ier(\"slow\", free)"),
        "Jump Shipping.kt:18 NamedCarrier.NamedCarrier SameFile",
        "the secondary constructor"
    );
    assert_eq!(
        go("NamedCarr§ier(\"slower\")"),
        "Jump Shipping.kt:17 NamedCarrier SameFile",
        "the primary constructor is the class"
    );
    assert_eq!(
        go("NamedCarrier(\"slow\", free).na§me"),
        "Jump Shipping.kt:17 NamedCarrier.name Scope",
        "a constructor call has the type of its class"
    );
}

#[test]
fn java_calls_kotlin_with_the_receiver_as_an_argument() {
    let index = freight_index();
    let go = |snippet: &str| go_in(&freight_root(), &index, DISPATCH, snippet);
    assert_eq!(
        go("ShippingKt.lab§el(product)"),
        "Jump Shipping.kt:9 label Scope",
        "`ShippingKt` is the facade class of Shipping.kt"
    );
    assert_eq!(
        go("Shipping.INSTANCE.ra§te(label, true)"),
        "Jump Shipping.kt:38 Shipping.rate Scope",
        "`INSTANCE` is the object itself; the String, Boolean overload"
    );
}

#[test]
fn kotlin_extensions_and_their_receivers() {
    let index = freight_index();
    let go = |snippet: &str| go_in(&freight_root(), &index, SHIPPING, snippet);
    assert_eq!(
        go("= na§me + this"),
        "Jump Shipping.kt:17 NamedCarrier.name Scope",
        "the members of the extension receiver are in scope"
    );
    assert_eq!(
        go("this.quo§te(sample())"),
        "Jump Shipping.kt:20 NamedCarrier.quote Scope",
        "`this` is the extension receiver"
    );
    assert_eq!(
        go("= quo§te(product).amount"),
        "Jump Shipping.kt:12 Carrier.quote Scope"
    );
    assert_eq!(
        go("carrier.descr§ibe()"),
        "Jump Shipping.kt:65 describe SameFile"
    );
    assert_eq!(
        go("carrier.che§ap(sample())"),
        "Jump Shipping.kt:67 cheap SameFile",
        "an extension on a supertype of the receiver"
    );
    assert_eq!(
        go("text.sho§ut()"),
        "Jump Shipping.kt:69 shout SameFile",
        "an extension on a library type"
    );
    assert_eq!(
        go("text.che§ap(sample())"),
        "NotFound",
        "the only `cheap` is an extension on another type"
    );
    assert_eq!(
        go_in(
            &freight_root(),
            &index,
            "src/main/kotlin/freight/app/Labels.kt",
            "product.lab§el()"
        ),
        "Choose [Shipping.kt:9 label SameModule]",
        "an extension the file neither imports nor shares a package with is a guess"
    );
}

#[test]
fn kotlin_multiplatform_pairs_are_one_target() {
    let index = freight_index();
    let go =
        |file: &str, snippet: &str| go_in(&freight_root(), &index, &format!("kmp/{file}"), snippet);
    assert_eq!(
        go("common/src/Clock.kt", "+ no§w() +"),
        "Jump Clock.kt:3 now SameFile",
        "common code sees the `expect` declaration"
    );
    assert_eq!(
        go("common/src/Clock.kt", "no§w(\"utc\")"),
        "Jump Clock.kt:5 now SameFile",
        "pairs are matched by signature"
    );
    let jvm_name = go("jvm/src/Report.kt", "Zone.na§me");
    assert_eq!(jvm_name, "Jump Clock.kt:8 Zone.name Scope");
    assert_eq!(
        go("jvm/src/Report.kt", "no§w(\"local\")"),
        "Jump Clock.kt:5 now SamePackage",
        "a file in a platform source set sees that platform's `actual`"
    );
    // Both land in a file called Clock.kt; make sure they are different files.
    let target = |file: &str, snippet: &str| {
        let path = freight_root().join("kmp").join(file);
        let source = std::fs::read_to_string(&path).unwrap();
        let plain = snippet.replacen('§', "", 1);
        let offset = source.find(&plain).unwrap() + snippet.find('§').unwrap();
        let buffer = Buffer::new(&source);
        let tree = parse(Language::Kotlin, &buffer, || false).unwrap().unwrap();
        let site = Site {
            index: &index,
            file: index.file_by_path(&path).unwrap(),
            tree: &tree,
            buffer: &buffer,
        };
        let resolution = resolve(site, ByteOffset(offset)).unwrap();
        assert_eq!(resolution.decision, Decision::Jump);
        resolution.targets[0].path.clone()
    };
    assert!(target("common/src/Clock.kt", "+ no§w() +").ends_with("common/src/Clock.kt"));
    assert!(target("jvm/src/Report.kt", "no§w(\"local\")").ends_with("jvm/src/Clock.kt"));
    assert!(target("jvm/src/Report.kt", "Zone.na§me").ends_with("jvm/src/Clock.kt"));
}

#[test]
fn receivers_of_unknown_type_are_guesses() {
    let index = freight_index();
    assert_eq!(
        go_in(
            &freight_root(),
            &index,
            "src/main/java/freight/app/Invoice.java",
            "each.tot§al()"
        ),
        "Choose [Invoice.java:5 Invoice.total Scope]",
        "an untyped lambda parameter; a record component is its accessor"
    );
    assert_eq!(
        go_in(&freight_root(), &index, SHIPPING, "state.amo§unt else"),
        "Choose [Shipping.kt:3 Money.amount SameFile]",
        "`Any` has no `amount`, so `state` was smart cast to something"
    );
}

#[test]
fn java_and_kotlin_see_each_others_accessors() {
    let index = freight_index();
    let java = |snippet: &str| {
        go_in(
            &freight_root(),
            &index,
            "src/main/java/freight/app/Interop.java",
            snippet,
        )
    };
    assert_eq!(
        java("carrier.getNa§me()"),
        "Jump Shipping.kt:17 NamedCarrier.name Scope",
        "a Kotlin property's getter"
    );
    assert_eq!(
        java("money.getAmo§unt()"),
        "Jump Shipping.kt:3 Money.amount Scope"
    );
    assert_eq!(
        java("Shipping.INSTANCE.getFr§ee()"),
        "Jump Shipping.kt:32 Shipping.free Scope"
    );
    assert_eq!(
        java("Shipping.INSTA§NCE.getFree()"),
        "Jump Shipping.kt:31 Shipping Scope",
        "`INSTANCE` is the object"
    );
    assert_eq!(
        java("ShippingKt.sample().getCost().getAmo§unt()"),
        "Jump Shipping.kt:3 Money.amount Scope",
        "a chain through a facade function and getters"
    );
    let kotlin = |snippet: &str| {
        go_in(
            &freight_root(),
            &index,
            "src/main/kotlin/freight/app/Books.kt",
            snippet,
        )
    };
    assert_eq!(
        kotlin("ledger.isO§pen"),
        "Jump Ledger.java:8 Ledger.isOpen Scope",
        "a Java `isX()` read as a Kotlin property"
    );
    assert_eq!(
        kotlin("ledger.tot§al"),
        "Jump Ledger.java:4 Ledger.getTotal Scope"
    );
}

#[test]
fn references_outside_the_project_name_library_classes() {
    use dumbj_index::{LibraryTarget, library_targets};
    let index = shop_index();
    let targets = |file: &str, snippet: &str| {
        let path = shop_root().join(file);
        let source = std::fs::read_to_string(&path).unwrap();
        let plain = snippet.replacen('§', "", 1);
        let offset = source.find(&plain).unwrap() + snippet.find('§').unwrap();
        let buffer = Buffer::new(&source);
        let tree = parse(Language::from_path(&path).unwrap(), &buffer, || false)
            .unwrap()
            .unwrap();
        let site = Site {
            index: &index,
            file: index.file_by_path(&path).unwrap(),
            tree: &tree,
            buffer: &buffer,
        };
        library_targets(site, ByteOffset(offset))
    };
    let target = |class: &str, member: Option<&str>| LibraryTarget {
        class: class.to_owned(),
        member: member.map(str::to_owned),
    };
    assert_eq!(
        targets(CART, "import java.util.Arr§ayList;"),
        [
            target("java.util.ArrayList", None),
            target("java.util", Some("ArrayList"))
        ]
    );
    assert_eq!(
        targets(CART, "new Array§List<>()"),
        [target("java.util.ArrayList", None)]
    );
    assert_eq!(
        targets(CART, "products.ad§d(product);"),
        [target("java.util.List", Some("add"))],
        "a member of a field's library type"
    );
    assert_eq!(
        targets(CART, "products.si§ze();"),
        [target("java.util.List", Some("size"))]
    );
}

#[test]
fn java_sees_jvm_names() {
    let index = freight_index();
    let go = |snippet: &str| {
        go_in(
            &freight_root(),
            &index,
            "src/main/java/freight/app/Tariffs.java",
            snippet,
        )
    };
    assert_eq!(
        go("RateTables.standard§Rate()"),
        "Jump Rates.kt:6 standard Scope",
        "@file:JvmName names the facade, @JvmName the function"
    );
    assert_eq!(go("RateTables.isCh§eap()"), "Jump Rates.kt:9 cheap Scope");
    assert_eq!(
        go("tariff.isAct§ive()"),
        "Jump Rates.kt:13 Tariff.active Scope"
    );
}

#[test]
fn gradle_scripts_reach_the_version_catalog() {
    let index = freight_index();
    let go = |snippet: &str| go_in(&freight_root(), &index, "build.gradle.kts", snippet);
    assert_eq!(
        go("libs.kotlinx.coroutines.co§re)"),
        "Jump libs.versions.toml:5 libraries.kotlinx-coroutines-core Scope"
    );
    assert_eq!(
        go("libs.ok§io)"),
        "Jump libs.versions.toml:7 libraries.okio Scope"
    );
    assert_eq!(
        go("libs.plugins.kotlin.j§vm)"),
        "Jump libs.versions.toml:10 plugins.kotlin-jvm Scope"
    );
    assert_eq!(
        go("libs.versions.kot§lin.get()"),
        "Jump libs.versions.toml:2 versions.kotlin Scope"
    );
    assert_eq!(
        go("testImplementation(libs.kotlinx.corou§tines.test)"),
        "Choose [libs.versions.toml:5 libraries.kotlinx-coroutines-core Scope] \
         [libs.versions.toml:6 libraries.kotlinx-coroutines-test Scope]",
        "a partial accessor offers the entries under it"
    );
}

#[test]
fn kotlin_scope_functions() {
    let index = freight_index();
    let go = |snippet: &str| {
        go_in(
            &freight_root(),
            &index,
            "src/main/kotlin/freight/Scopes.kt",
            snippet,
        )
    };
    assert_eq!(
        go("let { it.tot§al.amount }"),
        "Jump Scopes.kt:3 Ledger2.total Scope",
        "`it` in `let` is the receiver"
    );
    assert_eq!(
        go("it.total.amo§unt }"),
        "Jump Shipping.kt:3 Money.amount Scope"
    );
    assert_eq!(
        go("also { it.quo§te(sample()) }"),
        "Jump Shipping.kt:20 NamedCarrier.quote Scope"
    );
    assert_eq!(
        go("apply { clo§se() }"),
        "Jump Scopes.kt:4 Ledger2.close Scope",
        "the `apply` receiver comes before the enclosing class"
    );
    assert_eq!(
        go("describe() + na§me }"),
        "Jump Shipping.kt:17 NamedCarrier.name Scope",
        "`with(x)` makes x the receiver"
    );
    assert_eq!(
        go("forEach { it.na§me }"),
        "Jump Shipping.kt:17 NamedCarrier.name Scope",
        "`listOf(x)` holds elements of x's type"
    );
    assert_eq!(
        go("filter { it.co§st.amount"),
        "Jump Shipping.kt:5 Product.cost Scope",
        "`it` in `filter` is an element of a `List<Product>`"
    );
    assert_eq!(
        go("map { p -> p.pri§ce() }"),
        "Jump Shipping.kt:6 Product.price Scope",
        "a named parameter, through an element-preserving `filter`"
    );
    assert_eq!(
        go("catalogue().first().lab§el()"),
        "Jump Shipping.kt:9 label SamePackage",
        "the element type of a function's return type"
    );
}
