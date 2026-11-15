//! Dumb Go to Declaration: resolves the identifier under the caret with tree-sitter and
//! the symbol index only.
//!
//! The jump rule (ARCHITECTURE, "Dumb jump rule"): jump directly only when exactly one
//! candidate is in the best rank tier, its kind fits the reference and nothing about the
//! lookup was a guess. Otherwise the caller shows a chooser sorted by rank. A wrong jump is
//! worse than a chooser.

use std::collections::HashSet;
use std::path::PathBuf;

use dumbj_syntax::{
    FileFacts, Language, Qualifier, Reference, ReferenceRole, Symbol, SymbolKind, SyntaxTree,
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

fn kind_fits(kind: SymbolKind, role: ReferenceRole, language: Language) -> bool {
    match role {
        ReferenceRole::Type => kind.is_type(),
        // In Kotlin `Foo()` constructs a Foo, so a call may name a type.
        ReferenceRole::Call => {
            kind.is_callable() || (language == Language::Kotlin && kind.is_type())
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
    fn members_named(&self, owner: SymbolId, name: &str, role: ReferenceRole) -> Vec<SymbolId> {
        let language = self.index.file(owner.file).and_then(|file| file.language);
        let fits = |symbol: &Symbol| {
            symbol.name == name
                && language.is_some_and(|language| kind_fits(symbol.kind, role, language))
        };
        for type_id in self.with_supertypes(owner) {
            let mut found: Vec<SymbolId> = self
                .index
                .members(type_id)
                .filter(|(_, symbol)| fits(symbol))
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
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
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

/// What stands before the dot of a qualified reference, as far as the index can tell.
enum Receiver {
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
    reference: Reference,
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

    /// What a single-name qualifier denotes: the type of a local or field called `name`,
    /// or the type `name` itself.
    fn receiver_of_name(&self, name: &str) -> Receiver {
        let Site {
            tree, buffer, file, ..
        } = self.site;
        let at = self.reference.range.start;
        if let Some(local) = tree.resolve_local(buffer, at, name, ReferenceRole::Value) {
            return Self::receiver_of_type(local.type_name.as_ref(), |type_name| {
                self.resolver.resolve_type(file, &self.enclosing, type_name)
            });
        }
        // A field or property of an enclosing type (or of one of its supertypes).
        let field = self
            .enclosing
            .iter()
            .flat_map(|owner| {
                self.resolver
                    .members_named(*owner, name, ReferenceRole::Value)
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

    /// What the qualifier of the reference denotes.
    fn receiver(&self, qualifier: &Qualifier) -> Receiver {
        let types = |types: Vec<SymbolId>| {
            if types.is_empty() {
                Receiver::Unknown
            } else {
                Receiver::Types(types)
            }
        };
        match qualifier {
            Qualifier::This => types(self.enclosing.clone()),
            Qualifier::Super => types(
                self.enclosing
                    .first()
                    .map_or_else(Vec::new, |own| self.resolver.supertypes(*own)),
            ),
            Qualifier::Name(name) => self.receiver_of_name(name),
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
                let mut receiver = parts
                    .next()
                    .map_or(Receiver::Unknown, |first| self.receiver_of_name(first));
                for part in parts {
                    let Receiver::Types(owners) = &receiver else {
                        break;
                    };
                    let member = owners
                        .iter()
                        .flat_map(|owner| {
                            self.resolver
                                .members_named(*owner, part, ReferenceRole::Member)
                        })
                        .next();
                    receiver = match member {
                        None => Receiver::Unknown,
                        Some(member) if self.resolver.is_type(member) => {
                            Receiver::Types(vec![member])
                        }
                        Some(member) => self.receiver_of_symbol(member),
                    };
                }
                receiver
            }
            Qualifier::Other => Receiver::Unknown,
        }
    }

    /// Every declaration called like the reference whose kind fits its role.
    fn named_candidates(&self) -> Vec<SymbolId> {
        let (name, role) = (&self.reference.name, self.reference.role);
        self.site
            .index
            .symbols_named(name)
            .iter()
            .copied()
            .filter(|id| {
                let language = self.site.index.file(id.file).and_then(|file| file.language);
                self.resolver.symbol(*id).is_some_and(|symbol| {
                    language.is_some_and(|language| kind_fits(symbol.kind, role, language))
                })
            })
            .collect()
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

    /// Turns ranked candidates into a resolution. `confident` is false when the lookup
    /// involved a guess, which always means a chooser.
    fn finish(self, mut ranked: Vec<(SymbolId, Tier)>, confident: bool) -> Resolution {
        ranked.sort_by_key(|(id, tier)| (*tier, *id));
        ranked.dedup();
        let best = ranked.first().map(|(_, tier)| *tier);
        let in_best_tier = ranked
            .iter()
            .filter(|(_, tier)| Some(*tier) == best)
            .count();
        let targets: Vec<Target> = ranked
            .iter()
            .filter_map(|(id, tier)| self.target(*id, *tier))
            .collect();
        let decision = match (targets.len(), in_best_tier) {
            (0, _) => Decision::NotFound,
            (_, 1) if confident => Decision::Jump,
            _ => Decision::Choose,
        };
        Resolution {
            reference: self.reference,
            targets,
            decision,
        }
    }

    fn resolve_import(self) -> Resolution {
        let prefix = match &self.reference.qualifier {
            Some(Qualifier::Name(prefix) | Qualifier::Path(prefix)) => format!("{prefix}."),
            _ => String::new(),
        };
        let fqn = format!("{prefix}{}", self.reference.name);
        let found = self.site.index.symbols_by_fqn(&fqn);
        let ranked = found
            .into_iter()
            .map(|id| (id, Tier::ExplicitImport))
            .collect();
        self.finish(ranked, true)
    }

    fn resolve_unqualified(self) -> Resolution {
        let Site {
            tree, buffer, file, ..
        } = self.site;
        let (name, role) = (self.reference.name.clone(), self.reference.role);
        if let Some(local) = tree.resolve_local(buffer, self.reference.range.start, &name, role) {
            let path = self
                .site
                .index
                .file(file)
                .map_or_else(PathBuf::new, |file| file.path.clone());
            let target = Target {
                path,
                range: local.name_range,
                symbol: None,
                tier: Tier::Local,
            };
            return Resolution {
                reference: self.reference,
                targets: vec![target],
                decision: Decision::Jump,
            };
        }
        // An import alias (`import a.B as C`) names exactly one declaration.
        let aliased = self.resolver.facts(file).and_then(|facts| {
            facts
                .imports
                .iter()
                .find(|import| import.alias.as_deref() == Some(name.as_str()))
        });
        if let Some(import) = aliased {
            let found = self.site.index.symbols_by_fqn(&import.path);
            let ranked = found
                .into_iter()
                .map(|id| (id, Tier::ExplicitImport))
                .collect();
            return self.finish(ranked, true);
        }
        // Members of the enclosing types and of their supertypes.
        let in_scope: Vec<SymbolId> = self
            .enclosing
            .iter()
            .flat_map(|owner| self.resolver.members_named(*owner, &name, role))
            .collect();
        if let Some(first) = in_scope.first() {
            // The innermost type that declares the name wins; overloads there tie.
            let owner = self.site.index.parent(*first);
            let ranked = in_scope
                .into_iter()
                .filter(|id| self.site.index.parent(*id) == owner)
                .map(|id| (id, Tier::Scope))
                .collect();
            return self.finish(ranked, true);
        }
        // Types, top-level functions and properties, and statically imported members
        // are visible unqualified. Other members can only be reached through an implicit
        // receiver (inherited from a library type, or a Kotlin scope function), which the
        // dumb engine cannot see: they are offered as guesses.
        let candidates = self.named_candidates();
        let mut visible = Vec::new();
        let mut guesses = Vec::new();
        for id in candidates {
            let tier = self.resolver.tier(file, &self.enclosing, id, false);
            let is_type = self.resolver.is_type(id);
            if is_type || !self.is_member(id) || tier == Tier::ExplicitImport {
                visible.push((id, tier));
            } else {
                guesses.push((id, self.resolver.tier(file, &self.enclosing, id, true)));
            }
        }
        if visible.is_empty() {
            return self.finish(guesses, false);
        }
        // An extension function's receiver is not checked, so it is never a sure thing.
        let confident = !visible.iter().any(|(id, _)| self.is_extension(*id));
        self.finish(visible, confident)
    }

    fn resolve_qualified(self, qualifier: &Qualifier) -> Resolution {
        let (name, role) = (self.reference.name.clone(), self.reference.role);
        let file = self.site.file;
        if role == ReferenceRole::Type
            && let Qualifier::Name(prefix) | Qualifier::Path(prefix) = qualifier
        {
            let types =
                self.resolver
                    .resolve_type(file, &self.enclosing, &format!("{prefix}.{name}"));
            let ranked = types
                .into_iter()
                .map(|id| (id, self.resolver.tier(file, &self.enclosing, id, true)))
                .collect();
            return self.finish(ranked, true);
        }
        // Kotlin extensions declared on a type with one of these simple names.
        let extensions_on = |receiver_names: &HashSet<&str>| -> Vec<(SymbolId, Tier)> {
            self.named_candidates()
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
        match self.receiver(qualifier) {
            Receiver::Types(owners) => {
                let members: Vec<SymbolId> = owners
                    .iter()
                    .flat_map(|owner| self.resolver.members_named(*owner, &name, role))
                    .collect();
                if !members.is_empty() {
                    let ranked = members.into_iter().map(|id| (id, Tier::Scope)).collect();
                    return self.finish(ranked, true);
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
                    return self.finish(extensions, false);
                }
                // Otherwise it may be inherited from a library supertype: fall through.
            }
            // The receiver's type is written but is not in the project (a library type).
            // Its members are not ours; only an extension on that type can match.
            Receiver::External(type_name) => {
                let simple = type_name.rsplit('.').next().unwrap_or(&type_name);
                let extensions = extensions_on(&HashSet::from([simple]));
                return self.finish(extensions, false);
            }
            Receiver::Unknown => {}
        }
        // Unknown receiver: every member or extension with this name is a guess.
        let ranked = self
            .named_candidates()
            .into_iter()
            .filter(|id| self.is_member(*id) || self.is_extension(*id))
            .map(|id| (id, self.resolver.tier(file, &self.enclosing, id, true)))
            .collect();
        self.finish(ranked, false)
    }
}

/// Resolves the identifier at `offset`. Returns `None` when there is no identifier there.
#[must_use]
pub fn resolve(site: Site<'_>, offset: ByteOffset) -> Option<Resolution> {
    let reference = site.tree.reference_at(site.buffer, offset)?;
    let resolver = Resolver { index: site.index };
    let enclosing = resolver.enclosing_types(site.file, reference.range.start);
    let lookup = Lookup {
        site,
        resolver,
        enclosing,
        reference,
    };
    Some(
        match (lookup.reference.role, lookup.reference.qualifier.clone()) {
            (ReferenceRole::Declaration, _) => Resolution {
                reference: lookup.reference,
                targets: Vec::new(),
                decision: Decision::NotFound,
            },
            _ if lookup.reference.in_import => lookup.resolve_import(),
            (_, Some(qualifier)) => lookup.resolve_qualified(&qualifier),
            (_, None) => lookup.resolve_unqualified(),
        },
    )
}
