use crate::fuzzy::fuzzy_match;
use crate::index::{FileId, Index, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchScope {
    All,
    Classes,
    Files,
    Symbols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitTarget {
    File(FileId),
    Symbol(SymbolId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub target: HitTarget,
    pub score: i32,
    /// Byte offsets of the matched characters in the hit's name, for highlighting.
    pub positions: Vec<usize>,
}

/// In the combined list classes come before files, and files before other symbols, when
/// they match equally well.
const CLASS_BOOST: i32 = 10;
const FILE_BOOST: i32 = 5;
const CANCEL_INTERVAL: usize = 1024;

fn file_name(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

/// The best `limit` matches of `query` in `scope`, best first. An empty query matches
/// nothing. `cancelled` is polled regularly; when it returns `true` the search stops and
/// returns `None`.
pub fn search(
    index: &Index,
    query: &str,
    scope: SearchScope,
    limit: usize,
    cancelled: impl Fn() -> bool,
) -> Option<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut hits: Vec<(SearchHit, &str)> = Vec::new();
    let mut seen = 0_usize;
    let mut poll = || {
        seen += 1;
        seen.is_multiple_of(CANCEL_INTERVAL) && cancelled()
    };
    if matches!(scope, SearchScope::All | SearchScope::Files) {
        // A query with a slash is matched against the whole relative path.
        let by_path = query.contains('/');
        for (id, file) in index.files() {
            if poll() {
                return None;
            }
            let name = if by_path {
                file.relative.as_str()
            } else {
                file_name(&file.relative)
            };
            if let Some(found) = fuzzy_match(query, name) {
                let hit = SearchHit {
                    target: HitTarget::File(id),
                    score: found.score + FILE_BOOST,
                    positions: found.positions,
                };
                hits.push((hit, name));
            }
        }
    }
    if scope != SearchScope::Files {
        for (id, symbol) in index.symbols() {
            if poll() {
                return None;
            }
            let is_type = symbol.kind.is_type();
            if scope == SearchScope::Classes && !is_type {
                continue;
            }
            if let Some(found) = fuzzy_match(query, &symbol.name) {
                let boost = if is_type { CLASS_BOOST } else { 0 };
                let hit = SearchHit {
                    target: HitTarget::Symbol(id),
                    score: found.score + boost,
                    positions: found.positions,
                };
                hits.push((hit, symbol.name.as_str()));
            }
        }
    }
    hits.sort_by(|(a, a_name), (b, b_name)| {
        b.score
            .cmp(&a.score)
            .then_with(|| a_name.cmp(b_name))
            .then_with(|| {
                let key = |target: &HitTarget| match target {
                    HitTarget::File(id) => (*id, 0),
                    HitTarget::Symbol(id) => (id.file, id.index + 1),
                };
                key(&a.target).cmp(&key(&b.target))
            })
    });
    hits.truncate(limit);
    Some(hits.into_iter().map(|(hit, _)| hit).collect())
}

/// The declarations of one file for the File Structure popup: in source order when `query`
/// is empty, otherwise the matching ones, best first.
#[must_use]
pub fn file_structure(index: &Index, file: FileId, query: &str) -> Vec<SearchHit> {
    let Some(indexed) = index.file(file) else {
        return Vec::new();
    };
    let filtering = !query.trim().is_empty();
    let mut hits: Vec<SearchHit> = indexed
        .facts
        .symbols
        .iter()
        .enumerate()
        .filter_map(|(position, symbol)| {
            let found = fuzzy_match(query, &symbol.name)?;
            let id = SymbolId {
                file,
                index: u32::try_from(position).ok()?,
            };
            Some(SearchHit {
                target: HitTarget::Symbol(id),
                score: found.score,
                positions: found.positions,
            })
        })
        .collect();
    if filtering {
        // A stable sort keeps source order among equally good matches.
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    }
    hits
}
