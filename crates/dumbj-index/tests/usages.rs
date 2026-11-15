//! A `§` in a snippet marks the caret.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use common::{shop_index, shop_root};
use dumbj_index::{Certainty, Index, Site, find_usages, usage_target_at};
use dumbj_syntax::{Language, parse};
use dumbj_text::{Buffer, ByteOffset};

const CART: &str = "app/src/main/java/com/shop/app/Cart.java";
const CHECKOUT: &str = "app/src/main/kotlin/com/shop/app/Checkout.kt";
const MONEY: &str = "core/src/main/java/com/shop/core/Money.java";
const PRICED: &str = "core/src/main/java/com/shop/core/Priced.java";
const PRODUCT: &str = "core/src/main/kotlin/com/shop/core/Product.kt";

/// The usages of whatever is at the caret, as `File:line text` with `?` for a maybe.
fn usages(index: &Index, file: &str, snippet: &str) -> Vec<String> {
    usages_in(&shop_root(), index, file, snippet)
}

fn usages_in(root: &Path, index: &Index, file: &str, snippet: &str) -> Vec<String> {
    let path = root.join(file);
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
    let target = usage_target_at(site, ByteOffset(offset)).unwrap();
    find_usages(index, &target, &AtomicBool::new(false))
        .unwrap()
        .into_iter()
        .map(|usage| {
            let name = usage
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mark = if usage.certainty == Certainty::Maybe {
                " ?"
            } else {
                ""
            };
            format!("{name}:{} {}{mark}", usage.line + 1, usage.line_text)
        })
        .collect()
}

#[test]
fn usages_of_a_class_cross_languages_and_skip_the_namesake() {
    let index = shop_index();
    let found = usages(&index, MONEY, "public final class Mon§ey");
    assert_eq!(
        found,
        [
            "Cart.java:3 import com.shop.core.Money;",
            "Cart.java:11 private Money total = Money.ZERO;",
            "Cart.java:11 private Money total = Money.ZERO;",
            "Cart.java:15 Money cost = product.price();",
            "Cart.java:20 public Money price() {",
            "Checkout.kt:4 import com.shop.core.Money as Cash",
            "Checkout.kt:11 val total: Cash = cart.price()",
            "Checkout.kt:17 fun pay(amount: Int): Cash {",
            "Checkout.kt:18 val paid = Cash.of(amount)",
            "Money.java:4 public static final Money ZERO = new Money(0);",
            "Money.java:4 public static final Money ZERO = new Money(0);",
            "Money.java:11 public static Money of(int amount) {",
            "Money.java:12 return new Money(amount);",
            "Money.java:15 public Money add(Money other) {",
            "Money.java:15 public Money add(Money other) {",
            "Money.java:17 return new Money(sum);",
            "Priced.java:4 Money price();",
            "Receipt.java:6 public record Receipt(Money total, Status status) {",
            "Product.kt:3 data class Product(val name: String, val cost: Money) : Priced {",
            "Product.kt:4 override fun price(): Money = cost",
            "Product.kt:7 val reduced = Money.of(cost.amount() * (100 - percent) / 100)",
            "Product.kt:14 fun sample(): Product = Product(DEFAULT_NAME, Money.ZERO)",
            "Product.kt:20 fun formatMoney(money: Money): String = money.amount().toString()",
        ],
        "com.shop.util.Money and its uses are not included; the Kotlin alias is"
    );
}

#[test]
fn usages_of_a_method_from_a_reference() {
    let index = shop_index();
    assert_eq!(
        usages(&index, CART, "total = total.a§dd(cost);"),
        [
            "Cart.java:16 total = total.add(cost);",
            "Checkout.kt:19 return paid.add(cart.price())",
        ],
        "`paid` has the type its initialiser returns; `items.add(product)` does not pass a Money"
    );
    assert_eq!(
        usages(&index, PRODUCT, "fun sam§ple(): Product"),
        [
            "Cart.java:30 Product best = Product.Companion.sample();",
            "Checkout.kt:12 val first = Product.sample()"
        ]
    );
}

#[test]
fn usages_of_fields_and_properties() {
    let index = shop_index();
    assert_eq!(
        usages(&index, CART, "private Money tot§al"),
        [
            "Cart.java:16 total = total.add(cost);",
            "Cart.java:16 total = total.add(cost);",
            "Cart.java:21 return total;"
        ]
    );
    assert_eq!(
        usages(&index, PRODUCT, "val co§st: Money"),
        [
            "Product.kt:4 override fun price(): Money = cost",
            "Product.kt:7 val reduced = Money.of(cost.amount() * (100 - percent) / 100)",
            "Product.kt:18 fun Product.label(): String = \"$name: ${formatMoney(cost)}\"",
        ],
        "inside the extension function its receiver is the implicit `this`"
    );
}

#[test]
fn usages_of_locals_stay_in_their_scope() {
    let index = shop_index();
    assert_eq!(
        usages(&index, CART, "int cou§nt = products.size();"),
        ["Cart.java:26 return count;"],
        "not the count() method or its call"
    );
    assert_eq!(
        usages(&index, CART, "products.add(prod§uct);"),
        [
            "Cart.java:14 products.add(product);",
            "Cart.java:15 Money cost = product.price();"
        ],
        "from a usage of the parameter"
    );
    assert_eq!(
        usages(&index, CHECKOUT, "val pa§id = Cash.of(amount)"),
        ["Checkout.kt:19 return paid.add(cart.price())"]
    );
}

#[test]
fn interface_methods_and_cancellation() {
    let index = shop_index();
    let found = usages(&index, PRICED, "Money pri§ce();");
    assert!(
        found.contains(&"Priced.java:7 return price().amount() == 0;".to_owned()),
        "{found:?}"
    );

    let path = shop_root().join(MONEY);
    let source = std::fs::read_to_string(&path).unwrap();
    let buffer = Buffer::new(&source);
    let tree = parse(Language::Java, &buffer, || false).unwrap().unwrap();
    let site = Site {
        index: &index,
        file: index.file_by_path(&path).unwrap(),
        tree: &tree,
        buffer: &buffer,
    };
    let target =
        usage_target_at(site, ByteOffset(source.find("class Money").unwrap() + 7)).unwrap();
    assert_eq!(find_usages(&index, &target, &AtomicBool::new(true)), None);
    assert_eq!(
        usage_target_at(site, ByteOffset(0)),
        None,
        "a keyword has no usages"
    );
}

#[test]
fn usages_cross_the_accessor_boundary() {
    let index = common::freight_index();
    let found = usages_in(
        &common::freight_root(),
        &index,
        "src/main/kotlin/freight/Shipping.kt",
        "class NamedCarrier(val na§me: String)",
    );
    assert!(
        found
            .iter()
            .any(|usage| usage.starts_with("Interop.java:10")),
        "{found:?}"
    );
    let found = usages_in(
        &common::freight_root(),
        &index,
        "src/main/java/freight/app/Ledger.java",
        "public int getTo§tal()",
    );
    assert!(
        found.iter().any(|usage| usage.starts_with("Books.kt:3")),
        "{found:?}"
    );
}

#[test]
fn usages_in_one_file_for_highlighting() {
    use dumbj_index::usage_highlights;
    let index = shop_index();
    let path = shop_root().join(CART);
    let source = std::fs::read_to_string(&path).unwrap();
    let offset = source.find("total.add(cost)").unwrap() + 1;
    let buffer = Buffer::new(&source);
    let tree = parse(Language::Java, &buffer, || false).unwrap().unwrap();
    let site = Site {
        index: &index,
        file: index.file_by_path(&path).unwrap(),
        tree: &tree,
        buffer: &buffer,
    };
    let found: Vec<(usize, bool)> =
        usage_highlights(site, ByteOffset(offset), &AtomicBool::new(false))
            .into_iter()
            .map(|usage| {
                (
                    buffer.point_at(usage.range.start).line.get() + 1,
                    usage.write,
                )
            })
            .collect();
    assert_eq!(
        found,
        [(11, true), (16, true), (16, false), (21, false)],
        "the declaration and `total = …` write; `total.add` and `return total` read"
    );
}
