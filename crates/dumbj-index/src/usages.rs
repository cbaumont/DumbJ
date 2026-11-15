//! Dumb Find Usages: an identifier search through the indexed files, narrowed with
//! tree-sitter (comments and strings never match) and with the same resolver Go to
//! Declaration uses.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dumbj_syntax::{Language, ReferenceRole, SymbolKind, SyntaxTree};
use dumbj_text::{Buffer, ByteOffset, ByteRange};

use crate::index::{FileId, Index, SymbolId};
use crate::resolve::{Decision, Site, resolve};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageTarget {
    Symbol(SymbolId),
    /// A parameter or local variable; its usages are all in one file.
    Local {
        file: FileId,
        name: String,
        name_range: ByteRange,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Certainty {
    Sure,
    /// The target is one of several candidates (unknown receiver, extension, overload).
    Maybe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub path: PathBuf,
    pub range: ByteRange,
    /// The zero-based line of the identifier.
    pub line: usize,
    pub line_text: String,
    pub certainty: Certainty,
}

/// Works out what the identifier at `offset` denotes, for Find Usages: the declaration
/// under the caret, or what the reference there resolves to. When resolution is ambiguous
/// the best-ranked candidate is used.
#[must_use]
pub fn usage_target_at(site: Site<'_>, offset: ByteOffset) -> Option<UsageTarget> {
    let resolution = resolve(site, offset)?;
    if resolution.reference.role == ReferenceRole::Declaration {
        let range = &resolution.reference.range;
        let facts = &site.index.file(site.file)?.facts;
        let position = facts
            .symbols
            .iter()
            .position(|symbol| symbol.name_range == *range);
        return Some(match position {
            Some(index) => UsageTarget::Symbol(SymbolId {
                file: site.file,
                index: u32::try_from(index).ok()?,
            }),
            // A declared name that is not in the index is a local or a parameter.
            None => UsageTarget::Local {
                file: site.file,
                name: resolution.reference.name,
                name_range: range.clone(),
            },
        });
    }
    let target = resolution.targets.first()?;
    Some(match target.symbol {
        Some(symbol) => UsageTarget::Symbol(symbol),
        None => UsageTarget::Local {
            file: site.file,
            name: resolution.reference.name,
            name_range: target.range.clone(),
        },
    })
}

struct ParsedFile {
    buffer: Buffer,
    tree: SyntaxTree,
}

/// Reads and parses `file` if it is a Java or Kotlin file that mentions one of `names`.
fn parse_if_mentions(
    index: &Index,
    file: FileId,
    names: &[String],
    cancelled: &AtomicBool,
) -> Option<ParsedFile> {
    let indexed = index.file(file)?;
    let language = indexed
        .language
        .filter(|l| matches!(l, Language::Java | Language::Kotlin))?;
    let text = String::from_utf8_lossy(&std::fs::read(&indexed.path).ok()?).into_owned();
    if !names.iter().any(|name| text.contains(name.as_str())) {
        return None;
    }
    let buffer = Buffer::new(&text);
    let tree =
        dumbj_syntax::parse(language, &buffer, || cancelled.load(Ordering::Relaxed)).ok()??;
    Some(ParsedFile { buffer, tree })
}

fn usage(
    index: &Index,
    file: FileId,
    parsed: &ParsedFile,
    range: ByteRange,
    certainty: Certainty,
) -> Option<Usage> {
    let line = parsed.buffer.point_at(range.start).line;
    Some(Usage {
        path: index.file(file)?.path.clone(),
        range,
        line: line.get(),
        line_text: parsed.buffer.line(line).trim().to_owned(),
        certainty,
    })
}

fn usages_in_file(
    index: &Index,
    file: FileId,
    target: &UsageTarget,
    name: &str,
    cancelled: &AtomicBool,
) -> Vec<Usage> {
    // The target's own name, plus any alias this file imports it under (Kotlin `as`).
    let mut names = vec![name.to_owned()];
    if let (UsageTarget::Symbol(id), Some(indexed)) = (target, index.file(file)) {
        let fqn = index.fqn(*id);
        let aliases = indexed
            .facts
            .imports
            .iter()
            .filter(|import| import.path == fqn);
        names.extend(aliases.filter_map(|import| import.alias.clone()));
    }
    let Some(parsed) = parse_if_mentions(index, file, &names, cancelled) else {
        return Vec::new();
    };
    let site = Site {
        index,
        file,
        tree: &parsed.tree,
        buffer: &parsed.buffer,
    };
    let mut found = Vec::new();
    let references = names
        .iter()
        .flat_map(|name| parsed.tree.references_named(&parsed.buffer, name));
    for reference in references {
        if reference.role == ReferenceRole::Declaration {
            continue;
        }
        let Some(resolution) = resolve(site, reference.range.start) else {
            continue;
        };
        let hits = |candidate: &crate::resolve::Target| match target {
            UsageTarget::Symbol(id) => candidate.symbol.is_some_and(|found| {
                found == *id
                    || (index.parent(found) == Some(*id)
                        && index
                            .symbol(found)
                            .is_some_and(|symbol| symbol.kind == SymbolKind::Constructor))
            }),
            UsageTarget::Local { name_range, .. } => {
                candidate.symbol.is_none() && candidate.range == *name_range
            }
        };
        let certainty = match resolution.decision {
            Decision::Jump if resolution.targets.first().is_some_and(hits) => Certainty::Sure,
            Decision::Choose if resolution.targets.iter().any(hits) => Certainty::Maybe,
            _ => continue,
        };
        found.extend(usage(index, file, &parsed, reference.range, certainty));
    }
    found
}

/// Files are searched in parallel. `cancelled` is polled between files; when it is set the
/// search stops and returns `None`.
pub fn find_usages(
    index: &Index,
    target: &UsageTarget,
    cancelled: &AtomicBool,
) -> Option<Vec<Usage>> {
    let (name, files): (String, Vec<FileId>) = match target {
        UsageTarget::Symbol(id) => (
            index.symbol(*id)?.name.clone(),
            index.files().map(|(file, _)| file).collect(),
        ),
        UsageTarget::Local { file, name, .. } => (name.clone(), vec![*file]),
    };
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(files.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !cancelled.load(Ordering::Relaxed) {
                    let Some(file) = files.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let usages = usages_in_file(index, *file, target, &name, cancelled);
                    if let (false, Ok(mut found)) = (usages.is_empty(), found.lock()) {
                        found.extend(usages);
                    }
                }
            });
        }
    });
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let mut usages = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    usages.sort_by(|a, b| (&a.path, a.range.start).cmp(&(&b.path, b.range.start)));
    Some(usages)
}
