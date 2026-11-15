//! Go to Declaration against the `shop` fixture project. A `§` in a snippet marks the
//! caret; the snippet (without the marker) must occur exactly once in the file.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use std::fmt::Write as _;

use common::{shop_index, shop_root};
use dumbj_index::{Decision, Index, Site, resolve};
use dumbj_syntax::{Language, parse};
use dumbj_text::{Buffer, ByteOffset};

const CART: &str = "app/src/main/java/com/shop/app/Cart.java";
const CHECKOUT: &str = "app/src/main/kotlin/com/shop/app/Checkout.kt";
const MONEY: &str = "core/src/main/java/com/shop/core/Money.java";
const PRICED: &str = "core/src/main/java/com/shop/core/Priced.java";
const RECEIPT: &str = "core/src/main/java/com/shop/core/Receipt.java";
const PRODUCT: &str = "core/src/main/kotlin/com/shop/core/Product.kt";

/// Resolves the caret in `snippet` and renders the outcome:
/// `Jump file:line name` or `Choose [file:line name, …]` or `NotFound`.
fn go(index: &Index, file: &str, snippet: &str) -> String {
    let path = shop_root().join(file);
    let source = std::fs::read_to_string(&path).unwrap();
    let plain = snippet.replacen('§', "", 1);
    assert_eq!(
        source.matches(&plain).count(),
        1,
        "`{plain}` must occur once in {file}"
    );
    let offset = source.find(&plain).unwrap() + snippet.find('§').unwrap();
    let buffer = Buffer::new(&source);
    let tree = parse(Language::from_path(&path).unwrap(), &buffer, || false)
        .unwrap()
        .unwrap();
    let site = Site {
        index,
        file: index.file_by_path(&path).unwrap(),
        tree: &tree,
        buffer: &buffer,
    };
    let Some(resolution) = resolve(site, ByteOffset(offset)) else {
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
fn extensions_get_a_chooser() {
    let index = shop_index();
    // An extension function is never a sure thing, even with a single candidate.
    assert_eq!(
        go(&index, CHECKOUT, "first.lab§el()"),
        "Choose [Product.kt:18 label ExplicitImport]"
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
