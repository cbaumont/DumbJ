//! Kotlin-specific tree walking. The grammar has almost no field names, so children are
//! found by kind and position.

use tree_sitter::Node;

use crate::structure::{
    Collector, Import, SymbolKind, all_children, child_of_kind, named_children,
};

/// The dotted name of a `user_type` without type arguments: `java.io.Serializable`.
fn user_type_name(collector: &Collector<'_>, user_type: Node<'_>) -> String {
    let parts: Vec<String> = named_children(user_type)
        .into_iter()
        .filter(|child| child.kind() == "type_identifier")
        .map(|part| collector.source.text(part))
        .collect();
    parts.join(".")
}

/// Finds the `user_type` naming a supertype or receiver, looking through wrappers such as
/// `constructor_invocation`, `nullable_type` and `receiver_type`.
fn find_user_type(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "user_type" {
        return Some(node);
    }
    named_children(node)
        .into_iter()
        .filter(|child| {
            matches!(
                child.kind(),
                "user_type"
                    | "constructor_invocation"
                    | "nullable_type"
                    | "explicit_delegation"
                    | "parenthesized_type"
                    | "type_modifiers"
            )
        })
        .find_map(find_user_type)
}

fn has_modifier(collector: &Collector<'_>, node: Node<'_>, modifier: &str) -> bool {
    child_of_kind(node, "modifiers").is_some_and(|modifiers| {
        named_children(modifiers)
            .into_iter()
            .any(|child| collector.source.text(child) == modifier)
    })
}

fn class_kind(collector: &Collector<'_>, node: Node<'_>) -> SymbolKind {
    // `interface` and `enum` are keyword tokens of the declaration; `annotation` is a modifier.
    let has_keyword = |keyword: &str| {
        all_children(node)
            .iter()
            .any(|child| child.kind() == keyword)
    };
    if has_keyword("interface") {
        SymbolKind::Interface
    } else if has_keyword("enum") || has_modifier(collector, node, "enum") {
        SymbolKind::Enum
    } else if has_modifier(collector, node, "annotation") {
        SymbolKind::Annotation
    } else {
        SymbolKind::Class
    }
}

fn receiver(collector: &Collector<'_>, node: Node<'_>) -> Option<String> {
    let receiver = child_of_kind(node, "receiver_type")?;
    find_user_type(receiver).map(|user_type| user_type_name(collector, user_type))
}

fn import(collector: &Collector<'_>, node: Node<'_>) -> Option<Import> {
    let path = child_of_kind(node, "identifier")?;
    let parts: Vec<String> = named_children(path)
        .into_iter()
        .map(|part| collector.source.text(part))
        .collect();
    let alias = child_of_kind(node, "import_alias")
        .and_then(|alias| alias.named_child(0))
        .map(|name| collector.source.text(name));
    Some(Import {
        path: parts.join("."),
        wildcard: child_of_kind(node, "wildcard_import").is_some(),
        is_static: false,
        alias,
    })
}

/// A class, interface, object or companion: its symbol, supertypes, constructor
/// properties and members.
fn collect_type(
    collector: &mut Collector<'_>,
    node: Node<'_>,
    kind: SymbolKind,
    parent: Option<usize>,
) {
    let name = child_of_kind(node, "type_identifier");
    let index = match (name, kind) {
        (Some(name), _) => collector.push(kind, node, name, parent),
        // An unnamed companion is called `Companion`; navigation lands on the keyword.
        (None, SymbolKind::Companion) => {
            let index = collector.push(kind, node, node, parent);
            if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                "Companion".clone_into(&mut symbol.name);
                symbol.name_range.end = symbol.name_range.start;
            }
            index
        }
        (None, _) => return,
    };
    let supers: Vec<String> = named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "delegation_specifier")
        .filter_map(find_user_type)
        .map(|user_type| user_type_name(collector, user_type))
        .collect();
    if let Some(symbol) = collector.facts.symbols.get_mut(index) {
        symbol.supertypes = supers;
    }
    for child in named_children(node) {
        match child.kind() {
            "primary_constructor" => {
                for parameter in named_children(child) {
                    // Only `val`/`var` parameters declare properties.
                    let is_property = child_of_kind(parameter, "binding_pattern_kind").is_some();
                    let name = child_of_kind(parameter, "simple_identifier");
                    if let Some(name) = name.filter(|_| is_property) {
                        collector.push(SymbolKind::Property, parameter, name, Some(index));
                    }
                }
            }
            "class_body" | "enum_class_body" => collect(collector, child, Some(index)),
            _ => {}
        }
    }
}

/// Walks `node` and records the declarations below it.
pub(crate) fn collect(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>) {
    match node.kind() {
        "package_header" => {
            collector.facts.package = child_of_kind(node, "identifier").map(|path| {
                let parts: Vec<String> = named_children(path)
                    .into_iter()
                    .map(|part| collector.source.text(part))
                    .collect();
                parts.join(".")
            });
        }
        "import_header" => {
            if let Some(import) = import(collector, node) {
                collector.facts.imports.push(import);
            }
        }
        "class_declaration" => {
            let kind = class_kind(collector, node);
            collect_type(collector, node, kind, parent);
        }
        "object_declaration" => collect_type(collector, node, SymbolKind::Object, parent),
        "companion_object" => collect_type(collector, node, SymbolKind::Companion, parent),
        "type_alias" => {
            if let Some(name) = child_of_kind(node, "type_identifier") {
                collector.push(SymbolKind::TypeAlias, node, name, parent);
            }
        }
        "function_declaration" => {
            if let Some(name) = child_of_kind(node, "simple_identifier") {
                let kind = if parent.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                let index = collector.push(kind, node, name, parent);
                let receiver = receiver(collector, node);
                if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                    symbol.receiver = receiver;
                }
            }
        }
        "property_declaration" => {
            let receiver = receiver(collector, node);
            let declarations =
                named_children(node)
                    .into_iter()
                    .flat_map(|child| match child.kind() {
                        "variable_declaration" => vec![child],
                        "multi_variable_declaration" => named_children(child),
                        _ => Vec::new(),
                    });
            for declaration in declarations.collect::<Vec<_>>() {
                if let Some(name) = child_of_kind(declaration, "simple_identifier") {
                    let index = collector.push(SymbolKind::Property, node, name, parent);
                    if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                        symbol.receiver.clone_from(&receiver);
                    }
                }
            }
        }
        "enum_entry" => {
            if let Some(name) = child_of_kind(node, "simple_identifier") {
                collector.push(SymbolKind::EnumConstant, node, name, parent);
            }
        }
        "secondary_constructor" => {
            // Lands on the `constructor` keyword; named after the class like in Java.
            let class_name = parent
                .and_then(|index| collector.facts.symbols.get(index))
                .map(|s| s.name.clone());
            if let Some(class_name) = class_name {
                let index = collector.push(SymbolKind::Constructor, node, node, parent);
                if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                    symbol.name = class_name;
                    symbol.name_range.end = symbol.name_range.start;
                }
            }
        }
        // Containers of declarations, and ERROR nodes so a syntax error hides as little
        // as possible. Function bodies are not entered.
        "source_file" | "import_list" | "class_body" | "enum_class_body" | "ERROR" => {
            for child in named_children(node) {
                collect(collector, child, parent);
            }
        }
        _ => {}
    }
}
