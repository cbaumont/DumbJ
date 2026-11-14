//! Java-specific tree walking.

use tree_sitter::Node;

use crate::reference::{Binder, Qualifier, ReferenceRole, qualifier_from_text};
use crate::structure::{Collector, Import, Source, SymbolKind, all_children, named_children};

/// Type names in a `superclass`, `super_interfaces` or `extends_interfaces` clause.
fn supertypes(collector: &Collector<'_>, clause: Node<'_>, out: &mut Vec<String>) {
    for child in named_children(clause) {
        match child.kind() {
            "type_list" => supertypes(collector, child, out),
            "type_identifier" | "scoped_type_identifier" => out.push(collector.source.text(child)),
            // `List<String>`: the first child is the type without its arguments.
            "generic_type" => {
                if let Some(base) = child.named_child(0) {
                    out.push(collector.source.text(base));
                }
            }
            _ => {}
        }
    }
}

fn import(collector: &Collector<'_>, node: Node<'_>) -> Option<Import> {
    let children = all_children(node);
    let path = children
        .iter()
        .find(|child| matches!(child.kind(), "scoped_identifier" | "identifier"))?;
    Some(Import {
        path: collector.source.text(*path),
        wildcard: children.iter().any(|child| child.kind() == "asterisk"),
        is_static: children.iter().any(|child| child.kind() == "static"),
        alias: None,
    })
}

/// Walks `node` and records the declarations below it.
pub(crate) fn collect(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>) {
    let type_kind = match node.kind() {
        "class_declaration" => Some(SymbolKind::Class),
        "interface_declaration" => Some(SymbolKind::Interface),
        "enum_declaration" => Some(SymbolKind::Enum),
        "record_declaration" => Some(SymbolKind::Record),
        "annotation_type_declaration" => Some(SymbolKind::Annotation),
        _ => None,
    };
    if let Some(kind) = type_kind {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let index = collector.push(kind, node, name, parent);
        let mut supers = Vec::new();
        for clause in named_children(node) {
            if matches!(
                clause.kind(),
                "superclass" | "super_interfaces" | "extends_interfaces"
            ) {
                supertypes(collector, clause, &mut supers);
            }
        }
        if let Some(symbol) = collector.facts.symbols.get_mut(index) {
            symbol.supertypes = supers;
        }
        // Record components are fields of the record.
        if let Some(parameters) = node
            .child_by_field_name("parameters")
            .filter(|_| kind == SymbolKind::Record)
        {
            for parameter in named_children(parameters) {
                if let Some(name) = parameter.child_by_field_name("name") {
                    collector.push(SymbolKind::Field, parameter, name, Some(index));
                }
            }
        }
        if let Some(body) = node.child_by_field_name("body") {
            collect(collector, body, Some(index));
        }
        return;
    }
    match node.kind() {
        "package_declaration" => {
            let name = named_children(node)
                .into_iter()
                .find(|child| matches!(child.kind(), "scoped_identifier" | "identifier"));
            collector.facts.package = name.map(|name| collector.source.text(name));
        }
        "import_declaration" => {
            if let Some(import) = import(collector, node) {
                collector.facts.imports.push(import);
            }
        }
        "method_declaration" | "annotation_type_element_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                collector.push(SymbolKind::Method, node, name, parent);
            }
        }
        "constructor_declaration" | "compact_constructor_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                collector.push(SymbolKind::Constructor, node, name, parent);
            }
        }
        "field_declaration" | "constant_declaration" => {
            for declarator in named_children(node) {
                if let Some(name) = declarator
                    .child_by_field_name("name")
                    .filter(|_| declarator.kind() == "variable_declarator")
                {
                    collector.push(SymbolKind::Field, node, name, parent);
                }
            }
        }
        "enum_constant" => {
            if let Some(name) = node.child_by_field_name("name") {
                collector.push(SymbolKind::EnumConstant, node, name, parent);
            }
        }
        // Containers of declarations, and ERROR nodes so a syntax error hides as little
        // as possible. Method bodies are not entered: local classes are not indexed.
        "program"
        | "class_body"
        | "interface_body"
        | "enum_body"
        | "enum_body_declarations"
        | "annotation_type_body"
        | "ERROR" => {
            for child in named_children(node) {
                collect(collector, child, parent);
            }
        }
        _ => {}
    }
}

/// The type named by a `type` field, without type arguments or array brackets. `var` and
/// primitive types give `None`.
fn type_name(source: &Source<'_>, node: Node<'_>) -> Option<String> {
    match node.kind() {
        "type_identifier" if source.text(node) == "var" => None,
        "type_identifier" | "scoped_type_identifier" => Some(source.text(node)),
        "generic_type" | "array_type" | "annotated_type" => named_children(node)
            .into_iter()
            .find_map(|child| type_name(source, child)),
        _ => None,
    }
}

/// The type of a declared variable: its written type, or for `var x = new Foo()` the
/// constructed type.
fn declared_type(
    source: &Source<'_>,
    declaration: Node<'_>,
    declarator: Option<Node<'_>>,
) -> Option<String> {
    let written = declaration
        .child_by_field_name("type")
        .and_then(|node| type_name(source, node));
    written.or_else(|| {
        let value = declarator?.child_by_field_name("value")?;
        (value.kind() == "object_creation_expression")
            .then(|| value.child_by_field_name("type"))
            .flatten()
            .and_then(|node| type_name(source, node))
    })
}

/// The names `child` introduces into `scope` for code that comes after it.
pub(crate) fn binders<'t>(
    source: &Source<'_>,
    scope: Node<'t>,
    child: Node<'t>,
    want_type: bool,
) -> Vec<Binder<'t>> {
    let named = |declaration: Node<'t>| {
        declaration.child_by_field_name("name").map(|name| Binder {
            name,
            type_name: declared_type(source, declaration, None),
        })
    };
    if want_type {
        // Type parameters of the enclosing method or class.
        return if child.kind() == "type_parameters" {
            named_children(child)
                .into_iter()
                .filter_map(|parameter| named_children(parameter).into_iter().next())
                .filter(|name| name.kind() == "type_identifier")
                .map(|name| Binder {
                    name,
                    type_name: None,
                })
                .collect()
        } else {
            Vec::new()
        };
    }
    match child.kind() {
        "local_variable_declaration" => named_children(child)
            .into_iter()
            .filter(|declarator| declarator.kind() == "variable_declarator")
            .filter_map(|declarator| {
                let name = declarator.child_by_field_name("name")?;
                Some(Binder {
                    name,
                    type_name: declared_type(source, child, Some(declarator)),
                })
            })
            .collect(),
        "formal_parameters" => named_children(child)
            .into_iter()
            .filter_map(|parameter| match parameter.kind() {
                "formal_parameter" => named(parameter),
                // `String... rest`: the name sits in a variable_declarator.
                "spread_parameter" => named_children(parameter)
                    .into_iter()
                    .find(|part| part.kind() == "variable_declarator")
                    .and_then(|declarator| declarator.child_by_field_name("name"))
                    .map(|name| Binder {
                        name,
                        type_name: None,
                    }),
                _ => None,
            })
            .collect(),
        // Lambda parameters without types: `(a, b) ->`.
        "inferred_parameters" => named_children(child)
            .into_iter()
            .filter(|name| name.kind() == "identifier")
            .map(|name| Binder {
                name,
                type_name: None,
            })
            .collect(),
        // A single untyped lambda parameter: `x ->`.
        "identifier" if scope.kind() == "lambda_expression" => {
            vec![Binder {
                name: child,
                type_name: None,
            }]
        }
        // The loop variable of `for (String s : names)`.
        "identifier"
            if scope.kind() == "enhanced_for_statement"
                && scope
                    .child_by_field_name("name")
                    .is_some_and(|name| name.id() == child.id()) =>
        {
            vec![Binder {
                name: child,
                type_name: declared_type(source, scope, None),
            }]
        }
        "resource_specification" => named_children(child)
            .into_iter()
            .filter_map(named)
            .collect(),
        "catch_formal_parameter" => {
            let type_name = named_children(child)
                .into_iter()
                .find(|part| part.kind() == "catch_type")
                .and_then(|types| named_children(types).into_iter().next())
                .and_then(|first| type_name(source, first));
            child
                .child_by_field_name("name")
                .map(|name| Binder { name, type_name })
                .into_iter()
                .collect()
        }
        // `if (x instanceof Foo f)`: the pattern variable is visible after the condition.
        "parenthesized_expression" | "binary_expression" | "instanceof_expression" => {
            let mut found = Vec::new();
            pattern_variables(source, child, &mut found);
            found
        }
        _ => Vec::new(),
    }
}

fn pattern_variables<'t>(source: &Source<'_>, node: Node<'t>, out: &mut Vec<Binder<'t>>) {
    if node.kind() == "instanceof_expression"
        && let Some(name) = node.child_by_field_name("name")
    {
        let type_name = node
            .child_by_field_name("right")
            .and_then(|right| type_name(source, right));
        out.push(Binder { name, type_name });
    }
    if matches!(
        node.kind(),
        "parenthesized_expression" | "binary_expression" | "instanceof_expression"
    ) {
        for child in named_children(node) {
            pattern_variables(source, child, out);
        }
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
    let is_field = |field: &str| {
        parent
            .child_by_field_name(field)
            .is_some_and(|n| n.id() == node.id())
    };
    let qualifier_of = |field: &str| {
        parent
            .child_by_field_name(field)
            .map(|object| qualifier_from_text(&source.text(object)))
    };
    let in_import = {
        let mut ancestor = Some(parent);
        std::iter::from_fn(|| {
            let current = ancestor?;
            ancestor = current.parent();
            Some(current)
        })
        .any(|ancestor| ancestor.kind() == "import_declaration")
    };
    match parent.kind() {
        "method_invocation" if is_field("name") => {
            (ReferenceRole::Call, qualifier_of("object"), false)
        }
        "field_access" if is_field("field") => {
            (ReferenceRole::Member, qualifier_of("object"), false)
        }
        // `Foo::bar`: the first identifier is the qualifier, the second the method.
        "method_reference" => {
            let first = parent
                .named_child(0)
                .filter(|first| first.id() != node.id());
            match first {
                Some(first) => (
                    ReferenceRole::Call,
                    Some(qualifier_from_text(&source.text(first))),
                    false,
                ),
                None => (ReferenceRole::Value, None, false),
            }
        }
        // `a.b.C` in imports, annotations and package statements.
        "scoped_identifier" if is_field("name") => {
            (ReferenceRole::Type, qualifier_of("scope"), in_import)
        }
        "scoped_identifier" => (ReferenceRole::Type, None, in_import),
        // `Outer.Inner`: every part but the first is qualified by what precedes it.
        "scoped_type_identifier" => {
            let before = source
                .buffer
                .text(
                    dumbj_text::ByteOffset(parent.start_byte())
                        ..dumbj_text::ByteOffset(node.start_byte()),
                )
                .unwrap_or_default();
            let before = before.trim_end().trim_end_matches('.');
            let qualifier = (!before.is_empty()).then(|| qualifier_from_text(before));
            (ReferenceRole::Type, qualifier, false)
        }
        "marker_annotation" | "annotation" => (ReferenceRole::Type, None, false),
        "import_declaration" => (ReferenceRole::Type, None, true),
        _ if node.kind() == "type_identifier" => (ReferenceRole::Type, None, false),
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration"
        | "annotation_type_declaration"
        | "method_declaration"
        | "constructor_declaration"
        | "variable_declarator"
        | "formal_parameter"
        | "enum_constant"
        | "catch_formal_parameter"
        | "resource"
        | "annotation_type_element_declaration"
            if is_field("name") =>
        {
            (ReferenceRole::Declaration, None, false)
        }
        _ => (ReferenceRole::Value, None, in_import),
    }
}
