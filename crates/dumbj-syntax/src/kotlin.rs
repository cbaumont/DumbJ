//! The grammar has almost no field names, so children are found by kind and position.

use tree_sitter::Node;

use crate::reference::{
    Argument, Binder, Call, LambdaCall, Qualifier, ReferenceRole, qualifier_from_text,
};
use crate::structure::{
    Collector, Import, Parameter, Platform, Source, SymbolKind, all_children, child_of_kind,
    is_type_parameter, named_children,
};

fn user_type_name(source: &Source<'_>, user_type: Node<'_>) -> String {
    let parts: Vec<String> = named_children(user_type)
        .into_iter()
        .filter(|child| child.kind() == "type_identifier")
        .map(|part| source.text(part))
        .collect();
    parts.join(".")
}

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

fn is_vararg(source: &Source<'_>, modifiers: Node<'_>) -> bool {
    named_children(modifiers)
        .into_iter()
        .any(|modifier| source.text(modifier) == "vararg")
}

/// Modifiers and default values are siblings of their parameter.
fn function_parameters(source: &Source<'_>, list: Node<'_>) -> Vec<Parameter> {
    let mut found: Vec<Parameter> = Vec::new();
    let mut variadic = false;
    for child in all_children(list) {
        match child.kind() {
            "parameter_modifiers" => variadic = is_vararg(source, child),
            "parameter" => {
                let name = child_of_kind(child, "simple_identifier");
                found.push(Parameter {
                    name: name.map_or_else(String::new, |name| source.text(name)),
                    type_name: declared_type(source, child),
                    optional: false,
                    variadic: std::mem::take(&mut variadic),
                });
            }
            "=" => {
                if let Some(last) = found.last_mut() {
                    last.optional = true;
                }
            }
            _ => {}
        }
    }
    found
}

fn class_parameters(source: &Source<'_>, constructor: Node<'_>) -> Vec<Parameter> {
    named_children(constructor)
        .into_iter()
        .filter(|child| child.kind() == "class_parameter")
        .map(|parameter| Parameter {
            name: child_of_kind(parameter, "simple_identifier")
                .map_or_else(String::new, |name| source.text(name)),
            type_name: declared_type(source, parameter),
            optional: all_children(parameter).iter().any(|c| c.kind() == "="),
            variadic: child_of_kind(parameter, "modifiers")
                .is_some_and(|modifiers| is_vararg(source, modifiers)),
        })
        .collect()
}

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
    // What `Name(…)` calls: the primary constructor, or the implicit one of a class that
    // declares none. A class with only secondary constructors is called through those.
    if matches!(kind, SymbolKind::Class | SymbolKind::Annotation) {
        let primary = child_of_kind(node, "primary_constructor")
            .map(|constructor| class_parameters(&collector.source, constructor));
        let has_secondary = child_of_kind(node, "class_body").is_some_and(|body| {
            named_children(body)
                .iter()
                .any(|member| member.kind() == "secondary_constructor")
        });
        let parameters = primary.or_else(|| (!has_secondary).then(Vec::new));
        collector.set_parameters(index, parameters);
    }
    // Before the members are walked: they inherit `expect` from the type.
    mark_platform(collector, node, parent, index);
    for child in named_children(node) {
        match child.kind() {
            "primary_constructor" => {
                for parameter in named_children(child) {
                    let is_property = child_of_kind(parameter, "binding_pattern_kind").is_some();
                    let name = child_of_kind(parameter, "simple_identifier");
                    if let Some(name) = name.filter(|_| is_property) {
                        let property =
                            collector.push(SymbolKind::Property, parameter, name, Some(index));
                        let type_name = declared_type(&collector.source, parameter);
                        collector.set_type(property, type_name);
                        let argument = declared_type_argument(&collector.source, parameter);
                        collector.set_type_argument(property, argument);
                    }
                }
            }
            "class_body" | "enum_class_body" => collect(collector, child, Some(index)),
            _ => {}
        }
    }
}

fn jvm_name(source: &Source<'_>, annotations: &[Node<'_>], target: Option<&str>) -> Option<String> {
    annotations.iter().find_map(|annotation| {
        let site = child_of_kind(*annotation, "use_site_target")
            .map(|site| source.text(site).trim_end_matches(':').trim().to_owned());
        if site.as_deref() != target && annotation.kind() != "file_annotation" {
            return None;
        }
        let invocation = child_of_kind(*annotation, "constructor_invocation")?;
        let user_type = child_of_kind(invocation, "user_type")?;
        let named = user_type_name(source, user_type);
        if named.rsplit('.').next() != Some("JvmName") {
            return None;
        }
        let arguments = child_of_kind(invocation, "value_arguments")?;
        let literal = named_children(arguments)
            .into_iter()
            .find_map(|argument| child_of_kind(argument, "string_literal"))?;
        child_of_kind(literal, "string_content").map(|content| source.text(content))
    })
}

fn annotations_of(node: Node<'_>) -> Vec<Node<'_>> {
    child_of_kind(node, "modifiers")
        .map_or_else(Vec::new, named_children)
        .into_iter()
        .filter(|child| child.kind() == "annotation")
        .collect()
}

/// `actual`. Members of an `expect` type are `expect` without saying so.
fn mark_platform(
    collector: &mut Collector<'_>,
    declaration: Node<'_>,
    parent: Option<usize>,
    first: usize,
) {
    let inherited = parent
        .and_then(|parent| collector.facts.symbols.get(parent))
        .and_then(|parent| parent.platform)
        .filter(|platform| *platform == Platform::Expect);
    let platform = if has_modifier(collector, declaration, "expect") {
        Some(Platform::Expect)
    } else if has_modifier(collector, declaration, "actual") {
        Some(Platform::Actual)
    } else {
        inherited
    };
    // Only the declaration's own symbols: its members are marked when they are walked,
    // which happens after this for types because `collect_type` marks before descending.
    for symbol in collector.facts.symbols.iter_mut().skip(first) {
        if symbol.parent == parent {
            symbol.platform = platform;
        }
    }
}

pub(crate) fn collect(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>) {
    let first = collector.facts.symbols.len();
    collect_declaration(collector, node, parent);
    if matches!(
        node.kind(),
        "function_declaration" | "property_declaration" | "secondary_constructor"
    ) {
        mark_platform(collector, node, parent, first);
    }
}

fn collect_function(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>) {
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
        let type_argument = declared_type_argument(&collector.source, node);
        let parameters = child_of_kind(node, "function_value_parameters")
            .map(|list| function_parameters(&collector.source, list));
        let renamed = jvm_name(&collector.source, &annotations_of(node), None);
        if let Some(symbol) = collector.facts.symbols.get_mut(index) {
            symbol.receiver = receiver;
            symbol.type_name = type_name;
            symbol.type_argument = type_argument;
            symbol.parameters = parameters;
            symbol.jvm_name = renamed;
        }
    }
}

fn collect_property(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>) {
    let receiver = receiver(collector, node);
    let constructed = constructed_type(&collector.source, node);
    let getter = jvm_name(&collector.source, &annotations_of(node), Some("get"));
    let declarations = named_children(node)
        .into_iter()
        .flat_map(|child| match child.kind() {
            "variable_declaration" => vec![child],
            "multi_variable_declaration" => named_children(child),
            _ => Vec::new(),
        });
    for declaration in declarations.collect::<Vec<_>>() {
        if let Some(name) = child_of_kind(declaration, "simple_identifier") {
            let index = collector.push(SymbolKind::Property, node, name, parent);
            let type_name =
                declared_type(&collector.source, declaration).or_else(|| constructed.clone());
            let type_argument = declared_type_argument(&collector.source, declaration);
            if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                symbol.receiver.clone_from(&receiver);
                symbol.type_name = type_name;
                symbol.type_argument = type_argument;
                symbol.jvm_name.clone_from(&getter);
            }
        }
    }
}

fn collect_declaration(collector: &mut Collector<'_>, node: Node<'_>, parent: Option<usize>) {
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
        "file_annotation" => {
            if let Some(name) = jvm_name(&collector.source, &[node], None) {
                collector.facts.jvm_name = Some(name);
            }
        }
        "type_alias" => {
            if let Some(name) = child_of_kind(node, "type_identifier") {
                collector.push(SymbolKind::TypeAlias, node, name, parent);
            }
        }
        "function_declaration" => collect_function(collector, node, parent),
        "property_declaration" => collect_property(collector, node, parent),
        "enum_entry" => {
            if let Some(name) = child_of_kind(node, "simple_identifier") {
                collector.push(SymbolKind::EnumConstant, node, name, parent);
            }
        }
        "secondary_constructor" => {
            let class_name = parent
                .and_then(|index| collector.facts.symbols.get(index))
                .map(|s| s.name.clone());
            if let Some(class_name) = class_name {
                let index = collector.push(SymbolKind::Constructor, node, node, parent);
                let parameters = child_of_kind(node, "function_value_parameters")
                    .map(|list| function_parameters(&collector.source, list));
                if let Some(symbol) = collector.facts.symbols.get_mut(index) {
                    symbol.name = class_name;
                    symbol.name_range.end = symbol.name_range.start;
                    symbol.parameters = parameters;
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

fn declared_type(source: &Source<'_>, declaration: Node<'_>) -> Option<String> {
    named_children(declaration)
        .into_iter()
        .filter(|child| matches!(child.kind(), "user_type" | "nullable_type"))
        .find_map(find_user_type)
        .map(|user_type| user_type_name(source, user_type))
        .filter(|name| !is_type_parameter(source, declaration, name))
}

fn declared_type_argument(source: &Source<'_>, declaration: Node<'_>) -> Option<String> {
    let user_type = named_children(declaration)
        .into_iter()
        .filter(|child| matches!(child.kind(), "user_type" | "nullable_type"))
        .find_map(find_user_type)?;
    let arguments = child_of_kind(user_type, "type_arguments")?;
    let projection = named_children(arguments).into_iter().next()?;
    named_children(projection)
        .into_iter()
        .filter(|child| matches!(child.kind(), "user_type" | "nullable_type"))
        .find_map(find_user_type)
        .map(|argument| user_type_name(source, argument))
        .filter(|name| !is_type_parameter(source, declaration, name))
}

pub(crate) fn expression(source: &Source<'_>, node: Node<'_>) -> Qualifier {
    let typed = |name: &str| Qualifier::Typed(name.to_owned());
    match node.kind() {
        "simple_identifier" => Qualifier::Name(source.text(node)),
        // `this@Outer` names some enclosing receiver; only a bare `this` is followed.
        "this_expression" if source.text(node) == "this" => Qualifier::This,
        "super_expression" => Qualifier::Super,
        "parenthesized_expression" => node
            .named_child(0)
            .map_or(Qualifier::Other, |inner| expression(source, inner)),
        // `x!!` has the type of `x`; `x++` is not worth following.
        "postfix_expression" if source.text(node).ends_with("!!") => node
            .named_child(0)
            .map_or(Qualifier::Other, |inner| expression(source, inner)),
        "as_expression" => named_children(node)
            .into_iter()
            .skip(1)
            .find_map(find_user_type)
            .map(|user_type| user_type_name(source, user_type))
            .filter(|name| !is_type_parameter(source, node, name))
            .map_or(Qualifier::Other, Qualifier::Typed),
        "string_literal" => typed("String"),
        "boolean_literal" => typed("Boolean"),
        "integer_literal" | "hex_literal" | "bin_literal" => typed("Int"),
        "long_literal" => typed("Long"),
        "real_literal" => typed("Double"),
        "character_literal" => typed("Char"),
        "lambda_literal" | "anonymous_function" | "callable_reference" => Qualifier::Lambda,
        "navigation_expression" => {
            let name = child_of_kind(node, "navigation_suffix")
                .and_then(|suffix| child_of_kind(suffix, "simple_identifier"));
            match (node.named_child(0), name) {
                (Some(receiver), Some(name)) => {
                    Qualifier::member(expression(source, receiver), source.text(name))
                }
                _ => Qualifier::Other,
            }
        }
        "call_expression" => {
            let Some(callee) = node.named_child(0) else {
                return Qualifier::Other;
            };
            let (receiver, name) = match callee.kind() {
                "simple_identifier" => (None, Some(callee)),
                "navigation_expression" => (
                    callee.named_child(0),
                    child_of_kind(callee, "navigation_suffix")
                        .and_then(|suffix| child_of_kind(suffix, "simple_identifier")),
                ),
                _ => (None, None),
            };
            name.map_or(Qualifier::Other, |name| {
                Qualifier::Call(Box::new(Call {
                    receiver: receiver.map(|receiver| expression(source, receiver)),
                    name: source.text(name),
                    arguments: arguments(source, node),
                }))
            })
        }
        _ => Qualifier::Other,
    }
}

fn arguments(source: &Source<'_>, call: Node<'_>) -> Vec<Argument> {
    let Some(suffix) = child_of_kind(call, "call_suffix") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for part in named_children(suffix) {
        match part.kind() {
            "value_arguments" => {
                for argument in named_children(part) {
                    let named = all_children(argument).iter().any(|c| c.kind() == "=");
                    let name = argument
                        .named_child(0)
                        .filter(|first| named && first.kind() == "simple_identifier")
                        .map(|name| source.text(name));
                    let value = named_children(argument)
                        .into_iter()
                        .last()
                        .map_or(Qualifier::Other, |value| expression(source, value));
                    found.push(Argument {
                        name,
                        value,
                        trailing: false,
                    });
                }
            }
            "annotated_lambda" => found.push(Argument {
                name: None,
                value: Qualifier::Lambda,
                trailing: true,
            }),
            _ => {}
        }
    }
    found
}

/// The lambdas around `node`, innermost first, with the call each is passed to.
pub(crate) fn enclosing_lambdas(source: &Source<'_>, node: Node<'_>) -> Vec<LambdaCall> {
    std::iter::successors(Some(node), Node::parent)
        .filter(|lambda| lambda.kind() == "lambda_literal")
        .filter_map(|lambda| {
            let call = std::iter::successors(lambda.parent(), Node::parent)
                .take(3)
                .find(|ancestor| ancestor.kind() == "call_expression")?;
            let Qualifier::Call(call_info) = expression(source, call) else {
                return None;
            };
            Some(LambdaCall {
                callee: call_info.name,
                receiver: call_info.receiver,
                first_argument: call_info
                    .arguments
                    .into_iter()
                    .find(|argument| !argument.trailing)
                    .map(|argument| argument.value),
                has_parameters: child_of_kind(lambda, "lambda_parameters").is_some(),
                first_parameter: child_of_kind(lambda, "lambda_parameters")
                    .and_then(|parameters| named_children(parameters).into_iter().next())
                    .and_then(|first| child_of_kind(first, "simple_identifier"))
                    .map(crate::structure::range_of),
            })
        })
        .collect()
}

pub(crate) fn extension_receiver(source: &Source<'_>, node: Node<'_>) -> Option<String> {
    std::iter::successors(Some(node), Node::parent)
        .filter(|scope| {
            matches!(
                scope.kind(),
                "function_declaration" | "property_declaration"
            )
        })
        .find_map(|declaration| {
            let receiver = child_of_kind(declaration, "receiver_type")?;
            find_user_type(receiver)
                .map(|user_type| user_type_name(source, user_type))
                .filter(|name| !is_type_parameter(source, declaration, name))
        })
}

pub(crate) fn call_arguments(source: &Source<'_>, node: Node<'_>) -> Option<Vec<Argument>> {
    let parent = node.parent()?;
    let call = match parent.kind() {
        "call_expression" => parent,
        "navigation_suffix" => parent.parent()?.parent()?,
        _ => return None,
    };
    (call.kind() == "call_expression").then(|| arguments(source, call))
}

fn initialiser(source: &Source<'_>, property: Node<'_>) -> Option<Qualifier> {
    let has_value = all_children(property).iter().any(|c| c.kind() == "=");
    let value = named_children(property).into_iter().last()?;
    has_value.then(|| expression(source, value))
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

fn variable_binders<'t>(
    source: &Source<'_>,
    node: Node<'t>,
    initialiser: Option<&Qualifier>,
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
            Some(Binder {
                name,
                type_name: declared_type(source, declaration),
                type_argument: declared_type_argument(source, declaration),
                initialiser: initialiser.filter(|_| single).cloned(),
            })
        })
        .collect()
}

pub(crate) fn binders<'t>(
    source: &Source<'_>,
    scope: Node<'t>,
    child: Node<'t>,
    want_type: bool,
) -> Vec<Binder<'t>> {
    let parameter = |parameter: Node<'t>| {
        child_of_kind(parameter, "simple_identifier").map(|name| Binder {
            type_argument: declared_type_argument(source, parameter),
            ..Binder::new(name, declared_type(source, parameter))
        })
    };
    if want_type {
        return if child.kind() == "type_parameters" {
            named_children(child)
                .into_iter()
                .filter_map(|parameter| child_of_kind(parameter, "type_identifier"))
                .map(|name| Binder::new(name, None))
                .collect()
        } else {
            Vec::new()
        };
    }
    match (scope.kind(), child.kind()) {
        ("statements", "property_declaration") => {
            let initialiser = initialiser(source, child);
            named_children(child)
                .into_iter()
                .flat_map(|part| variable_binders(source, part, initialiser.as_ref()))
                .collect()
        }
        ("statements", "function_declaration") => child_of_kind(child, "simple_identifier")
            .map(|name| Binder::new(name, None))
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
            vec![Binder::new(child, declared_type(source, scope))]
        }
        ("setter", "parameter_with_optional_type") => parameter(child).into_iter().collect(),
        _ => Vec::new(),
    }
}

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
                .map(|receiver| expression(source, receiver));
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
