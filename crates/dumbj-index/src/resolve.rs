//! Dumb Go to Declaration: resolves the identifier under the caret with tree-sitter and
//! the symbol index only.
//!
//! The jump rule (ARCHITECTURE, "Dumb jump rule"): jump directly only when exactly one
//! candidate is in the best rank tier, its kind fits the reference and nothing about the
//! lookup was a guess. Otherwise the caller shows a chooser sorted by rank. A wrong jump is
//! worse than a chooser.

use std::cell::OnceCell;
use std::collections::HashSet;
use std::path::PathBuf;

use dumbj_syntax::{
    Argument, FileFacts, Language, LocalDefinition, Parameter, Qualifier, Reference, ReferenceRole,
    Symbol, SymbolKind, SyntaxTree,
};
use dumbj_text::{Buffer, ByteOffset, ByteRange};

use crate::index::{FileId, Index, SymbolId};

/// How close a candidate is to the reference. Earlier variants rank higher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    /// A parameter, local variable or type parameter in an enclosing scope.
    Local,
    /// A member of an enclosing type or of one of its supertypes, or a member of the
    /// receiver's known type.
    Scope,
    SameFile,
    /// Named by a single-name import (including a Kotlin alias or a Java static import).
    ExplicitImport,
    SamePackage,
    /// Covered by a wildcard import.
    WildcardImport,
    /// In the same module (Gradle project or Maven module).
    SameModule,
    /// Anywhere else in the project.
    Repo,
}

/// Where a reference may lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// The declared name; the caret goes to its start.
    pub range: ByteRange,
    /// The declaration in the index; `None` for a local.
    pub symbol: Option<SymbolId>,
    pub tier: Tier,
}

/// What Go to Declaration should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Go straight to the first target.
    Jump,
    /// Show the targets in a chooser, labelled as a best guess.
    Choose,
    NotFound,
}

/// The outcome of resolving one reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub reference: Reference,
    /// Candidates, best first.
    pub targets: Vec<Target>,
    pub decision: Decision,
}

/// The file a reference sits in, with its parsed tree.
#[derive(Debug, Clone, Copy)]
pub struct Site<'a> {
    pub index: &'a Index,
    /// The file's entry in `index`.
    pub file: FileId,
    pub tree: &'a SyntaxTree,
    pub buffer: &'a Buffer,
}

/// How deep supertype chains are followed. Guards against cycles and absurd hierarchies.
const MAX_SUPERTYPE_DEPTH: usize = 12;

/// How many calls and local initialisers are followed to find the type of one expression.
const MAX_EXPRESSION_DEPTH: usize = 8;

/// Whether a declaration of `kind` can be what a reference with `role`, written in
/// `language`, names.
fn kind_fits(kind: SymbolKind, role: ReferenceRole, language: Language) -> bool {
    match role {
        ReferenceRole::Type => kind.is_type(),
        // In Kotlin `Foo()` constructs a Foo, so a call may name a type; its constructors
        // are reached through the type (see `Lookup::callables_of`).
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

/// Lookups shared by every step of a resolution.
struct Resolver<'a> {
    index: &'a Index,
    /// The language of the file the reference is in.
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

    /// The outermost declaration enclosing `id` (or `id` itself).
    fn top_level(&self, id: SymbolId) -> SymbolId {
        let mut current = id;
        while let Some(parent) = self.index.parent(current) {
            current = parent;
        }
        current
    }

    /// The types whose bodies contain `offset` in `file`, innermost first.
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

    /// How `candidate` ranks for an unqualified name used in `file`.
    ///
    /// With `as_member`, the candidate is a member reached through some receiver, so it
    /// ranks like the top-level type that contains it.
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
        // A nested declaration is only visible unqualified through an import that names
        // it or its container, so "same package" applies to top-level subjects only.
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

    /// Keeps the candidates in the best tier.
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

    /// Resolves a type name written in `file`: simple (`Money`), nested (`Money.Currency`)
    /// or fully qualified. Returns the types in the best tier.
    fn resolve_type(&self, file: FileId, enclosing: &[SymbolId], name: &str) -> Vec<SymbolId> {
        let types = |candidates: &[SymbolId]| -> Vec<SymbolId> {
            candidates
                .iter()
                .copied()
                .filter(|id| self.is_type(*id))
                .collect()
        };
        let Some((first, rest)) = name.split_once('.') else {
            // An alias (`import a.B as C`) makes `C` mean `a.B`.
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
        // `Outer.Inner`: resolve the first part, then step into nested types.
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

    /// The supertypes of `type_id` found in the index, resolved in its own file.
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

    /// `type_id`, then its supertypes breadth first, each once.
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

    /// The members of `owner` called `name` that fit `role`: its own, those of its Kotlin
    /// companion object, or failing that the nearest supertype's.
    ///
    /// `applicable` says whether a member can take the call's arguments. Overloads may be
    /// spread over a hierarchy, so the nearest type with an applicable member wins; when
    /// no member is applicable anywhere, the nearest type that has the name does.
    fn members_named(
        &self,
        owner: SymbolId,
        name: &str,
        role: ReferenceRole,
        applicable: &dyn Fn(SymbolId) -> bool,
    ) -> Vec<SymbolId> {
        let fits =
            |symbol: &Symbol| symbol.name == name && kind_fits(symbol.kind, role, self.language);
        let mut nearest = Vec::new();
        for type_id in self.with_supertypes(owner) {
            // A Java record's components are also its accessor methods: `point.x()`.
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

    /// The types a value of declared type `type_name` has, resolved where `declared_in`
    /// is written. `None` when the type is unknown to the index.
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

/// Any member will do: the lookup is not a call, or its arguments do not matter.
fn any_member(_: SymbolId) -> bool {
    true
}

/// The group of interchangeable built-in types `name` belongs to: text, numbers (a Java
/// `char` widens to `int`) or booleans. Values never convert between groups.
fn scalar_group(name: &str) -> Option<u8> {
    match name {
        "String" => Some(0),
        "Int" | "Long" | "Short" | "Byte" | "Double" | "Float" | "Char" | "int" | "long"
        | "short" | "byte" | "double" | "float" | "char" | "Integer" | "Character" => Some(1),
        "Boolean" | "boolean" => Some(2),
        _ => None,
    }
}

fn simple_name(type_name: &str) -> &str {
    type_name.rsplit('.').next().unwrap_or(type_name)
}

/// How well a call's arguments suit one callable. Later variants are better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    /// The call cannot be to this callable: wrong argument count, an unknown parameter
    /// name, or an argument whose type certainly does not convert.
    No,
    /// Nothing rules it out.
    Maybe,
    /// Every argument has exactly the declared type of its parameter.
    Exact,
}

/// What an expression denotes, as far as the index can tell.
#[derive(PartialEq, Eq)]
enum Receiver {
    /// A lambda or a function reference.
    Function,
    /// Values of these indexed types.
    Types(Vec<SymbolId>),
    /// A value whose declared type is not in the index: a library type such as `List`.
    External(String),
    /// Nothing is known about it (a call result, an untyped local).
    Unknown,
}

/// One resolution in progress: the reference site plus the resolver.
struct Lookup<'a> {
    site: Site<'a>,
    resolver: Resolver<'a>,
    enclosing: Vec<SymbolId>,
}

/// A name to look up: the reference under the caret, or a callee inside its qualifier.
#[derive(Clone, Copy)]
struct Query<'q> {
    name: &'q str,
    role: ReferenceRole,
    qualifier: Option<&'q Qualifier>,
    /// The arguments of a call; they pick the overload.
    arguments: Option<&'q [Argument]>,
    /// Where the name is written; locals are looked up from here.
    at: ByteOffset,
    /// How many expressions deep this lookup is.
    depth: usize,
}

/// The arguments of the call being resolved, with their types worked out on first use.
struct CallSite<'q> {
    arguments: &'q [Argument],
    types: OnceCell<Vec<Receiver>>,
    at: ByteOffset,
    depth: usize,
}

/// What a lookup found.
#[derive(Default)]
struct Found {
    /// A local declaration; it shadows everything in the index.
    local: Option<LocalDefinition>,
    ranked: Vec<(SymbolId, Tier)>,
    /// False when the lookup involved a guess, which always means a chooser.
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

    /// Sorts best first and returns how many candidates share the best tier.
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

    /// What a declared type name denotes, resolved by `resolve`.
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

    /// The receiver a symbol's declared type denotes, resolved where the symbol is written.
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

    /// A value of a type written at the reference site.
    fn receiver_of_written_type(&self, type_name: &String) -> Receiver {
        Self::receiver_of_type(Some(type_name), |name| {
            self.resolver
                .resolve_type(self.site.file, &self.enclosing, name)
        })
    }

    /// What a single name written at `at` denotes: the type of a local or field called
    /// `name`, or the type `name` itself.
    fn receiver_of_name(&self, name: &str, at: ByteOffset, depth: usize) -> Receiver {
        let Site {
            tree, buffer, file, ..
        } = self.site;
        if let Some(local) = tree.resolve_local(buffer, at, name, ReferenceRole::Value) {
            return match (&local.type_name, &local.initialiser) {
                (Some(type_name), _) => self.receiver_of_written_type(type_name),
                // `val paid = Cash.of(…)`: the local has the type of its initialiser,
                // which is evaluated where the local is declared.
                (None, Some(initialiser)) => {
                    self.receiver(initialiser, local.name_range.start, depth + 1)
                }
                (None, None) => Receiver::Unknown,
            };
        }
        // A field or property of an enclosing type (or of one of its supertypes).
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
        // A Kotlin top-level property.
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

    /// What reading the member `name` of `receiver` gives.
    fn receiver_of_member(&self, receiver: &Receiver, name: &str) -> Receiver {
        let Receiver::Types(owners) = receiver else {
            return Receiver::Unknown;
        };
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
            // `Color.RED` is a `Color`.
            (Some(member), Some(SymbolKind::EnumConstant)) => self
                .site
                .index
                .parent(member)
                .map_or(Receiver::Unknown, |owner| Receiver::Types(vec![owner])),
            (Some(member), _) => self.receiver_of_symbol(member),
            (None, _) => Receiver::Unknown,
        }
    }

    /// What a call evaluates to: the callee's return type, or the constructed type.
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
        if found.local.is_some() || !found.confident {
            return Receiver::Unknown;
        }
        if found.ranked.is_empty() {
            // `ArrayList<Foo>()`: a Kotlin constructor call of a library type.
            let constructs = call.receiver.is_none()
                && self.site.tree.language() == Language::Kotlin
                && call.name.starts_with(char::is_uppercase);
            return if constructs {
                Receiver::External(call.name.clone())
            } else {
                Receiver::Unknown
            };
        }
        // Overloads may tie as long as they all return the same thing.
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

    /// What `expression`, written at `at`, denotes.
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
            Qualifier::This => types(self.enclosing.clone()),
            Qualifier::Super => types(
                self.enclosing
                    .first()
                    .map_or_else(Vec::new, |own| self.resolver.supertypes(*own)),
            ),
            Qualifier::Name(name) => self.receiver_of_name(name, at, depth),
            Qualifier::Path(path) => {
                // A fully qualified or nested type name, e.g. `com.shop.core.Money`.
                let named = self
                    .resolver
                    .resolve_type(self.site.file, &self.enclosing, path);
                if !named.is_empty() {
                    return Receiver::Types(named);
                }
                // Otherwise a chain of member accesses: `a.b.c`.
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

    /// Every declaration called `name` whose kind fits `role`.
    fn named_candidates(&self, name: &str, role: ReferenceRole) -> Vec<SymbolId> {
        self.site
            .index
            .symbols_named(name)
            .iter()
            .copied()
            .filter(|id| {
                self.resolver
                    .symbol(*id)
                    .is_some_and(|symbol| kind_fits(symbol.kind, role, self.resolver.language))
            })
            .collect()
    }

    /// The type of an argument. Unlike a receiver, a bare type name is not a value of
    /// that type (`foo(Job)` passes a companion object), unless it names an `object`.
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

    /// Whether a value of type `argument` can be passed for `parameter` of `callee`.
    /// Only a certain mismatch is `No`: library types are not known to the index.
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
        // A value of a library type is never an instance of a project type, and nothing
        // but text, a number or a boolean is passed for one.
        let unless = |ruled_out: bool| if ruled_out { Fit::No } else { Fit::Maybe };
        match argument {
            Receiver::Unknown => Fit::Maybe,
            Receiver::Function if declared.is_empty() => unless(scalar_group(simple).is_some()),
            // A lambda converts to an interface or a function type alias, not to a class.
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

    /// How well the arguments of `call` suit the callable `id`.
    fn fit(&self, call: &CallSite<'_>, id: SymbolId) -> Fit {
        let Some(symbol) = self.resolver.symbol(id) else {
            return Fit::No;
        };
        // No parameter list to check: a Java class without a constructor, a property of a
        // function type, a record accessor.
        let Some(declared) = &symbol.parameters else {
            return Fit::Maybe;
        };
        // Java calls a Kotlin extension with the receiver as the first argument.
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
                // A variadic parameter takes every remaining positional argument.
                if parameters.get(slot).is_some_and(|p| !p.variadic) {
                    next_positional = Some(slot + 1);
                }
                Some(slot).filter(|slot| *slot < parameters.len())
            } else {
                // A positional argument after a named one: not worth modelling.
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

    /// What calling the candidate `id` runs: itself, or for a type its constructors. A
    /// class with a primary constructor (or none at all) stands for that constructor.
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

    /// Whether `id` is a type that `Name(…)` cannot construct: an interface, an object.
    /// Calling it can only mean a SAM constructor or an `invoke` operator.
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

    /// Narrows the candidates of a call to those its arguments can be passed to.
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
        // If nothing fits, the model is missing something: keep every candidate.
        if fits.iter().any(|(_, _, fit)| *fit != Fit::No) {
            fits.retain(|(_, _, fit)| *fit != Fit::No);
        }
        // `Job()` is the function `Job`, not the interface next to it.
        let callable_tier = fits
            .iter()
            .filter(|(id, _, _)| !self.is_uncallable_type(*id))
            .map(|(_, tier, _)| *tier)
            .min();
        fits.retain(|(id, tier, _)| {
            !(self.is_uncallable_type(*id) && callable_tier.is_some_and(|best| best <= *tier))
        });
        // Among overloads in the best tier, one whose parameter types are exactly the
        // argument types is the most specific and wins.
        if let Some(best) = fits.iter().map(|(_, tier, _)| *tier).min() {
            let exact =
                |(_, tier, fit): &(SymbolId, Tier, Fit)| *tier == best && *fit == Fit::Exact;
            if !call.arguments.is_empty() && fits.iter().filter(|c| exact(c)).count() == 1 {
                fits.retain(|candidate| candidate.1 != best || exact(candidate));
            }
        }
        found.ranked = fits.into_iter().map(|(id, tier, _)| (id, tier)).collect();
    }

    fn is_extension(&self, id: SymbolId) -> bool {
        self.resolver
            .symbol(id)
            .is_some_and(|symbol| symbol.receiver.is_some())
    }

    /// Whether `id` is declared inside a type (a member rather than a top-level symbol).
    fn is_member(&self, id: SymbolId) -> bool {
        self.site.index.parent(id).is_some()
    }

    /// Looks `query` up among locals and in the index.
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
        found
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
        // An import alias (`import a.B as C`) names exactly one declaration.
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
        // Members of the enclosing types and of their supertypes.
        let in_scope: Vec<SymbolId> = self
            .enclosing
            .iter()
            .flat_map(|owner| self.resolver.members_named(*owner, name, role, applicable))
            .collect();
        if let Some(first) = in_scope.first() {
            // The innermost type that declares the name wins; overloads there tie.
            let owner = self.site.index.parent(*first);
            return Found::sure(
                in_scope
                    .into_iter()
                    .filter(|id| self.site.index.parent(*id) == owner)
                    .map(|id| (id, Tier::Scope))
                    .collect(),
            );
        }
        // Types, top-level functions and properties, and statically imported members
        // are visible unqualified. Other members can only be reached through an implicit
        // receiver (inherited from a library type, or a Kotlin scope function), which the
        // dumb engine cannot see: they are offered as guesses.
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
        // An extension function's receiver is not checked, so it is never a sure thing.
        let confident = !visible.iter().any(|(id, _)| self.is_extension(*id));
        Found {
            local: None,
            ranked: visible,
            confident,
        }
    }

    fn find_qualified(
        &self,
        query: Query<'_>,
        qualifier: &Qualifier,
        applicable: &dyn Fn(SymbolId) -> bool,
    ) -> Found {
        let Query { name, role, .. } = query;
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
        // Kotlin extensions declared on a type with one of these simple names.
        let extensions_on = |receiver_names: &HashSet<&str>| -> Vec<(SymbolId, Tier)> {
            self.named_candidates(name, role)
                .into_iter()
                .filter(|id| {
                    let receiver = self
                        .resolver
                        .symbol(*id)
                        .and_then(|s| s.receiver.as_deref());
                    receiver.is_some_and(|receiver| {
                        receiver_names.contains(receiver.rsplit('.').next().unwrap_or(receiver))
                    })
                })
                .map(|id| (id, self.resolver.tier(file, &self.enclosing, id, false)))
                .collect()
        };
        match self.receiver(qualifier, query.at, query.depth) {
            Receiver::Types(owners) => {
                let members: Vec<SymbolId> = owners
                    .iter()
                    .flat_map(|owner| self.resolver.members_named(*owner, name, role, applicable))
                    .collect();
                if !members.is_empty() {
                    return Found::sure(members.into_iter().map(|id| (id, Tier::Scope)).collect());
                }
                // Not a member: perhaps a Kotlin extension on the receiver's type.
                let owner_names: HashSet<&str> = owners
                    .iter()
                    .flat_map(|owner| self.resolver.with_supertypes(*owner))
                    .filter_map(|id| self.resolver.symbol(id))
                    .map(|symbol| symbol.name.as_str())
                    .collect();
                let extensions = extensions_on(&owner_names);
                if !extensions.is_empty() {
                    return Found::guess(extensions);
                }
                // The receiver's type is known and has no such member: it is inherited from a
                // library supertype (`JpaRepository.save`, `Enum.name`). Members of unrelated
                // project types with the same name would only be wrong answers.
                Found::sure(Vec::new())
            }
            // The receiver's type is written but is not in the project (a library type).
            // Its members are not ours; only an extension on that type can match.
            Receiver::External(type_name) => {
                let simple = type_name.rsplit('.').next().unwrap_or(&type_name);
                Found::guess(extensions_on(&HashSet::from([simple])))
            }
            // Unknown receiver: every member or extension with this name is a guess.
            Receiver::Unknown | Receiver::Function => Found::guess(
                self.named_candidates(name, role)
                    .into_iter()
                    .filter(|id| self.is_member(*id) || self.is_extension(*id))
                    .map(|id| (id, self.resolver.tier(file, &self.enclosing, id, true)))
                    .collect(),
            ),
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

    /// Turns what was found into a resolution.
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

/// Resolves the identifier at `offset`. Returns `None` when there is no identifier there.
#[must_use]
pub fn resolve(site: Site<'_>, offset: ByteOffset) -> Option<Resolution> {
    let reference = site.tree.reference_at(site.buffer, offset)?;
    let resolver = Resolver {
        index: site.index,
        language: site.tree.language(),
    };
    let enclosing = resolver.enclosing_types(site.file, reference.range.start);
    let lookup = Lookup {
        site,
        resolver,
        enclosing,
    };
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
