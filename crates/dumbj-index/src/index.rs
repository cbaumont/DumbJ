use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dumbj_syntax::{FileFacts, Language, Symbol};

use crate::project::{ModuleId, ProjectModel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId {
    pub file: FileId,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub path: PathBuf,
    pub relative: String,
    /// `None` for files DumbJ has no grammar for; they are still listed for Go to File.
    pub language: Option<Language>,
    pub facts: FileFacts,
}

#[derive(Debug, Default)]
pub struct Index {
    files: Vec<IndexedFile>,
    by_path: HashMap<PathBuf, FileId>,
    by_name: HashMap<String, Vec<SymbolId>>,
    by_package: HashMap<String, Vec<FileId>>,
    project: ProjectModel,
    modules: Vec<Option<ModuleId>>,
    /// Each type's direct subtypes, worked out on first use (see `hierarchy`).
    pub(crate) subtypes: OnceLock<HashMap<SymbolId, Vec<SymbolId>>>,
    /// Kotlin files by their facade class name (`ShippingKt`, or `@file:JvmName`).
    pub(crate) facades: OnceLock<HashMap<String, Vec<FileId>>>,
}

fn to_u32(value: usize) -> u32 {
    // An index holds far fewer than 2^32 files, and a file far fewer symbols.
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl Index {
    /// Builds the lookup tables for `files`. Files are sorted by path so that IDs and
    /// result orders do not depend on the order indexing threads finished in.
    #[must_use]
    pub fn new(mut files: Vec<IndexedFile>) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path);
        let project = ProjectModel::discover(&files);
        let modules = files
            .iter()
            .map(|file| project.module_of_path(&file.relative))
            .collect();
        let mut index = Self {
            files,
            project,
            modules,
            ..Self::default()
        };
        for (position, file) in index.files.iter().enumerate() {
            let id = FileId(to_u32(position));
            index.by_path.insert(file.path.clone(), id);
            if let Some(package) = &file.facts.package {
                index
                    .by_package
                    .entry(package.clone())
                    .or_default()
                    .push(id);
            }
            for (symbol_index, symbol) in file.facts.symbols.iter().enumerate() {
                let symbol_id = SymbolId {
                    file: id,
                    index: to_u32(symbol_index),
                };
                index
                    .by_name
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(symbol_id);
            }
        }
        index
    }

    #[must_use]
    pub fn with_file(&self, file: IndexedFile) -> Self {
        let mut files: Vec<IndexedFile> = self
            .files
            .iter()
            .filter(|existing| existing.path != file.path)
            .cloned()
            .collect();
        files.push(file);
        Self::new(files)
    }

    #[must_use]
    pub fn without_file(&self, path: &Path) -> Self {
        Self::new(
            self.files
                .iter()
                .filter(|file| file.path != path)
                .cloned()
                .collect(),
        )
    }

    #[must_use]
    pub fn project(&self) -> &ProjectModel {
        &self.project
    }

    #[must_use]
    pub fn module_of(&self, file: FileId) -> Option<ModuleId> {
        self.modules.get(file.0 as usize).copied().flatten()
    }

    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.files.iter().map(|file| file.facts.symbols.len()).sum()
    }

    pub fn files(&self) -> impl Iterator<Item = (FileId, &IndexedFile)> {
        self.files
            .iter()
            .enumerate()
            .map(|(position, file)| (FileId(to_u32(position)), file))
    }

    #[must_use]
    pub fn file(&self, id: FileId) -> Option<&IndexedFile> {
        self.files.get(id.0 as usize)
    }

    #[must_use]
    pub fn file_by_path(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    #[must_use]
    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.file(id.file)?.facts.symbols.get(id.index as usize)
    }

    /// Every declaration called `name`, in file then source order.
    #[must_use]
    pub fn symbols_named(&self, name: &str) -> &[SymbolId] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    pub fn symbols(&self) -> impl Iterator<Item = (SymbolId, &Symbol)> {
        self.files().flat_map(|(file, indexed)| {
            indexed
                .facts
                .symbols
                .iter()
                .enumerate()
                .map(move |(index, symbol)| {
                    (
                        SymbolId {
                            file,
                            index: to_u32(index),
                        },
                        symbol,
                    )
                })
        })
    }

    #[must_use]
    pub fn files_in_package(&self, package: &str) -> &[FileId] {
        self.by_package.get(package).map_or(&[], Vec::as_slice)
    }

    /// The package of the file that declares `id`; empty for the default package.
    #[must_use]
    pub fn package_of(&self, id: SymbolId) -> &str {
        self.file(id.file)
            .and_then(|file| file.facts.package.as_deref())
            .unwrap_or("")
    }

    #[must_use]
    pub fn parent(&self, id: SymbolId) -> Option<SymbolId> {
        let parent = self.symbol(id)?.parent?;
        Some(SymbolId {
            file: id.file,
            index: to_u32(parent),
        })
    }

    pub fn members(&self, id: SymbolId) -> impl Iterator<Item = (SymbolId, &Symbol)> {
        let symbols = self
            .file(id.file)
            .map_or(&[][..], |file| file.facts.symbols.as_slice());
        symbols
            .iter()
            .enumerate()
            .filter(move |(_, symbol)| symbol.parent == Some(id.index as usize))
            .map(move |(index, symbol)| {
                (
                    SymbolId {
                        file: id.file,
                        index: to_u32(index),
                    },
                    symbol,
                )
            })
    }

    #[must_use]
    pub fn qualified_name(&self, id: SymbolId) -> String {
        self.file(id.file).map_or_else(String::new, |file| {
            file.facts.qualified_name(id.index as usize)
        })
    }

    #[must_use]
    pub fn fqn(&self, id: SymbolId) -> String {
        let (package, name) = (self.package_of(id), self.qualified_name(id));
        if package.is_empty() {
            name
        } else {
            format!("{package}.{name}")
        }
    }

    #[must_use]
    pub fn symbols_by_fqn(&self, fqn: &str) -> Vec<SymbolId> {
        let name = fqn.rsplit('.').next().unwrap_or(fqn);
        self.symbols_named(name)
            .iter()
            .copied()
            .filter(|id| self.fqn(*id) == fqn)
            .collect()
    }
}
