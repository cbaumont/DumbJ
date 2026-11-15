//! Kotlin-specific tree walking. The grammar has almost no field names, so children are
//! found by kind and position.

use tree_sitter::Node;

use crate::reference::{Binder, Qualifier, ReferenceRole, qualifier_from_text};
use crate::structure::{
    Collector, Import, Source, SymbolKind, all_children, child_of_kind, named_children,
};

/// The dotted name of a `user_type` without type arguments: `java.io.Serializable`.
fn user_type_name(source: &Source<'_>, user_type: Node<'_>) -> String {
    let parts: Vec<String> = named_children(user_type)
        .into_iter()
        .filter(|child| child.kind() == "type_identifier")
        .map(|part| source.text(part))
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
    find_user_type(receiver).map(|user_type| user_type_name(&collector.source, user_type))
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
        .map(|user_type| user_type_name(&collector.source, user_type))
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
                        let property =
                            collector.push(SymbolKind::Property, parameter, name, Some(index));
                        let type_name = declared_type(&collector.source, parameter);
                        collector.set_type(property, type_name);
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
                // The return type is the type written directly in the declaration; the
                // receiver's type sits inside `receiver_type` and is not picked up here.
                let type_name = declared_type(&collector.source, node);
                if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                    symbol.receiver = receiver;
                    symbol.type_name = type_name;
                }
            }
        }
        "property_declaration" => {
            let receiver = receiver(collector, node);
            let constructed = constructed_type(&collector.source, node);
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
                    let type_name = declared_type(&collector.source, declaration)
                        .or_else(|| constructed.clone());
                    if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                        symbol.receiver.clone_from(&receiver);
                        symbol.type_name = type_name;
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

/// The declared type of a parameter or variable node, without type arguments.
fn declared_type(source: &Source<'_>, declaration: Node<'_>) -> Option<String> {
    named_children(declaration)
        .into_iter()
        .filter(|child| matches!(child.kind(), "user_type" | "nullable_type"))
        .find_map(find_user_type)
        .map(|user_type| user_type_name(source, user_type))
}

/// For `val x = Foo(...)`: the constructor name, taken as the type when none is written.
/// Only a capitalised bare callee counts, so `val x = compute()` stays unknown.
fn constructed_type(source: &Source<'_>, property: Node<'_>) -> Option<String> {
    let initialiser = named_children(property).into_iter().last()?;
    let callee =
        (initialiser.kind() == "call_expression").then(|| initialiser.named_child(0))??;
    let name = (callee.kind() == "simple_identifier").then(|| source.text(callee))?;
    name.starts_with(char::is_uppercase).then_some(name)
}

/// The names declared by a `variable_declaration` or `multi_variable_declaration`.
fn variable_binders<'t>(
    source: &Source<'_>,
    node: Node<'t>,
    fallback_type: Option<&String>,
) -> Vec<Binder<'t>> {
    let declarations = match node.kind() {
        "variable_declaration" => vec![node],
        "multi_variable_declaration" => named_children(node),
        _ => Vec::new(),
    };
    let single = declarations.len() == 1;
    declarations
        .into_iter()
        .filter_map(|declaration| {
            let name = child_of_kind(declaration, "simple_identifier")?;
            let type_name = declared_type(source, declaration)
                .or_else(|| fallback_type.filter(|_| single).cloned());
            Some(Binder { name, type_name })
        })
        .collect()
}

/// The names `child` introduces into `scope` for code that comes after it.
pub(crate) fn binders<'t>(
    source: &Source<'_>,
    scope: Node<'t>,
    child: Node<'t>,
    want_type: bool,
) -> Vec<Binder<'t>> {
    let parameter = |parameter: Node<'t>| {
        child_of_kind(parameter, "simple_identifier").map(|name| Binder {
            name,
            type_name: declared_type(source, parameter),
        })
    };
    if want_type {
        return if child.kind() == "type_parameters" {
            named_children(child)
                .into_iter()
                .filter_map(|parameter| child_of_kind(parameter, "type_identifier"))
                .map(|name| Binder {
                    name,
                    type_name: None,
                })
                .collect()
        } else {
            Vec::new()
        };
    }
    match (scope.kind(), child.kind()) {
        ("statements", "property_declaration") => {
            let constructed = constructed_type(source, child);
            named_children(child)
                .into_iter()
                .flat_map(|part| variable_binders(source, part, constructed.as_ref()))
                .collect()
        }
        // A local function can be called by the code after it.
        ("statements", "function_declaration") => child_of_kind(child, "simple_identifier")
            .map(|name| Binder {
                name,
                type_name: None,
            })
            .into_iter()
            .collect(),
        (_, "function_value_parameters") => named_children(child)
            .into_iter()
            .filter_map(parameter)
            .collect(),
        // Plain constructor parameters are visible in initialisers and `init` blocks.
        // `val`/`var` parameters are properties, which the index resolves as members.
        ("class_declaration", "primary_constructor") => named_children(child)
            .into_iter()
            .filter(|parameter| child_of_kind(*parameter, "binding_pattern_kind").is_none())
            .filter_map(parameter)
            .collect(),
        (_, "lambda_parameters") => named_children(child)
            .into_iter()
            .flat_map(|part| variable_binders(source, part, None))
            .collect(),
        ("for_statement", "variable_declaration" | "multi_variable_declaration") => {
            variable_binders(source, child, None)
        }
        ("when_expression", "when_subject") => named_children(child)
            .into_iter()
            .flat_map(|part| variable_binders(source, part, None))
            .collect(),
        ("catch_block", "simple_identifier") => {
            vec![Binder {
                name: child,
                type_name: declared_type(source, scope),
            }]
        }
        ("setter", "parameter_with_optional_type") => parameter(child).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The role, qualifier and import flag of an identifier node.
pub(crate) fn classify(
    source: &Source<'_>,
    node: Node<'_>,
) -> (ReferenceRole, Option<Qualifier>, bool) {
    let Some(parent) = node.parent() else {
        return (ReferenceRole::Value, None, false);
    };
    let is_first = parent
        .named_child(0)
        .is_some_and(|first| first.id() == node.id());
    // The dotted parts written before `node` inside `parent`, e.g. `java.io` in a type.
    let preceding = || {
        let before = source
            .buffer
            .text(
                dumbj_text::ByteOffset(parent.start_byte())
                    ..dumbj_text::ByteOffset(node.start_byte()),
            )
            .unwrap_or_default();
        let before: String = before.split_whitespace().collect();
        let before = before.trim_end_matches('.').to_owned();
        (!before.is_empty()).then(|| qualifier_from_text(&before))
    };
    match parent.kind() {
        "navigation_suffix" => {
            let Some(navigation) = parent.parent() else {
                return (ReferenceRole::Member, None, false);
            };
            let qualifier = navigation
                .named_child(0)
                .map(|receiver| match receiver.kind() {
                    "this_expression" => Qualifier::This,
                    "super_expression" => Qualifier::Super,
                    _ => qualifier_from_text(&source.text(receiver)),
                });
            let is_callee = navigation.parent().is_some_and(|call| {
                call.kind() == "call_expression"
                    && call
                        .named_child(0)
                        .is_some_and(|callee| callee.id() == navigation.id())
            });
            let role = if is_callee {
                ReferenceRole::Call
            } else {
                ReferenceRole::Member
            };
            (role, qualifier, false)
        }
        "call_expression" if is_first => (ReferenceRole::Call, None, false),
        "user_type" => (ReferenceRole::Type, preceding(), false),
        "identifier" => match parent.parent().map(|grandparent| grandparent.kind()) {
            Some("import_header") => (ReferenceRole::Type, preceding(), true),
            _ => (ReferenceRole::Declaration, None, false),
        },
        // `Foo::bar`
        "callable_reference" if node.kind() == "simple_identifier" => {
            let qualifier = child_of_kind(parent, "type_identifier")
                .map(|owner| qualifier_from_text(&source.text(owner)));
            (ReferenceRole::Call, qualifier, false)
        }
        "function_declaration"
        | "variable_declaration"
        | "parameter"
        | "class_parameter"
        | "enum_entry"
        | "catch_block"
        | "parameter_with_optional_type"
            if node.kind() == "simple_identifier" =>
        {
            (ReferenceRole::Declaration, None, false)
        }
        "class_declaration" | "object_declaration" | "companion_object" | "type_alias"
        | "type_parameter" | "import_alias"
            if is_first =>
        {
            (ReferenceRole::Declaration, None, false)
        }
        _ if node.kind() == "type_identifier" => (ReferenceRole::Type, None, false),
        _ => (ReferenceRole::Value, None, false),
    }
}
