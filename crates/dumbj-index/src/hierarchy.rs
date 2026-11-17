use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;

use dumbj_syntax::{Symbol, SymbolKind};
use dumbj_text::ByteRange;

use crate::index::{FileId, Index, SymbolId};
use crate::resolve::resolve_supertypes;
use crate::usages::{Usage, UsageTarget, find_usages};

const MAX_DEPTH: usize = 32;

fn subtype_map(index: &Index) -> &HashMap<SymbolId, Vec<SymbolId>> {
    index.subtypes.get_or_init(|| {
        let mut map: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        for (id, symbol) in index.symbols() {
            if !symbol.kind.is_type() || symbol.supertypes.is_empty() {
                continue;
            }
            for supertype in resolve_supertypes(index, id) {
                if supertype != id {
                    map.entry(supertype).or_default().push(id);
                }
            }
        }
        for subtypes in map.values_mut() {
            subtypes.sort_unstable();
            subtypes.dedup();
        }
        map
    })
}

pub fn warm_hierarchy(index: &Index) {
    subtype_map(index);
}

#[must_use]
pub fn direct_subtypes(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    subtype_map(index).get(&id).cloned().unwrap_or_default()
}

#[must_use]
pub fn direct_supertypes(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    resolve_supertypes(index, id)
}

/// The supertype that is not an interface, the parent in IntelliJ's class hierarchy.
#[must_use]
pub fn superclass(index: &Index, id: SymbolId) -> Option<SymbolId> {
    direct_supertypes(index, id).into_iter().find(|supertype| {
        index
            .symbol(*supertype)
            .is_some_and(|s| s.kind != SymbolKind::Interface)
    })
}

fn all_subtypes(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    let mut seen = HashSet::from([id]);
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(id, 0)]);
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH {
            continue;
        }
        for subtype in direct_subtypes(index, current) {
            if seen.insert(subtype) {
                found.push(subtype);
                queue.push_back((subtype, depth + 1));
            }
        }
    }
    found
}

fn is_member_like(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind,
        SymbolKind::Method | SymbolKind::Function | SymbolKind::Property
    )
}

/// A parameter of unknown type (a type parameter, a function type) matches anything.
fn same_signature(candidate: &Symbol, base: &Symbol) -> bool {
    if candidate.name != base.name || !is_member_like(candidate) || !is_member_like(base) {
        return false;
    }
    let is_property = |symbol: &Symbol| symbol.kind == SymbolKind::Property;
    if is_property(candidate) || is_property(base) {
        return is_property(candidate) && is_property(base);
    }
    match (&candidate.parameters, &base.parameters) {
        (Some(ours), Some(theirs)) => {
            ours.len() == theirs.len()
                && ours.iter().zip(theirs).all(|(a, b)| {
                    let simple = |name: &str| name.rsplit('.').next().unwrap_or(name).to_owned();
                    match (&a.type_name, &b.type_name) {
                        (Some(a), Some(b)) => simple(a) == simple(b),
                        _ => true,
                    }
                })
        }
        _ => true,
    }
}

fn matching_member(index: &Index, owner: SymbolId, member: &Symbol) -> Option<SymbolId> {
    index
        .members(owner)
        .find(|(_, candidate)| same_signature(candidate, member))
        .map(|(id, _)| id)
}

fn member_and_owner(index: &Index, id: SymbolId) -> Option<(&Symbol, SymbolId)> {
    let symbol = index.symbol(id)?;
    let owner = index.parent(id)?;
    let owner_is_type = index.symbol(owner).is_some_and(|s| s.kind.is_type());
    (is_member_like(symbol) && owner_is_type).then_some((symbol, owner))
}

#[must_use]
pub fn super_members(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    let Some((member, owner)) = member_and_owner(index, id) else {
        return Vec::new();
    };
    let mut seen = HashSet::from([owner]);
    let mut found = Vec::new();
    let mut queue: VecDeque<(SymbolId, usize)> = direct_supertypes(index, owner)
        .into_iter()
        .map(|supertype| (supertype, 0))
        .collect();
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH || !seen.insert(current) {
            continue;
        }
        match matching_member(index, current, member) {
            Some(base) => found.push(base),
            None => queue.extend(
                direct_supertypes(index, current)
                    .into_iter()
                    .map(|supertype| (supertype, depth + 1)),
            ),
        }
    }
    found
}

#[must_use]
pub fn overriders(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    let Some((member, owner)) = member_and_owner(index, id) else {
        return Vec::new();
    };
    all_subtypes(index, owner)
        .into_iter()
        .filter_map(|subtype| matching_member(index, subtype, member))
        .collect()
}

#[must_use]
pub fn implementations(index: &Index, id: SymbolId) -> Vec<SymbolId> {
    match index.symbol(id) {
        Some(symbol) if symbol.kind.is_type() => all_subtypes(index, id),
        Some(_) => overriders(index, id),
        None => Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GutterMark {
    pub symbol: SymbolId,
    pub overrides: bool,
    pub overridden: bool,
}

#[must_use]
pub fn gutter_marks(index: &Index, file: FileId) -> Vec<GutterMark> {
    let Some(indexed) = index.file(file) else {
        return Vec::new();
    };
    let count = u32::try_from(indexed.facts.symbols.len()).unwrap_or(u32::MAX);
    (0..count)
        .map(|position| SymbolId {
            file,
            index: position,
        })
        .filter_map(|id| {
            let symbol = index.symbol(id)?;
            let mark = if symbol.kind.is_type() {
                GutterMark {
                    symbol: id,
                    overrides: false,
                    overridden: !direct_subtypes(index, id).is_empty(),
                }
            } else if member_and_owner(index, id).is_some() {
                GutterMark {
                    symbol: id,
                    overrides: !super_members(index, id).is_empty(),
                    overridden: !overriders(index, id).is_empty(),
                }
            } else {
                return None;
            };
            (mark.overrides || mark.overridden).then_some(mark)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub symbol: Option<SymbolId>,
    pub file: FileId,
    pub calls: Vec<Usage>,
}

fn enclosing_declaration(index: &Index, file: FileId, range: &ByteRange) -> Option<SymbolId> {
    let facts = &index.file(file)?.facts;
    facts
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, symbol)| {
            symbol.range.start <= range.start
                && range.end <= symbol.range.end
                && symbol.kind != SymbolKind::EnumConstant
        })
        .min_by_key(|(_, symbol)| symbol.range.end.get() - symbol.range.start.get())
        .and_then(|(position, _)| {
            Some(SymbolId {
                file,
                index: u32::try_from(position).ok()?,
            })
        })
}

/// Returns `None` when cancelled.
#[must_use]
pub fn callers(index: &Index, id: SymbolId, cancelled: &AtomicBool) -> Option<Vec<Caller>> {
    let usages = find_usages(index, &UsageTarget::Symbol(id), cancelled)?;
    let mut grouped: Vec<Caller> = Vec::new();
    let mut slot: HashMap<(FileId, Option<SymbolId>), usize> = HashMap::new();
    for usage in usages {
        let Some(file) = index.file_by_path(&usage.path) else {
            continue;
        };
        let symbol = enclosing_declaration(index, file, &usage.range);
        if symbol == Some(id) {
            let is_name = index
                .symbol(id)
                .is_some_and(|declared| declared.name_range == usage.range);
            if is_name {
                continue;
            }
        }
        let position = *slot.entry((file, symbol)).or_insert_with(|| {
            grouped.push(Caller {
                symbol,
                file,
                calls: Vec::new(),
            });
            grouped.len() - 1
        });
        if let Some(caller) = grouped.get_mut(position) {
            caller.calls.push(usage);
        }
    }
    Some(grouped)
}
