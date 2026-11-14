//! Java-specific tree walking.

use tree_sitter::Node;

use crate::structure::{Collector, Import, SymbolKind, all_children, named_children};

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
