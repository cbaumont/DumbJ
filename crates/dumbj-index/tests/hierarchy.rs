#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

mod common;

use std::sync::atomic::AtomicBool;

use common::freight_index;
use dumbj_index::{
    Index, SymbolId, callers, direct_subtypes, direct_supertypes, gutter_marks, implementations,
    overriders, super_members, superclass,
};

fn named(index: &Index, qualified: &str) -> SymbolId {
    index
        .symbols()
        .map(|(id, _)| id)
        .find(|id| index.qualified_name(*id) == qualified)
        .unwrap()
}

fn names(index: &Index, ids: &[SymbolId]) -> Vec<String> {
    ids.iter().map(|id| index.qualified_name(*id)).collect()
}

#[test]
fn types_and_their_subtypes() {
    let index = freight_index();
    let carrier = named(&index, "Carrier");
    assert_eq!(
        names(&index, &direct_subtypes(&index, carrier)),
        ["NamedCarrier"]
    );
    assert_eq!(
        names(&index, &direct_supertypes(&index, named(&index, "Crate"))),
        ["Parcel"]
    );
    assert_eq!(
        names(&index, &implementations(&index, carrier)),
        ["NamedCarrier"]
    );
}

#[test]
fn the_superclass_is_the_supertype_that_is_not_an_interface() {
    let index = freight_index();
    let pallet = named(&index, "Pallet");
    assert_eq!(
        names(&index, &direct_supertypes(&index, pallet)),
        ["Stackable", "Parcel"]
    );
    assert_eq!(
        superclass(&index, pallet).map(|id| index.qualified_name(id)),
        Some("Parcel".to_owned())
    );
    assert_eq!(superclass(&index, named(&index, "NamedCarrier")), None);
}

#[test]
fn members_override_by_name_and_signature() {
    let index = freight_index();
    let base = named(&index, "Carrier.quote");
    let implementation = named(&index, "NamedCarrier.quote");
    assert_eq!(
        names(&index, &overriders(&index, base)),
        ["NamedCarrier.quote"]
    );
    assert_eq!(
        names(&index, &super_members(&index, implementation)),
        ["Carrier.quote"]
    );
    let crate_send = named(&index, "Crate.send");
    assert!(
        super_members(&index, crate_send).is_empty(),
        "an overload with more parameters does not override"
    );
}

#[test]
fn gutter_marks_both_sides() {
    let index = freight_index();
    let carrier = named(&index, "Carrier");
    let marks = gutter_marks(&index, carrier.file);
    let marked = |qualified: &str| {
        let id = named(&index, qualified);
        marks
            .iter()
            .find(|mark| mark.symbol == id)
            .map(|mark| (mark.overrides, mark.overridden))
    };
    assert_eq!(marked("Carrier"), Some((false, true)));
    assert_eq!(marked("Carrier.quote"), Some((false, true)));
    assert_eq!(marked("NamedCarrier.quote"), Some((true, false)));
    assert_eq!(marked("Crate.send"), None);
}

#[test]
fn callers_are_grouped_by_enclosing_declaration() {
    let index = freight_index();
    let price = named(&index, "Product.price");
    let found = callers(&index, price, &AtomicBool::new(false)).unwrap();
    let mut grouped: Vec<(String, usize)> = found
        .iter()
        .map(|caller| {
            let name = caller
                .symbol
                .map_or_else(String::new, |id| index.qualified_name(id));
            (name, caller.calls.len())
        })
        .collect();
    grouped.sort();
    assert_eq!(
        grouped,
        [
            ("Desk.work".to_owned(), 1),
            ("NamedCarrier.quote".to_owned(), 1),
            ("Shipping.demo".to_owned(), 1),
            ("Shipping.rate".to_owned(), 1),
        ]
    );
}
