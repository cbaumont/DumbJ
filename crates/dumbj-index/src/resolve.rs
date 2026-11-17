//! Jumps only when one candidate is in the best tier and nothing was guessed; otherwise a chooser.

use std::cell::OnceCell;
use std::collections::HashSet;
use std::path::PathBuf;

use dumbj_syntax::{
    Argument, FileFacts, Language, LocalDefinition, Parameter, Platform, Qualifier, Reference,
    ReferenceRole, Symbol, SymbolKind, SyntaxTree,
};
use dumbj_text::{Buffer, ByteOffset, ByteRange};

use crate::index::{FileId, Index, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    Local,
    Scope,
    SameFile,
    ExplicitImport,
    SamePackage,
    WildcardImport,
    SameModule,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub range: ByteRange,
    pub symbol: Option<SymbolId>,
    pub tier: Tier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Jump,
    Choose,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub reference: Reference,
    pub targets: Vec<Target>,
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy)]
pub struct Site<'a> {
    pub index: &'a Index,
    pub file: FileId,
    pub tree: &'a SyntaxTree,
    pub buffer: &'a Buffer,
}

const MAX_SUPERTYPE_DEPTH: usize = 12;

const MAX_EXPRESSION_DEPTH: usize = 8;

fn kind_fits(kind: SymbolKind, role: ReferenceRole, language: Language) -> bool {
    match role {
        ReferenceRole::Type => kind.is_type(),
        ReferenceRole::Call => {
            matches!(kind, SymbolKind::Function | SymbolKind::Method)
                || (language == Language::Kotlin && kind.is_type())
        }
        ReferenceRole::Member | ReferenceRole::Value => {
            matches!(
                kind,
                SymbolKind::Field | SymbolKind::Property | SymbolKind::EnumConstant
            ) || kind.is_type()
        }
        ReferenceRole::Declaration => false,
    }
}

struct Resolver<'a> {
    index: &'a Index,
    language: Language,
}

impl<'a> Resolver<'a> {
    fn facts(&self, file: FileId) -> Option<&'a FileFacts> {
        self.index.file(file).map(|file| &file.facts)
    }

    fn symbol(&self, id: SymbolId) -> Option<&'a Symbol> {
        self.index.symbol(id)
    }

    fn is_type(&self, id: SymbolId) -> bool {
        self.symbol(id).is_some_and(|symbol| symbol.kind.is_type())
    }

    fn top_level(&self, id: SymbolId) -> SymbolId {
        let mut current = id;
        while let Some(parent) = self.index.parent(current) {
            current = parent;
        }
        current
    }

    fn enclosing_types(&self, file: FileId, offset: ByteOffset) -> Vec<SymbolId> {
        let Some(facts) = self.facts(file) else {
            return Vec::new();
        };
        let mut found: Vec<(usize, SymbolId)> = facts
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, symbol)| {
                symbol.kind.is_type() && symbol.range.start <= offset && offset < symbol.range.end
            })
            .map(|(index, symbol)| {
                let id = SymbolId {
                    file,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                };
                (symbol.range.end.get() - symbol.range.start.get(), id)
            })
            .collect();
        found.sort_unstable();
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// With `as_member`, ranks like the top-level type containing the candidate.
    fn tier(
        &self,
        file: FileId,
        enclosing: &[SymbolId],
        candidate: SymbolId,
        as_member: bool,
    ) -> Tier {
        let parent = self.index.parent(candidate);
        if parent.is_some_and(|parent| enclosing.contains(&parent)) {
            return Tier::Scope;
        }
        let subject = if as_member {
            self.top_level(candidate)
        } else {
            candidate
        };
        let subject_fqn = self.index.fqn(subject);
        let candidate_fqn = self.index.fqn(candidate);
        let imports = self
            .facts(file)
            .map_or(&[][..], |facts| facts.imports.as_slice());
        let explicit = imports.iter().any(|import| {
            !import.wildcard && (import.path == subject_fqn || import.path == candidate_fqn)
        });
        if candidate.file == file {
            return Tier::SameFile;
        }
        if explicit {
            return Tier::ExplicitImport;
        }
        // Nested declarations are only visible unqualified through an import.
        let container = match self.index.parent(subject) {
            Some(parent) => self.index.fqn(parent),
            None => self.index.package_of(subject).to_owned(),
        };
        let nested = self.index.parent(subject).is_some();
        let own_package = self
            .facts(file)
            .and_then(|facts| facts.package.as_deref())
            .unwrap_or("");
        if !nested && container == own_package {
            return Tier::SamePackage;
        }
        if imports
            .iter()
            .any(|import| import.wildcard && import.path == container)
        {
            return Tier::WildcardImport;
        }
        let module = self.index.module_of(file);
        if module.is_some() && module == self.index.module_of(candidate.file) {
            return Tier::SameModule;
        }
        Tier::Repo
    }

    fn best(
        &self,
        file: FileId,
        enclosing: &[SymbolId],
        candidates: Vec<SymbolId>,
    ) -> Vec<SymbolId> {
        let tiers: Vec<Tier> = candidates
            .iter()
            .map(|id| self.tier(file, enclosing, *id, false))
            .collect();
        let Some(best) = tiers.iter().min().copied() else {
            return Vec::new();
        };
        candidates
            .into_iter()
            .zip(tiers)
            .filter(|(_, tier)| *tier == best)
            .map(|(id, _)| id)
            .collect()
    }

    fn resolve_type(&self, file: FileId, enclosing: &[SymbolId], name: &str) -> Vec<SymbolId> {
        let types = |candidates: &[SymbolId]| -> Vec<SymbolId> {
            candidates
                .iter()
                .copied()
                .filter(|id| self.is_type(*id))
                .collect()
        };
        let Some((first, rest)) = name.split_once('.') else {
            let aliased = self.facts(file).and_then(|facts| {
                facts
                    .imports
                    .iter()
                    .find(|import| import.alias.as_deref() == Some(name))
            });
            if let Some(import) = aliased {
                return types(&self.index.symbols_by_fqn(&import.path));
            }
            return self.best(file, enclosing, types(self.index.symbols_named(name)));
        };
        let exact = types(&self.index.symbols_by_fqn(name));
        if !exact.is_empty() {
            return exact;
        }
        let mut owners = self.resolve_type(file, enclosing, first);
        for part in rest.split('.') {
            owners = owners
                .iter()
                .flat_map(|owner| self.index.members(*owner))
                .filter(|(_, symbol)| symbol.name == part && symbol.kind.is_type())
                .map(|(id, _)| id)
                .collect();
        }
        owners
    }

    fn supertypes(&self, type_id: SymbolId) -> Vec<SymbolId> {
        let Some(symbol) = self.symbol(type_id) else {
            return Vec::new();
        };
        let enclosing: Vec<SymbolId> =
            std::iter::successors(self.index.parent(type_id), |id| self.index.parent(*id))
                .collect();
        symbol
            .supertypes
            .iter()
            .flat_map(|name| self.resolve_type(type_id.file, &enclosing, name))
            .collect()
    }

    fn with_supertypes(&self, type_id: SymbolId) -> Vec<SymbolId> {
        let mut seen = HashSet::from([type_id]);
        let mut order = vec![type_id];
        let mut frontier = vec![type_id];
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            let next: Vec<SymbolId> = frontier
                .iter()
                .flat_map(|id| self.supertypes(*id))
                .filter(|id| seen.insert(*id))
                .collect();
            if next.is_empty() {
                break;
            }
            order.extend(&next);
            frontier = next;
        }
        order
    }

    /// Overloads can span a hierarchy: the nearest type with an applicable member wins,
    /// else the nearest type with the name.
    fn members_named(
        &self,
        owner: SymbolId,
        name: &str,
        role: ReferenceRole,
        applicable: &dyn Fn(SymbolId) -> bool,
    ) -> Vec<SymbolId> {
        let found = self.members_named_as(owner, name, role, applicable);
        if !found.is_empty() {
            return found;
        }
        accessor_alternatives(name, role, self.language)
            .into_iter()
            .map(|(alternative, role)| {
                self.members_named_as(owner, &alternative, role, &any_member)
            })
            .find(|found| !found.is_empty())
            .unwrap_or_default()
    }

    fn members_named_as(
        &self,
        owner: SymbolId,
        name: &str,
        role: ReferenceRole,
        applicable: &dyn Fn(SymbolId) -> bool,
    ) -> Vec<SymbolId> {
        let fits = |symbol: &Symbol| fits_reference(symbol, name, role, self.language);
        let mut nearest = Vec::new();
        for type_id in self.with_supertypes(owner) {
            // Record components are also accessor methods.
            let is_record = self
                .symbol(type_id)
                .is_some_and(|s| s.kind == SymbolKind::Record);
            let is_accessor = |symbol: &Symbol| {
                is_record
                    && role == ReferenceRole::Call
                    && symbol.kind == SymbolKind::Field
                    && symbol.name == name
            };
            let mut found: Vec<SymbolId> = self
                .index
                .members(type_id)
                .filter(|(_, symbol)| fits(symbol) || is_accessor(symbol))
                .map(|(id, _)| id)
                .collect();
            let companions = self
                .index
                .members(type_id)
                .filter(|(_, symbol)| symbol.kind == SymbolKind::Companion)
                .map(|(id, _)| id);
            for companion in companions {
                found.extend(
                    self.index
                        .members(companion)
                        .filter(|(_, s)| fits(s))
                        .map(|(id, _)| id),
                );
            }
            let fitting: Vec<SymbolId> =
                found.iter().copied().filter(|id| applicable(*id)).collect();
            if !fitting.is_empty() {
                return fitting;
            }
            if nearest.is_empty() {
                nearest = found;
            }
        }
        nearest
    }

    /// `None` when the type is not in the index.
    fn types_named(
        &self,
        declared_in: SymbolId,
        type_name: Option<&String>,
    ) -> Option<Vec<SymbolId>> {
        let enclosing: Vec<SymbolId> =
            std::iter::successors(self.index.parent(declared_in), |id| self.index.parent(*id))
                .collect();
        let types = self.resolve_type(declared_in.file, &enclosing, type_name?);
        (!types.is_empty()).then_some(types)
    }
}

fn any_member(_: SymbolId) -> bool {
    true
}

/// Java also calls a Kotlin function by its `@JvmName` and a property's getter by its
/// `@get:JvmName`.
fn fits_reference(symbol: &Symbol, name: &str, role: ReferenceRole, language: Language) -> bool {
    let renamed = language == Language::Java
        && role == ReferenceRole::Call
        && symbol.jvm_name.as_deref() == Some(name)
        && (symbol.kind.is_callable() || symbol.kind == SymbolKind::Property);
    renamed || (symbol.name == name && kind_fits(symbol.kind, role, language))
}

const ELEMENT_LAMBDAS: &[&str] = &[
    "forEach",
    "onEach",
    "map",
    "mapNotNull",
    "flatMap",
    "filter",
    "filterNot",
    "first",
    "firstOrNull",
    "last",
    "lastOrNull",
    "find",
    "findLast",
    "single",
    "singleOrNull",
    "any",
    "all",
    "none",
    "count",
    "sumOf",
    "maxBy",
    "maxByOrNull",
    "minBy",
    "minByOrNull",
    "maxOf",
    "minOf",
    "sortedBy",
    "sortedByDescending",
    "groupBy",
    "associateBy",
    "associateWith",
    "associate",
    "partition",
    "distinctBy",
    "takeWhile",
    "dropWhile",
    "indexOfFirst",
    "indexOfLast",
    "collect",
    "collectLatest",
    "mapLatest",
    "flatMapLatest",
    "flatMapConcat",
    "flatMapMerge",
    "transformLatest",
];

const ELEMENT_PRESERVING: &[&str] = &[
    "filter",
    "filterNot",
    "filterNotNull",
    "sorted",
    "sortedBy",
    "sortedByDescending",
    "sortedWith",
    "distinct",
    "distinctBy",
    "reversed",
    "shuffled",
    "take",
    "takeLast",
    "takeWhile",
    "drop",
    "dropLast",
    "dropWhile",
    "toList",
    "toMutableList",
    "toSet",
    "toMutableSet",
    "asSequence",
    "asIterable",
    "asFlow",
    "onEach",
    "plus",
    "minus",
    "flowOn",
    "distinctUntilChanged",
    "distinctUntilChangedBy",
    "catch",
    "onStart",
    "onCompletion",
    "buffer",
    "conflate",
    "debounce",
    "sample",
    "retry",
    "stateIn",
    "shareIn",
    "filterIsInstance",
];

const ELEMENT_RETURNING: &[&str] = &[
    "first",
    "firstOrNull",
    "last",
    "lastOrNull",
    "single",
    "singleOrNull",
    "find",
    "findLast",
    "get",
    "getOrNull",
    "elementAt",
    "elementAtOrNull",
    "random",
    "randomOrNull",
    "maxBy",
    "maxByOrNull",
    "minBy",
    "minByOrNull",
    "removeFirst",
    "removeLast",
];

const ELEMENT_BUILDERS: &[&str] = &[
    "listOf",
    "listOfNotNull",
    "mutableListOf",
    "arrayListOf",
    "setOf",
    "mutableSetOf",
    "hashSetOf",
    "linkedSetOf",
    "arrayOf",
    "sequenceOf",
    "flowOf",
];

fn capitalise(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn decapitalise(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

fn strip_accessor<'n>(name: &'n str, prefix: &str) -> Option<&'n str> {
    name.strip_prefix(prefix)
        .filter(|rest| rest.starts_with(char::is_uppercase))
}

/// The names a Kotlin property and its Java accessors go by, other than `name`: Java
/// `getName()`/`setName()` for `name`, `isOpen()` for `isOpen`, and back.
fn accessor_alternatives(
    name: &str,
    role: ReferenceRole,
    language: Language,
) -> Vec<(String, ReferenceRole)> {
    match (language, role) {
        (Language::Java, ReferenceRole::Call) => {
            if strip_accessor(name, "is").is_some() {
                vec![(name.to_owned(), ReferenceRole::Member)]
            } else if let Some(rest) =
                strip_accessor(name, "get").or_else(|| strip_accessor(name, "set"))
            {
                let mut found = vec![(decapitalise(rest), ReferenceRole::Member)];
                if name.starts_with("set") {
                    found.push((format!("is{rest}"), ReferenceRole::Member));
                }
                found
            } else {
                Vec::new()
            }
        }
        (Language::Kotlin, ReferenceRole::Member | ReferenceRole::Value) => {
            if strip_accessor(name, "is").is_some() {
                vec![(name.to_owned(), ReferenceRole::Call)]
            } else {
                vec![(format!("get{}", capitalise(name)), ReferenceRole::Call)]
            }
        }
        _ => Vec::new(),
    }
}

pub(crate) fn accessor_aliases(name: &str, kind: SymbolKind) -> Vec<String> {
    match kind {
        SymbolKind::Property | SymbolKind::Field => {
            if let Some(rest) = strip_accessor(name, "is") {
                vec![format!("set{rest}")]
            } else {
                let capitalised = capitalise(name);
                vec![format!("get{capitalised}"), format!("set{capitalised}")]
            }
        }
        SymbolKind::Method => {
            if strip_accessor(name, "is").is_some() {
                Vec::new()
            } else {
                strip_accessor(name, "get")
                    .or_else(|| strip_accessor(name, "set"))
                    .map_or_else(Vec::new, |rest| vec![decapitalise(rest)])
            }
        }
        _ => Vec::new(),
    }
}

/// The JVM class name of a Kotlin file's top-level declarations: `ShippingKt` for
/// `Shipping.kt`, unless `@file:JvmName` says otherwise.
fn facade_name(file: &crate::index::IndexedFile) -> Option<String> {
    if let Some(name) = &file.facts.jvm_name {
        return Some(name.clone());
    }
    let stem = file.path.file_stem()?.to_str()?;
    let identifier: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(format!("{}Kt", capitalise(&identifier)))
}

fn scalar_group(name: &str) -> Option<u8> {
    match name {
        "String" => Some(0),
        "Int" | "Long" | "Short" | "Byte" | "Double" | "Float" | "Char" | "int" | "long"
        | "short" | "byte" | "double" | "float" | "char" | "Integer" | "Character" => Some(1),
        "Boolean" | "boolean" => Some(2),
        _ => None,
    }
}

pub(crate) fn simple_name(type_name: &str) -> &str {
    type_name.rsplit('.').next().unwrap_or(type_name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    No,
    Maybe,
    Exact,
}

#[derive(Clone, PartialEq, Eq)]
enum Receiver {
    Function,
    Facade(Vec<crate::index::FileId>),
    Types(Vec<SymbolId>),
    External(String),
    Unknown,
}

struct Lookup<'a> {
    site: Site<'a>,
    resolver: Resolver<'a>,
    enclosing: Vec<SymbolId>,
    /// Inside a Kotlin extension, `this` is its receiver, ahead of the enclosing types.
    extension_receiver: Option<Receiver>,
    lambda_receivers: Vec<Receiver>,
    it_receiver: Option<Receiver>,
    lambda_parameters: Vec<(ByteRange, Receiver)>,
}

#[derive(Clone, Copy)]
struct Query<'q> {
    name: &'q str,
    role: ReferenceRole,
    qualifier: Option<&'q Qualifier>,
    arguments: Option<&'q [Argument]>,
    at: ByteOffset,
    depth: usize,
}

struct CallSite<'q> {
    arguments: &'q [Argument],
    types: OnceCell<Vec<Receiver>>,
    at: ByteOffset,
    depth: usize,
}

#[derive(Default)]
struct Found {
    local: Option<LocalDefinition>,
    ranked: Vec<(SymbolId, Tier)>,
    confident: bool,
}

impl Found {
    fn sure(ranked: Vec<(SymbolId, Tier)>) -> Self {
        Self {
            local: None,
            ranked,
            confident: true,
        }
    }

    fn guess(ranked: Vec<(SymbolId, Tier)>) -> Self {
        Self {
            local: None,
            ranked,
            confident: false,
        }
    }

    fn rank(&mut self) -> usize {
        self.ranked.sort_by_key(|(id, tier)| (*tier, *id));
        self.ranked.dedup();
        let best = self.ranked.first().map(|(_, tier)| *tier);
        self.ranked
            .iter()
            .filter(|(_, tier)| Some(*tier) == best)
            .count()
    }
}

impl<'a> Lookup<'a> {
    fn new(site: Site<'a>, reference: &Reference) -> Self {
        let resolver = Resolver {
            index: site.index,
            language: site.tree.language(),
        };
        let enclosing = resolver.enclosing_types(site.file, reference.range.start);
        let mut lookup = Lookup {
            site,
            resolver,
            enclosing,
            extension_receiver: None,
            lambda_receivers: Vec::new(),
            it_receiver: None,
            lambda_parameters: Vec::new(),
        };
        let at = reference.range.start;
        lookup.extension_receiver = site
            .tree
            .extension_receiver_at(site.buffer, at)
            .map(|type_name| lookup.receiver_of_written_type(&type_name));
        let lambdas = site.tree.enclosing_lambdas(site.buffer, at);
        let mut lambda_receivers = Vec::new();
        for lambda in &lambdas {
            let target = match lambda.callee.as_str() {
                "apply" | "run" => lambda.receiver.as_ref(),
                "with" if lambda.receiver.is_none() => lambda.first_argument.as_ref(),
                _ => None,
            };
            if let Some(target) = target {
                lambda_receivers.push(lookup.receiver(target, at, 1));
            }
        }
        let parameter_of = |lambda: &dumbj_syntax::LambdaCall| {
            let receiver = lambda.receiver.as_ref()?;
            let callee = lambda.callee.as_str();
            if matches!(callee, "let" | "also" | "takeIf" | "takeUnless") {
                Some(lookup.receiver(receiver, at, 1))
            } else if ELEMENT_LAMBDAS.contains(&callee) {
                Some(lookup.element_of(receiver, at, 1))
            } else {
                None
            }
        };
        let it_receiver = lambdas
            .iter()
            .find(|lambda| !lambda.has_parameters)
            .and_then(parameter_of);
        let lambda_parameters = lambdas
            .iter()
            .filter_map(|lambda| Some((lambda.first_parameter.clone()?, parameter_of(lambda)?)))
            .collect();
        lookup.it_receiver = it_receiver;
        lookup.lambda_parameters = lambda_parameters;
        lookup.lambda_receivers = lambda_receivers;
        lookup
    }
}

impl Lookup<'_> {
    fn target(&self, id: SymbolId, tier: Tier) -> Option<Target> {
        let symbol = self.resolver.symbol(id)?;
        let path = self.site.index.file(id.file)?.path.clone();
        Some(Target {
            path,
            range: symbol.name_range.clone(),
            symbol: Some(id),
            tier,
        })
    }

    fn receiver_of_type(
        type_name: Option<&String>,
        resolve: impl Fn(&str) -> Vec<SymbolId>,
    ) -> Receiver {
        match type_name {
            None => Receiver::Unknown,
            Some(type_name) => {
                let types = resolve(type_name);
                if types.is_empty() {
                    Receiver::External(type_name.clone())
                } else {
                    Receiver::Types(types)
                }
            }
        }
    }

    fn receiver_of_symbol(&self, symbol: SymbolId) -> Receiver {
        let type_name = self
            .resolver
            .symbol(symbol)
            .and_then(|symbol| symbol.type_name.as_ref());
        Self::receiver_of_type(type_name, |name| {
            self.resolver
                .types_named(symbol, Some(&name.to_owned()))
                .unwrap_or_default()
        })
    }

    fn receiver_of_written_type(&self, type_name: &String) -> Receiver {
        Self::receiver_of_type(Some(type_name), |name| {
            self.resolver
                .resolve_type(self.site.file, &self.enclosing, name)
        })
    }

    fn receiver_of_name(&self, name: &str, at: ByteOffset, depth: usize) -> Receiver {
        let Site {
            tree, buffer, file, ..
        } = self.site;
        if name == "it"
            && let Some(it) = &self.it_receiver
            && tree
                .resolve_local(buffer, at, name, ReferenceRole::Value)
                .is_none()
        {
            return it.clone();
        }
        if let Some(local) = tree.resolve_local(buffer, at, name, ReferenceRole::Value) {
            if let Some((_, parameter)) = self
                .lambda_parameters
                .iter()
                .find(|(range, _)| *range == local.name_range)
            {
                return parameter.clone();
            }
            return match (&local.type_name, &local.initialiser) {
                (Some(type_name), _) => self.receiver_of_written_type(type_name),
                // Evaluated where the local is declared.
                (None, Some(initialiser)) => {
                    self.receiver(initialiser, local.name_range.start, depth + 1)
                }
                (None, None) => Receiver::Unknown,
            };
        }
        let field = self
            .enclosing
            .iter()
            .flat_map(|owner| {
                self.resolver
                    .members_named(*owner, name, ReferenceRole::Value, &any_member)
            })
            .find(|id| !self.resolver.is_type(*id));
        if let Some(field) = field {
            return self.receiver_of_symbol(field);
        }
        let types = self.resolver.resolve_type(file, &self.enclosing, name);
        if !types.is_empty() {
            return Receiver::Types(types);
        }
        let facades = self.facade_files(name);
        if !facades.is_empty() {
            return Receiver::Facade(facades);
        }
        let property = self
            .site
            .index
            .symbols_named(name)
            .iter()
            .copied()
            .find(|id| {
                self.site.index.parent(*id).is_none()
                    && self
                        .resolver
                        .symbol(*id)
                        .is_some_and(|s| s.kind == SymbolKind::Property)
            });
        property.map_or(Receiver::Unknown, |property| {
            self.receiver_of_symbol(property)
        })
    }

    fn receiver_of_member(&self, receiver: &Receiver, name: &str) -> Receiver {
        if let Receiver::Facade(files) = receiver {
            return self
                .top_level_in(files, name, ReferenceRole::Member)
                .first()
                .map_or(Receiver::Unknown, |id| self.receiver_of_symbol(*id));
        }
        let Receiver::Types(owners) = receiver else {
            return Receiver::Unknown;
        };
        if name == "INSTANCE" && self.all_objects(owners) {
            return Receiver::Types(owners.clone());
        }
        let member = owners
            .iter()
            .flat_map(|owner| {
                self.resolver
                    .members_named(*owner, name, ReferenceRole::Member, &any_member)
            })
            .next();
        let kind = member.and_then(|member| self.resolver.symbol(member).map(|s| s.kind));
        match (member, kind) {
            (Some(member), Some(kind)) if kind.is_type() => Receiver::Types(vec![member]),
            (Some(member), Some(SymbolKind::EnumConstant)) => self
                .site
                .index
                .parent(member)
                .map_or(Receiver::Unknown, |owner| Receiver::Types(vec![owner])),
            (Some(member), _) => self.receiver_of_symbol(member),
            (None, _) => Receiver::Unknown,
        }
    }

    fn receiver_of_call(
        &self,
        call: &dumbj_syntax::Call,
        at: ByteOffset,
        depth: usize,
    ) -> Receiver {
        let mut found = self.find(Query {
            name: &call.name,
            role: ReferenceRole::Call,
            qualifier: call.receiver.as_ref(),
            arguments: Some(&call.arguments),
            at,
            depth,
        });
        let in_best_tier = found.rank();
        if found.ranked.is_empty()
            && found.local.is_none()
            && ELEMENT_RETURNING.contains(&call.name.as_str())
            && let Some(receiver) = &call.receiver
        {
            return self.element_of(receiver, at, depth + 1);
        }
        if found.local.is_some() || !found.confident {
            return Receiver::Unknown;
        }
        if found.ranked.is_empty() {
            let constructs = call.receiver.is_none()
                && self.site.tree.language() == Language::Kotlin
                && call.name.starts_with(char::is_uppercase);
            return if constructs {
                Receiver::External(call.name.clone())
            } else {
                Receiver::Unknown
            };
        }
        let mut results = found.ranked.iter().take(in_best_tier).map(|(id, _)| {
            match self.resolver.symbol(*id).map(|symbol| symbol.kind) {
                Some(kind) if kind.is_type() => Receiver::Types(vec![*id]),
                Some(SymbolKind::Constructor) => self
                    .site
                    .index
                    .parent(*id)
                    .map_or(Receiver::Unknown, |owner| Receiver::Types(vec![owner])),
                _ => self.receiver_of_symbol(*id),
            }
        });
        let Some(first) = results.next() else {
            return Receiver::Unknown;
        };
        if results.all(|other| other == first) {
            first
        } else {
            Receiver::Unknown
        }
    }

    fn element_of_symbol(&self, symbol: SymbolId) -> Receiver {
        let argument = self
            .resolver
            .symbol(symbol)
            .and_then(|symbol| symbol.type_argument.as_ref());
        Self::receiver_of_type(argument, |name| {
            self.resolver
                .types_named(symbol, Some(&name.to_owned()))
                .unwrap_or_default()
        })
    }

    fn element_of_member(&self, owner: &Receiver, name: &str) -> Receiver {
        let Receiver::Types(owners) = owner else {
            return Receiver::Unknown;
        };
        owners
            .iter()
            .flat_map(|owner| {
                self.resolver
                    .members_named(*owner, name, ReferenceRole::Member, &any_member)
            })
            .next()
            .map_or(Receiver::Unknown, |member| self.element_of_symbol(member))
    }

    fn element_of(&self, expression: &Qualifier, at: ByteOffset, depth: usize) -> Receiver {
        if depth > MAX_EXPRESSION_DEPTH {
            return Receiver::Unknown;
        }
        let Site { tree, buffer, .. } = self.site;
        match expression {
            Qualifier::Name(name) => {
                if let Some(local) = tree.resolve_local(buffer, at, name, ReferenceRole::Value) {
                    return match (&local.type_argument, &local.initialiser) {
                        (Some(argument), _) => self.receiver_of_written_type(argument),
                        (None, Some(initialiser)) => {
                            self.element_of(initialiser, local.name_range.start, depth + 1)
                        }
                        (None, None) => Receiver::Unknown,
                    };
                }
                let field = self
                    .enclosing
                    .iter()
                    .flat_map(|owner| {
                        self.resolver
                            .members_named(*owner, name, ReferenceRole::Value, &any_member)
                    })
                    .find(|id| !self.resolver.is_type(*id));
                field.map_or(Receiver::Unknown, |field| self.element_of_symbol(field))
            }
            Qualifier::Path(path) => match path.rsplit_once('.') {
                Some((prefix, last)) => {
                    let owner = if prefix.contains('.') {
                        Qualifier::Path(prefix.to_owned())
                    } else {
                        Qualifier::Name(prefix.to_owned())
                    };
                    self.element_of_member(&self.receiver(&owner, at, depth + 1), last)
                }
                None => Receiver::Unknown,
            },
            Qualifier::Member(inner, name) => {
                self.element_of_member(&self.receiver(inner, at, depth + 1), name)
            }
            Qualifier::Call(call) => {
                let name = call.name.as_str();
                if ELEMENT_BUILDERS.contains(&name) {
                    return call
                        .arguments
                        .first()
                        .map_or(Receiver::Unknown, |argument| {
                            self.argument_type(&argument.value, at, depth + 1)
                        });
                }
                if ELEMENT_PRESERVING.contains(&name)
                    && let Some(receiver) = &call.receiver
                {
                    return self.element_of(receiver, at, depth + 1);
                }
                let mut found = self.find(Query {
                    name,
                    role: ReferenceRole::Call,
                    qualifier: call.receiver.as_ref(),
                    arguments: Some(&call.arguments),
                    at,
                    depth: depth + 1,
                });
                found.rank();
                match found.ranked.as_slice() {
                    [(only, _)] if found.confident && found.local.is_none() => {
                        self.element_of_symbol(*only)
                    }
                    _ => Receiver::Unknown,
                }
            }
            _ => Receiver::Unknown,
        }
    }

    fn receiver(&self, expression: &Qualifier, at: ByteOffset, depth: usize) -> Receiver {
        if depth > MAX_EXPRESSION_DEPTH {
            return Receiver::Unknown;
        }
        let types = |types: Vec<SymbolId>| {
            if types.is_empty() {
                Receiver::Unknown
            } else {
                Receiver::Types(types)
            }
        };
        match expression {
            Qualifier::This => match self
                .lambda_receivers
                .first()
                .or(self.extension_receiver.as_ref())
            {
                Some(Receiver::Types(receiver)) => Receiver::Types(receiver.clone()),
                Some(Receiver::External(name)) => Receiver::External(name.clone()),
                _ => types(self.enclosing.clone()),
            },
            Qualifier::Super => types(
                self.enclosing
                    .first()
                    .map_or_else(Vec::new, |own| self.resolver.supertypes(*own)),
            ),
            Qualifier::Name(name) => self.receiver_of_name(name, at, depth),
            Qualifier::Path(path) => {
                let named = self
                    .resolver
                    .resolve_type(self.site.file, &self.enclosing, path);
                if !named.is_empty() {
                    return Receiver::Types(named);
                }
                let mut parts = path.split('.');
                let mut receiver = parts.next().map_or(Receiver::Unknown, |first| {
                    self.receiver_of_name(first, at, depth)
                });
                for part in parts {
                    receiver = self.receiver_of_member(&receiver, part);
                }
                receiver
            }
            Qualifier::Member(inner, name) => {
                self.receiver_of_member(&self.receiver(inner, at, depth + 1), name)
            }
            Qualifier::Call(call) => self.receiver_of_call(call, at, depth + 1),
            Qualifier::Typed(type_name) => self.receiver_of_written_type(type_name),
            Qualifier::Lambda => Receiver::Function,
            Qualifier::Other => Receiver::Unknown,
        }
    }

    fn named_candidates(&self, name: &str, role: ReferenceRole) -> Vec<SymbolId> {
        self.site
            .index
            .symbols_named(name)
            .iter()
            .copied()
            .filter(|id| {
                self.resolver.symbol(*id).is_some_and(|symbol| {
                    kind_fits(symbol.kind, role, self.resolver.language)
                        || (role == ReferenceRole::Call && self.is_record_component(*id))
                })
            })
            .collect()
    }

    fn is_record_component(&self, id: SymbolId) -> bool {
        let kind = |id: SymbolId| self.resolver.symbol(id).map(|symbol| symbol.kind);
        kind(id) == Some(SymbolKind::Field)
            && self.site.index.parent(id).and_then(kind) == Some(SymbolKind::Record)
    }

    /// A bare type name passes a companion (`foo(Job)`), not a value of that type, unless it is an `object`.
    fn argument_type(&self, value: &Qualifier, at: ByteOffset, depth: usize) -> Receiver {
        let receiver = self.receiver(value, at, depth);
        let (Qualifier::Name(name) | Qualifier::Path(name), Receiver::Types(types)) =
            (value, &receiver)
        else {
            return receiver;
        };
        let names_them = self
            .resolver
            .resolve_type(self.site.file, &self.enclosing, name)
            == *types;
        let all_objects = types.iter().all(|id| {
            self.resolver
                .symbol(*id)
                .is_some_and(|symbol| symbol.kind == SymbolKind::Object)
        });
        if names_them && !all_objects {
            Receiver::Unknown
        } else {
            receiver
        }
    }

    /// Only a certain mismatch is `No`: library types are invisible to the index.
    fn type_fit(&self, argument: &Receiver, callee: SymbolId, parameter: &Parameter) -> Fit {
        let Some(written) = &parameter.type_name else {
            return Fit::Maybe;
        };
        let simple = simple_name(written);
        let declared = self
            .resolver
            .types_named(callee, Some(written))
            .unwrap_or_default();
        let kind = |id: &SymbolId| self.resolver.symbol(*id).map(|symbol| symbol.kind);
        let is_alias =
            |ids: &[SymbolId]| ids.iter().any(|id| kind(id) == Some(SymbolKind::TypeAlias));
        let unless = |ruled_out: bool| if ruled_out { Fit::No } else { Fit::Maybe };
        match argument {
            Receiver::Unknown | Receiver::Facade(_) => Fit::Maybe,
            Receiver::Function if declared.is_empty() => unless(scalar_group(simple).is_some()),
            // A lambda converts to an interface or function type alias, not a class.
            Receiver::Function => unless(declared.iter().all(|id| {
                !matches!(
                    kind(id),
                    Some(SymbolKind::Interface | SymbolKind::TypeAlias) | None
                )
            })),
            Receiver::External(name) if simple_name(name) == simple => Fit::Exact,
            Receiver::External(_) if !declared.is_empty() => unless(!is_alias(&declared)),
            Receiver::External(name) => {
                let groups = (scalar_group(simple_name(name)), scalar_group(simple));
                unless(matches!(groups, (Some(a), Some(b)) if a != b))
            }
            Receiver::Types(types) if is_alias(types) || is_alias(&declared) => Fit::Maybe,
            Receiver::Types(_) if declared.is_empty() => unless(scalar_group(simple).is_some()),
            Receiver::Types(types) if types.iter().all(|id| declared.contains(id)) => Fit::Exact,
            Receiver::Types(types) => {
                let converts = types.iter().any(|id| {
                    self.resolver
                        .with_supertypes(*id)
                        .iter()
                        .any(|ancestor| declared.contains(ancestor))
                });
                unless(!converts)
            }
        }
    }

    fn fit(&self, call: &CallSite<'_>, id: SymbolId) -> Fit {
        let Some(symbol) = self.resolver.symbol(id) else {
            return Fit::No;
        };
        let Some(declared) = &symbol.parameters else {
            return Fit::Maybe;
        };
        // Java passes a Kotlin extension's receiver as the first argument.
        let receiver = symbol
            .receiver
            .as_ref()
            .filter(|_| self.resolver.language == Language::Java)
            .map(|receiver| Parameter {
                name: String::new(),
                type_name: Some(receiver.clone()),
                optional: false,
                variadic: false,
            });
        let parameters: Vec<&Parameter> = receiver.iter().chain(declared).collect();
        let types = call.types.get_or_init(|| {
            call.arguments
                .iter()
                .map(|argument| self.argument_type(&argument.value, call.at, call.depth + 1))
                .collect()
        });
        let mut bound = vec![false; parameters.len()];
        let mut next_positional = Some(0);
        let mut fit = Fit::Exact;
        for (argument, argument_type) in call.arguments.iter().zip(types) {
            let slot = if argument.trailing {
                parameters.len().checked_sub(1)
            } else if let Some(name) = &argument.name {
                next_positional = None;
                parameters.iter().position(|p| p.name == *name)
            } else if let Some(slot) = next_positional {
                if parameters.get(slot).is_some_and(|p| !p.variadic) {
                    next_positional = Some(slot + 1);
                }
                Some(slot).filter(|slot| *slot < parameters.len())
            } else {
                return Fit::Maybe;
            };
            let Some((parameter, is_bound)) =
                slot.and_then(|slot| parameters.get(slot).copied().zip(bound.get_mut(slot)))
            else {
                return Fit::No;
            };
            *is_bound = true;
            fit = fit.min(self.type_fit(argument_type, id, parameter));
            if fit == Fit::No {
                return Fit::No;
            }
        }
        let missing = parameters
            .iter()
            .zip(&bound)
            .any(|(parameter, bound)| !bound && !parameter.optional && !parameter.variadic);
        if missing { Fit::No } else { fit }
    }

    /// A class with a primary constructor, or none, stands for that constructor.
    fn callables_of(&self, id: SymbolId) -> Vec<SymbolId> {
        if !self.resolver.is_type(id) {
            return vec![id];
        }
        let constructors: Vec<SymbolId> = self
            .site
            .index
            .members(id)
            .filter(|(_, symbol)| symbol.kind == SymbolKind::Constructor)
            .map(|(id, _)| id)
            .collect();
        let has_own = self
            .resolver
            .symbol(id)
            .is_some_and(|symbol| symbol.parameters.is_some());
        let own = (has_own || constructors.is_empty()).then_some(id);
        own.into_iter().chain(constructors).collect()
    }

    /// An interface or object: `Name(…)` would be a SAM constructor or `invoke`.
    fn is_uncallable_type(&self, id: SymbolId) -> bool {
        self.resolver.symbol(id).is_some_and(|symbol| {
            symbol.parameters.is_none()
                && matches!(
                    symbol.kind,
                    SymbolKind::Interface
                        | SymbolKind::Object
                        | SymbolKind::Companion
                        | SymbolKind::TypeAlias
                )
        })
    }

    fn select_overload(&self, found: &mut Found, call: &CallSite<'_>) {
        let mut fits: Vec<(SymbolId, Tier, Fit)> = found
            .ranked
            .iter()
            .flat_map(|(id, tier)| {
                self.callables_of(*id)
                    .into_iter()
                    .map(|callable| (callable, *tier, self.fit(call, callable)))
            })
            .collect();
        // Nothing fits: the model is missing something, so keep everything.
        if fits.iter().any(|(_, _, fit)| *fit != Fit::No) {
            fits.retain(|(_, _, fit)| *fit != Fit::No);
        }
        let callable_tier = fits
            .iter()
            .filter(|(id, _, _)| !self.is_uncallable_type(*id))
            .map(|(_, tier, _)| *tier)
            .min();
        fits.retain(|(id, tier, _)| {
            !(self.is_uncallable_type(*id) && callable_tier.is_some_and(|best| best <= *tier))
        });
        if let Some(best) = fits.iter().map(|(_, tier, _)| *tier).min() {
            let exact =
                |(_, tier, fit): &(SymbolId, Tier, Fit)| *tier == best && *fit == Fit::Exact;
            if !call.arguments.is_empty() && fits.iter().filter(|c| exact(c)).count() == 1 {
                fits.retain(|candidate| candidate.1 != best || exact(candidate));
            }
        }
        found.ranked = fits.into_iter().map(|(id, tier, _)| (id, tier)).collect();
    }

    fn all_objects(&self, types: &[SymbolId]) -> bool {
        !types.is_empty()
            && types.iter().all(|id| {
                self.resolver
                    .symbol(*id)
                    .is_some_and(|symbol| symbol.kind == SymbolKind::Object)
            })
    }

    fn facade_files(&self, name: &str) -> Vec<crate::index::FileId> {
        let index = self.site.index;
        let facades = index.facades.get_or_init(|| {
            let mut map: std::collections::HashMap<String, Vec<crate::index::FileId>> =
                std::collections::HashMap::new();
            for (id, file) in index.files() {
                let has_top_level = file
                    .facts
                    .symbols
                    .iter()
                    .any(|symbol| symbol.parent.is_none() && !symbol.kind.is_type());
                if file.language == Some(Language::Kotlin)
                    && has_top_level
                    && let Some(facade) = facade_name(file)
                {
                    map.entry(facade).or_default().push(id);
                }
            }
            map
        });
        let Some(all) = facades.get(name).cloned() else {
            return Vec::new();
        };
        let here = self.resolver.facts(self.site.file);
        let own_package = here
            .and_then(|facts| facts.package.as_deref())
            .unwrap_or("");
        let visible: Vec<crate::index::FileId> = all
            .iter()
            .copied()
            .filter(|id| {
                let package = index
                    .file(*id)
                    .and_then(|file| file.facts.package.as_deref())
                    .unwrap_or("");
                let fqn = if package.is_empty() {
                    name.to_owned()
                } else {
                    format!("{package}.{name}")
                };
                package == own_package
                    || here.is_some_and(|facts| {
                        facts.imports.iter().any(|import| {
                            import.path == fqn || (import.wildcard && import.path == package)
                        })
                    })
            })
            .collect();
        if visible.is_empty() { all } else { visible }
    }

    fn top_level_in(
        &self,
        files: &[crate::index::FileId],
        name: &str,
        role: ReferenceRole,
    ) -> Vec<SymbolId> {
        let language = self.resolver.language;
        let lookup = |name: &str, role: ReferenceRole| -> Vec<SymbolId> {
            files
                .iter()
                .filter_map(|file| Some((*file, self.site.index.file(*file)?)))
                .flat_map(|(file, indexed)| {
                    indexed.facts.symbols.iter().enumerate().filter_map(
                        move |(position, symbol)| {
                            Some((
                                SymbolId {
                                    file,
                                    index: u32::try_from(position).ok()?,
                                },
                                symbol,
                            ))
                        },
                    )
                })
                .filter(|(_, symbol)| {
                    symbol.parent.is_none()
                        && !symbol.kind.is_type()
                        && fits_reference(symbol, name, role, language)
                })
                .map(|(id, _)| id)
                .collect()
        };
        let found = lookup(name, role);
        if !found.is_empty() {
            return found;
        }
        accessor_alternatives(name, role, self.resolver.language)
            .into_iter()
            .map(|(alternative, role)| lookup(&alternative, role))
            .find(|found| !found.is_empty())
            .unwrap_or_default()
    }

    fn is_extension(&self, id: SymbolId) -> bool {
        self.resolver
            .symbol(id)
            .is_some_and(|symbol| symbol.receiver.is_some())
    }

    fn is_member(&self, id: SymbolId) -> bool {
        self.site.index.parent(id).is_some()
    }

    fn find(&self, query: Query<'_>) -> Found {
        let call = query
            .arguments
            .filter(|_| query.role == ReferenceRole::Call)
            .map(|arguments| CallSite {
                arguments,
                types: OnceCell::new(),
                at: query.at,
                depth: query.depth,
            });
        let applicable = |id: SymbolId| {
            call.as_ref().is_none_or(|call| {
                self.callables_of(id)
                    .into_iter()
                    .any(|callable| self.fit(call, callable) != Fit::No)
            })
        };
        let mut found = match query.qualifier {
            Some(qualifier) => self.find_qualified(query, qualifier, &applicable),
            None => self.find_unqualified(query, &applicable),
        };
        if let Some(call) = call.as_ref().filter(|_| found.local.is_none()) {
            self.select_overload(&mut found, call);
        }
        self.select_platform(&mut found);
        found
    }

    /// `core/jvm` for `core/jvm/src/A.kt` against `core/common/src/A.kt`.
    fn source_set<'p>(path: &'p str, other: &str) -> &'p str {
        let shared: usize = path
            .split('/')
            .zip(other.split('/'))
            .take_while(|(a, b)| a == b)
            .map(|(part, _)| part.len() + 1)
            .sum();
        let end = path
            .get(shared..)
            .and_then(|rest| rest.find('/'))
            .map_or(path.len(), |slash| shared + slash);
        path.get(..end).unwrap_or(path)
    }

    /// The `actual` in the reference's source set, otherwise the `expect` common code sees.
    fn select_platform(&self, found: &mut Found) {
        let platform = |id: SymbolId| self.resolver.symbol(id).and_then(|s| s.platform);
        let relative = |id: SymbolId| {
            self.site
                .index
                .file(id.file)
                .map_or("", |file| file.relative.as_str())
        };
        let signature = |id: SymbolId| {
            let symbol = self.resolver.symbol(id);
            let parameters: Option<Vec<Option<&str>>> = symbol
                .and_then(|symbol| symbol.parameters.as_ref())
                .map(|all| all.iter().map(|p| p.type_name.as_deref()).collect());
            (
                self.site.index.fqn(id),
                symbol.and_then(|symbol| symbol.receiver.as_deref()),
                parameters,
            )
        };
        let expects: Vec<SymbolId> = found
            .ranked
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| platform(*id) == Some(Platform::Expect))
            .collect();
        let here = self
            .site
            .index
            .file(self.site.file)
            .map_or("", |file| file.relative.as_str());
        for expect in expects {
            let key = signature(expect);
            let actuals: Vec<SymbolId> = found
                .ranked
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| platform(*id) == Some(Platform::Actual) && signature(*id) == key)
                .collect();
            let local = actuals.iter().copied().find(|actual| {
                let source_set = Self::source_set(relative(*actual), relative(expect));
                here.strip_prefix(source_set)
                    .is_some_and(|rest| rest.starts_with('/'))
            });
            let keep = local.unwrap_or(expect);
            found
                .ranked
                .retain(|(id, _)| *id == keep || (*id != expect && !actuals.contains(id)));
        }
    }

    fn find_unqualified(&self, query: Query<'_>, applicable: &dyn Fn(SymbolId) -> bool) -> Found {
        let Site {
            tree, buffer, file, ..
        } = self.site;
        let Query { name, role, .. } = query;
        if let Some(local) = tree.resolve_local(buffer, query.at, name, role) {
            return Found {
                local: Some(local),
                ranked: Vec::new(),
                confident: true,
            };
        }
        let aliased = self.resolver.facts(file).and_then(|facts| {
            facts
                .imports
                .iter()
                .find(|import| import.alias.as_deref() == Some(name))
        });
        if let Some(import) = aliased {
            let found = self.site.index.symbols_by_fqn(&import.path);
            return Found::sure(
                found
                    .into_iter()
                    .map(|id| (id, Tier::ExplicitImport))
                    .collect(),
            );
        }
        let in_scope: Vec<SymbolId> = self
            .implicit_receivers()
            .iter()
            .flat_map(|owner| self.resolver.members_named(*owner, name, role, applicable))
            .collect();
        if let Some(first) = in_scope.first() {
            let owner = self.site.index.parent(*first);
            return Found::sure(
                in_scope
                    .into_iter()
                    .filter(|id| self.site.index.parent(*id) == owner)
                    .map(|id| (id, Tier::Scope))
                    .collect(),
            );
        }
        // Other members need an implicit receiver the dumb engine cannot see: guesses.
        let mut visible = Vec::new();
        let mut guesses = Vec::new();
        for id in self.named_candidates(name, role) {
            let tier = self.resolver.tier(file, &self.enclosing, id, false);
            let is_type = self.resolver.is_type(id);
            if is_type || !self.is_member(id) || tier == Tier::ExplicitImport {
                visible.push((id, tier));
            } else {
                guesses.push((id, self.resolver.tier(file, &self.enclosing, id, true)));
            }
        }
        if visible.is_empty() {
            return Found::guess(guesses);
        }
        // Lambdas with receivers (`apply { }`) are invisible, so unmatched extensions are guesses.
        let receivers = self.implicit_receiver_names();
        let (sure, unsure): (Vec<_>, Vec<_>) = visible.into_iter().partition(|(id, _)| {
            self.extension_receiver_name(*id)
                .is_none_or(|receiver| receivers.contains(&receiver))
        });
        if sure.is_empty() {
            Found::guess(unsure)
        } else {
            Found::sure(sure)
        }
    }

    fn implicit_receivers(&self) -> Vec<SymbolId> {
        self.lambda_receivers
            .iter()
            .chain(&self.extension_receiver)
            .flat_map(|receiver| match receiver {
                Receiver::Types(types) => types.as_slice(),
                _ => &[],
            })
            .chain(&self.enclosing)
            .copied()
            .collect()
    }

    fn implicit_receiver_names(&self) -> Vec<&str> {
        let external = self
            .lambda_receivers
            .iter()
            .chain(&self.extension_receiver)
            .filter_map(|receiver| match receiver {
                Receiver::External(name) => Some(simple_name(name)),
                _ => None,
            });
        let indexed = self.type_names(&self.implicit_receivers());
        external.chain(indexed).collect()
    }

    fn type_names(&self, types: &[SymbolId]) -> Vec<&str> {
        types
            .iter()
            .flat_map(|owner| self.resolver.with_supertypes(*owner))
            .filter_map(|id| self.resolver.symbol(id))
            .map(|symbol| symbol.name.as_str())
            .collect()
    }

    fn extension_receiver_name(&self, id: SymbolId) -> Option<&str> {
        let receiver = self.resolver.symbol(id)?.receiver.as_deref()?;
        Some(simple_name(receiver))
    }

    /// Matched by simple name, so only an extension the file can see is a sure match.
    fn extensions_on(&self, name: &str, role: ReferenceRole, receiver_names: &[&str]) -> Found {
        if self.resolver.language != Language::Kotlin {
            return Found::sure(Vec::new());
        }
        let extensions: Vec<(SymbolId, Tier, &str)> = self
            .named_candidates(name, role)
            .into_iter()
            .filter_map(|id| {
                let tier = self
                    .resolver
                    .tier(self.site.file, &self.enclosing, id, false);
                Some((id, tier, self.extension_receiver_name(id)?))
            })
            .collect();
        let on = |receiver_name: &str, visible_only: bool| -> Vec<(SymbolId, Tier)> {
            extensions
                .iter()
                .filter(|(_, tier, on)| {
                    *on == receiver_name && !(visible_only && *tier > Tier::WildcardImport)
                })
                .map(|(id, tier, _)| (*id, *tier))
                .collect()
        };
        let nearest = |visible_only: bool| {
            receiver_names
                .iter()
                .map(|receiver_name| on(receiver_name, visible_only))
                .find(|found| !found.is_empty())
        };
        match (nearest(true), nearest(false)) {
            (Some(visible), _) => Found::sure(visible),
            (None, Some(unseen)) => Found::guess(unseen),
            (None, None) => Found::sure(Vec::new()),
        }
    }

    fn find_in_catalog(&self, qualifier: &Qualifier, name: &str) -> Option<Found> {
        let (Qualifier::Name(path) | Qualifier::Path(path)) = qualifier else {
            return None;
        };
        let index = self.site.index;
        let in_build_script = index
            .file(self.site.file)
            .is_some_and(|file| file.relative.ends_with(".gradle.kts"));
        let mut parts: Vec<&str> = path.split('.').collect();
        if !in_build_script || parts.first() != Some(&"libs") {
            return None;
        }
        parts.remove(0);
        parts.push(name);
        let (section, rest) = match parts.split_first() {
            Some((first, rest)) if matches!(*first, "plugins" | "versions" | "bundles") => {
                (*first, rest)
            }
            _ => ("libraries", parts.as_slice()),
        };
        let accessor = rest.join(".");
        let mut exact = Vec::new();
        let mut partial = Vec::new();
        for (file, indexed) in index.files() {
            if !indexed.relative.ends_with(".versions.toml") {
                continue;
            }
            for (position, symbol) in indexed.facts.symbols.iter().enumerate() {
                let in_section = symbol
                    .parent
                    .and_then(|parent| indexed.facts.symbols.get(parent))
                    .is_some_and(|table| table.name == section);
                if symbol.kind != SymbolKind::Key || !in_section {
                    continue;
                }
                let key = symbol.name.trim_matches('"').replace(['-', '_'], ".");
                let id = SymbolId {
                    file,
                    index: u32::try_from(position).ok()?,
                };
                if key == accessor {
                    exact.push((id, Tier::Scope));
                } else if key.starts_with(&format!("{accessor}.")) {
                    partial.push((id, Tier::Scope));
                }
            }
        }
        Some(if exact.is_empty() {
            Found::guess(partial)
        } else {
            Found::sure(exact)
        })
    }

    fn find_qualified(
        &self,
        query: Query<'_>,
        qualifier: &Qualifier,
        applicable: &dyn Fn(SymbolId) -> bool,
    ) -> Found {
        let Query { name, role, .. } = query;
        if let Some(found) = self.find_in_catalog(qualifier, name) {
            return found;
        }
        let file = self.site.file;
        if role == ReferenceRole::Type
            && let Qualifier::Name(prefix) | Qualifier::Path(prefix) = qualifier
        {
            let types =
                self.resolver
                    .resolve_type(file, &self.enclosing, &format!("{prefix}.{name}"));
            return Found::sure(
                types
                    .into_iter()
                    .map(|id| (id, self.resolver.tier(file, &self.enclosing, id, true)))
                    .collect(),
            );
        }
        let guess_all = || {
            Found::guess(
                self.named_candidates(name, role)
                    .into_iter()
                    .filter(|id| self.is_member(*id) || self.is_extension(*id))
                    .map(|id| (id, self.resolver.tier(file, &self.enclosing, id, true)))
                    .collect(),
            )
        };
        match self.receiver(qualifier, query.at, query.depth) {
            Receiver::Facade(files) => Found::sure(
                self.top_level_in(&files, name, role)
                    .into_iter()
                    .map(|id| (id, Tier::Scope))
                    .collect(),
            ),
            Receiver::Types(owners) => {
                let members: Vec<SymbolId> = owners
                    .iter()
                    .flat_map(|owner| self.resolver.members_named(*owner, name, role, applicable))
                    .collect();
                if !members.is_empty() {
                    return Found::sure(members.into_iter().map(|id| (id, Tier::Scope)).collect());
                }
                if name == "INSTANCE" && self.all_objects(&owners) {
                    return Found::sure(owners.into_iter().map(|id| (id, Tier::Scope)).collect());
                }
                // Otherwise inherited from a library supertype; namesakes elsewhere would be wrong.
                self.extensions_on(name, role, &self.type_names(&owners))
            }
            // A library type: only an extension on it can match.
            Receiver::External(type_name) => {
                let simple = simple_name(&type_name);
                let extensions = self.extensions_on(name, role, &[simple]);
                // Reached through a smart cast the engine cannot see.
                if extensions.ranked.is_empty() && matches!(simple, "Any" | "Object") {
                    guess_all()
                } else {
                    extensions
                }
            }
            Receiver::Unknown | Receiver::Function => guess_all(),
        }
    }

    fn find_import(&self, reference: &Reference) -> Found {
        let prefix = match &reference.qualifier {
            Some(Qualifier::Name(prefix) | Qualifier::Path(prefix)) => format!("{prefix}."),
            _ => String::new(),
        };
        let fqn = format!("{prefix}{}", reference.name);
        let found = self.site.index.symbols_by_fqn(&fqn);
        Found::sure(
            found
                .into_iter()
                .map(|id| (id, Tier::ExplicitImport))
                .collect(),
        )
    }

    fn finish(&self, reference: Reference, mut found: Found) -> Resolution {
        if let Some(local) = found.local {
            let path = self
                .site
                .index
                .file(self.site.file)
                .map_or_else(PathBuf::new, |file| file.path.clone());
            let target = Target {
                path,
                range: local.name_range,
                symbol: None,
                tier: Tier::Local,
            };
            return Resolution {
                reference,
                targets: vec![target],
                decision: Decision::Jump,
            };
        }
        let in_best_tier = found.rank();
        let targets: Vec<Target> = found
            .ranked
            .iter()
            .filter_map(|(id, tier)| self.target(*id, *tier))
            .collect();
        let decision = match (targets.len(), in_best_tier) {
            (0, _) => Decision::NotFound,
            (_, 1) if found.confident => Decision::Jump,
            _ => Decision::Choose,
        };
        Resolution {
            reference,
            targets,
            decision,
        }
    }
}

pub(crate) fn resolve_supertypes(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    let language = index
        .file(id.file)
        .and_then(|file| file.language)
        .unwrap_or(Language::Java);
    Resolver { index, language }.supertypes(id)
}

/// `None` when there is no identifier at `offset`.
#[must_use]
pub fn resolve(site: Site<'_>, offset: ByteOffset) -> Option<Resolution> {
    let reference = site.tree.reference_at(site.buffer, offset)?;
    let lookup = Lookup::new(site, &reference);
    let found = if reference.role == ReferenceRole::Declaration {
        Found::sure(Vec::new())
    } else if reference.in_import {
        lookup.find_import(&reference)
    } else {
        lookup.find(Query {
            name: &reference.name,
            role: reference.role,
            qualifier: reference.qualifier.as_ref(),
            arguments: reference.arguments.as_deref(),
            at: reference.range.start,
            depth: 0,
        })
    };
    Some(lookup.finish(reference, found))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTarget {
    pub class: String,
    pub member: Option<String>,
}

fn implicit_packages(language: Language) -> &'static [&'static str] {
    match language {
        Language::Kotlin => &[
            "kotlin",
            "kotlin.collections",
            "kotlin.ranges",
            "kotlin.sequences",
            "kotlin.text",
            "kotlin.io",
            "kotlin.annotation",
            "kotlin.comparisons",
            "kotlin.jvm",
            "java.lang",
        ],
        _ => &["java.lang"],
    }
}

impl Lookup<'_> {
    /// The classes a simple or qualified type name written here may mean, best first.
    fn library_classes(&self, type_name: &str) -> Vec<String> {
        let (first, rest) = type_name
            .split_once('.')
            .map_or((type_name, None), |(first, rest)| (first, Some(rest)));
        if !first.starts_with(char::is_uppercase) {
            return vec![type_name.to_owned()];
        }
        let nested = |outer: String| match rest {
            Some(rest) => format!("{outer}.{rest}"),
            None => outer,
        };
        let facts = self.resolver.facts(self.site.file);
        let imports = facts.map_or(&[][..], |facts| facts.imports.as_slice());
        if let Some(import) = imports
            .iter()
            .find(|import| import.local_name() == Some(first))
        {
            return vec![nested(import.path.clone())];
        }
        let own = facts.and_then(|facts| facts.package.clone());
        let wildcards = imports
            .iter()
            .filter(|import| import.wildcard)
            .map(|import| import.path.clone());
        own.into_iter()
            .chain(wildcards)
            .chain(
                implicit_packages(self.resolver.language)
                    .iter()
                    .map(|package| (*package).to_owned()),
            )
            .map(|package| nested(format!("{package}.{first}")))
            .collect()
    }

    fn library_targets(&self, reference: &Reference) -> Vec<LibraryTarget> {
        let class = |class: String, member: Option<&str>| LibraryTarget {
            class,
            member: member.map(str::to_owned),
        };
        if reference.in_import {
            let prefix = match &reference.qualifier {
                Some(Qualifier::Name(prefix) | Qualifier::Path(prefix)) => prefix.clone(),
                _ => String::new(),
            };
            let full = if prefix.is_empty() {
                reference.name.clone()
            } else {
                format!("{prefix}.{}", reference.name)
            };
            let mut found = vec![class(full, None)];
            if !prefix.is_empty() {
                found.push(class(prefix, Some(&reference.name)));
            }
            return found;
        }
        let at = reference.range.start;
        match &reference.qualifier {
            None if reference.name.starts_with(char::is_uppercase) => self
                .library_classes(&reference.name)
                .into_iter()
                .map(|name| class(name, None))
                .collect(),
            None => Vec::new(),
            Some(qualifier) => {
                let owner = match self.receiver(qualifier, at, 0) {
                    Receiver::External(type_name) => Some(type_name),
                    Receiver::Unknown => match qualifier {
                        Qualifier::Name(name) | Qualifier::Path(name)
                            if simple_name(name).starts_with(char::is_uppercase) =>
                        {
                            Some(name.clone())
                        }
                        _ => None,
                    },
                    _ => None,
                };
                owner.map_or_else(Vec::new, |owner| {
                    self.library_classes(&owner)
                        .into_iter()
                        .map(|name| class(name, Some(&reference.name)))
                        .collect()
                })
            }
        }
    }
}

/// Where a reference that the project does not declare may lead in a library, best
/// guess first. Empty when the project declares it or nothing points outside.
#[must_use]
pub fn library_targets(site: Site<'_>, offset: ByteOffset) -> Vec<LibraryTarget> {
    let Some(reference) = site.tree.reference_at(site.buffer, offset) else {
        return Vec::new();
    };
    if reference.role == ReferenceRole::Declaration {
        return Vec::new();
    }
    Lookup::new(site, &reference).library_targets(&reference)
}
