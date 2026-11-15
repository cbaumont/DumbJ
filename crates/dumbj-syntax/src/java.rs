use tree_sitter::Node;

use crate::reference::{Argument, Binder, Call, Qualifier, ReferenceRole, qualifier_from_text};
use crate::structure::{
    Collector, Import, Parameter, Source, SymbolKind, all_children, is_type_parameter,
    named_children,
};

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

fn parameters(source: &Source<'_>, declaration: Node<'_>) -> Option<Vec<Parameter>> {
    let list = declaration.child_by_field_name("parameters")?;
    let found = named_children(list)
        .into_iter()
        .filter_map(|parameter| match parameter.kind() {
            "formal_parameter" => Some(Parameter {
                name: parameter
                    .child_by_field_name("name")
                    .map_or_else(String::new, |name| source.text(name)),
                type_name: declared_type(source, parameter, None),
                optional: false,
                variadic: false,
            }),
            // `String... rest`: the name sits in a variable_declarator.
            "spread_parameter" => Some(Parameter {
                name: named_children(parameter)
                    .into_iter()
                    .find(|part| part.kind() == "variable_declarator")
                    .and_then(|declarator| declarator.child_by_field_name("name"))
                    .map_or_else(String::new, |name| source.text(name)),
                type_name: named_children(parameter)
                    .into_iter()
                    .find_map(|part| type_name(source, part))
                    .filter(|name| !is_type_parameter(source, parameter, name)),
                optional: false,
                variadic: true,
            }),
            _ => None,
        })
        .collect();
    Some(found)
}

fn collect_type(
    collector: &mut Collector<'_>,
    node: Node<'_>,
    kind: SymbolKind,
    parent: Option<usize>,
) {
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
    if let Some(parameters) = node
        .child_by_field_name("parameters")
        .filter(|_| kind == SymbolKind::Record)
    {
        for parameter in named_children(parameters) {
            if let Some(name) = parameter.child_by_field_name("name") {
                let field = collector.push(SymbolKind::Field, parameter, name, Some(index));
                let type_name = declared_type(&collector.source, parameter, None);
                collector.set_type(field, type_name);
                let argument = declared_type_argument(&collector.source, parameter);
                collector.set_type_argument(field, argument);
            }
        }
    }
    if let Some(body) = node.child_by_field_name("body") {
        collect(collector, body, Some(index));
    }
}

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
        collect_type(collector, node, kind, parent);
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
                let method = collector.push(SymbolKind::Method, node, name, parent);
                let type_name = declared_type(&collector.source, node, None);
                collector.set_type(method, type_name);
                let argument = declared_type_argument(&collector.source, node);
                collector.set_type_argument(method, argument);
                // An annotation element has no parameter list but is called as `name()`.
                let parameters = parameters(&collector.source, node).or_else(|| Some(Vec::new()));
                collector.set_parameters(method, parameters);
            }
        }
        "constructor_declaration" | "compact_constructor_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                let constructor = collector.push(SymbolKind::Constructor, node, name, parent);
                let parameters = parameters(&collector.source, node);
                collector.set_parameters(constructor, parameters);
            }
        }
        "field_declaration" | "constant_declaration" => {
            let type_name = declared_type(&collector.source, node, None);
            let argument = declared_type_argument(&collector.source, node);
            for declarator in named_children(node) {
                let name = declarator.child_by_field_name("name");
                if let Some(name) = name.filter(|_| declarator.kind() == "variable_declarator") {
                    let field = collector.push(SymbolKind::Field, node, name, parent);
                    collector.set_type(field, type_name.clone());
                    collector.set_type_argument(field, argument.clone());
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

fn declared_type_argument(source: &Source<'_>, declaration: Node<'_>) -> Option<String> {
    let written = declaration.child_by_field_name("type")?;
    let generic = if written.kind() == "generic_type" {
        written
    } else {
        named_children(written)
            .into_iter()
            .find(|child| child.kind() == "generic_type")?
    };
    let arguments = named_children(generic)
        .into_iter()
        .find(|child| child.kind() == "type_arguments")?;
    let first = named_children(arguments).into_iter().next()?;
    type_name(source, first).filter(|name| !is_type_parameter(source, declaration, name))
}

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

fn declared_type(
    source: &Source<'_>,
    declaration: Node<'_>,
    declarator: Option<Node<'_>>,
) -> Option<String> {
    let written = declaration
        .child_by_field_name("type")
        .and_then(|node| type_name(source, node))
        .filter(|name| !is_type_parameter(source, declaration, name));
    written.or_else(|| {
        let value = declarator?.child_by_field_name("value")?;
        (value.kind() == "object_creation_expression")
            .then(|| value.child_by_field_name("type"))
            .flatten()
            .and_then(|node| type_name(source, node))
    })
}

pub(crate) fn expression(source: &Source<'_>, node: Node<'_>) -> Qualifier {
    let typed = |name: &str| Qualifier::Typed(name.to_owned());
    let written_type = |node: Node<'_>| {
        node.child_by_field_name("type")
            .and_then(|written| type_name(source, written))
            .filter(|name| !is_type_parameter(source, node, name))
            .map_or(Qualifier::Other, Qualifier::Typed)
    };
    match node.kind() {
        "identifier" => Qualifier::Name(source.text(node)),
        "this" => Qualifier::This,
        "super" => Qualifier::Super,
        "parenthesized_expression" => node
            .named_child(0)
            .map_or(Qualifier::Other, |inner| expression(source, inner)),
        "object_creation_expression" | "cast_expression" => written_type(node),
        "string_literal" => typed("String"),
        "true" | "false" => typed("boolean"),
        "character_literal" => typed("char"),
        "decimal_integer_literal"
        | "hex_integer_literal"
        | "octal_integer_literal"
        | "binary_integer_literal" => typed("int"),
        "decimal_floating_point_literal" | "hex_floating_point_literal" => typed("double"),
        "lambda_expression" | "method_reference" => Qualifier::Lambda,
        "field_access" => {
            match (
                node.child_by_field_name("object"),
                node.child_by_field_name("field"),
            ) {
                (Some(object), Some(field)) => {
                    Qualifier::member(expression(source, object), source.text(field))
                }
                _ => Qualifier::Other,
            }
        }
        "method_invocation" => node
            .child_by_field_name("name")
            .map_or(Qualifier::Other, |name| {
                Qualifier::Call(Box::new(Call {
                    receiver: node
                        .child_by_field_name("object")
                        .map(|object| expression(source, object)),
                    name: source.text(name),
                    arguments: arguments(source, node),
                }))
            }),
        _ => Qualifier::Other,
    }
}

fn arguments(source: &Source<'_>, call: Node<'_>) -> Vec<Argument> {
    call.child_by_field_name("arguments")
        .map_or_else(Vec::new, named_children)
        .into_iter()
        .map(|argument| Argument {
            name: None,
            value: expression(source, argument),
            trailing: false,
        })
        .collect()
}

pub(crate) fn call_arguments(source: &Source<'_>, node: Node<'_>) -> Option<Vec<Argument>> {
    let call = node.parent()?;
    (call.kind() == "method_invocation").then(|| arguments(source, call))
}

pub(crate) fn binders<'t>(
    source: &Source<'_>,
    scope: Node<'t>,
    child: Node<'t>,
    want_type: bool,
) -> Vec<Binder<'t>> {
    let named = |declaration: Node<'t>| {
        declaration.child_by_field_name("name").map(|name| Binder {
            type_argument: declared_type_argument(source, declaration),
            ..Binder::new(name, declared_type(source, declaration, None))
        })
    };
    if want_type {
        return if child.kind() == "type_parameters" {
            named_children(child)
                .into_iter()
                .filter_map(|parameter| named_children(parameter).into_iter().next())
                .filter(|name| name.kind() == "type_identifier")
                .map(|name| Binder::new(name, None))
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
                    type_argument: declared_type_argument(source, child),
                    initialiser: declarator
                        .child_by_field_name("value")
                        .map(|value| expression(source, value)),
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
                    .map(|name| Binder::new(name, None)),
                _ => None,
            })
            .collect(),
        "inferred_parameters" => named_children(child)
            .into_iter()
            .filter(|name| name.kind() == "identifier")
            .map(|name| Binder::new(name, None))
            .collect(),
        "identifier" if scope.kind() == "lambda_expression" => {
            vec![Binder::new(child, None)]
        }
        "identifier"
            if scope.kind() == "enhanced_for_statement"
                && scope
                    .child_by_field_name("name")
                    .is_some_and(|name| name.id() == child.id()) =>
        {
            vec![Binder::new(child, declared_type(source, scope, None))]
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
                .map(|name| Binder::new(name, type_name))
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
        out.push(Binder::new(name, type_name));
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
        parent.child_by_field_name(field).map(|object| {
            if matches!(parent.kind(), "method_invocation" | "field_access") {
                expression(source, object)
            } else {
                qualifier_from_text(&source.text(object))
            }
        })
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
