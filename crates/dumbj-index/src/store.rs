//! An on-disk SQLite cache of per-file index facts, so reopening a project only re-parses
//! the files that changed. The database is a cache: it is rebuilt when it is corrupt or
//! was written by another schema version, and it is never the source of truth.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use dumbj_syntax::{FileFacts, Import, Language, Parameter, Platform, Symbol, SymbolKind};
use dumbj_text::ByteOffset;
use rusqlite::{Connection, params};

use crate::index::IndexedFile;

const SCHEMA_VERSION: i64 = 10;

const SCHEMA: &str = "
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    mtime_ns INTEGER NOT NULL,
    size INTEGER NOT NULL,
    language TEXT,
    package TEXT,
    jvm_name TEXT
);
CREATE TABLE symbols (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    range_start INTEGER NOT NULL,
    range_end INTEGER NOT NULL,
    name_start INTEGER NOT NULL,
    name_end INTEGER NOT NULL,
    parent INTEGER,
    receiver TEXT,
    type_name TEXT,
    supertypes TEXT NOT NULL,
    parameters TEXT,
    platform TEXT,
    jvm_name TEXT,
    type_argument TEXT,
    PRIMARY KEY (file_id, idx)
);
CREATE INDEX symbols_name ON symbols(name);
CREATE TABLE imports (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    path TEXT NOT NULL,
    wildcard INTEGER NOT NULL,
    is_static INTEGER NOT NULL,
    alias TEXT,
    PRIMARY KEY (file_id, idx)
);
";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("index cache database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("index cache file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("a freshly created index cache has an unexpected schema")]
    Schema,
}

/// The modification time and size of a file; when both match the stored values the file is
/// assumed unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheStamp {
    pub mtime_ns: i64,
    pub size: u64,
}

impl CacheStamp {
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let nanos = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            mtime_ns: i64::try_from(nanos).ok()?,
            size: meta.len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub language: Option<Language>,
    pub facts: FileFacts,
}

#[derive(Debug)]
pub struct IndexStore {
    connection: Connection,
}

impl IndexStore {
    /// Opens the cache at `path`, creating it and its parent directories when missing. A
    /// file that is not a healthy database of the current schema version is deleted and
    /// replaced by an empty one.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match Self::try_open(path) {
            Ok(Some(store)) => return Ok(store),
            Ok(None) => {
                tracing::info!(path = %path.display(), "index cache has another schema; rebuilding");
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "index cache is unusable; rebuilding");
            }
        }
        remove_database(path)?;
        match Self::try_open(path)? {
            Some(store) => Ok(store),
            None => Err(StoreError::Schema),
        }
    }

    /// `Ok(None)` when the database has a different schema version.
    fn try_open(path: &Path) -> Result<Option<Self>, StoreError> {
        let connection = Connection::open(path)?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version == 0 {
            // A new database, or one this code never finished creating.
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )?;
            if tables != 0 {
                return Ok(None);
            }
            connection.execute_batch(SCHEMA)?;
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version != SCHEMA_VERSION {
            return Ok(None);
        }
        let check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(check),
            )));
        }
        // The cache can always be rebuilt, so durability is traded for speed.
        connection.pragma_update(None, "synchronous", "OFF")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(Some(Self { connection }))
    }

    /// Every cached file by relative path. Files whose rows cannot be understood (an
    /// unknown kind or language) are left out, so they are parsed again.
    pub fn load(&self) -> Result<HashMap<String, (CacheStamp, StoredFile)>, StoreError> {
        let mut files: HashMap<i64, (String, CacheStamp, Option<StoredFile>)> = HashMap::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT id, path, mtime_ns, size, language, package, jvm_name FROM files",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let path: String = row.get(1)?;
                let size: i64 = row.get(3)?;
                let stamp = CacheStamp {
                    mtime_ns: row.get(2)?,
                    size: u64::try_from(size).unwrap_or(0),
                };
                let language: Option<String> = row.get(4)?;
                let package: Option<String> = row.get(5)?;
                let jvm_name: Option<String> = row.get(6)?;
                let language = match language {
                    None => Some(None),
                    Some(id) => Language::from_id(&id).map(Some),
                };
                let stored = language.map(|language| StoredFile {
                    language,
                    facts: FileFacts {
                        package,
                        jvm_name,
                        ..FileFacts::default()
                    },
                });
                files.insert(id, (path, stamp, stored));
            }
        }
        {
            let mut statement = self.connection.prepare(
                "SELECT file_id, name, kind, range_start, range_end, name_start, name_end,
                        parent, receiver, type_name, supertypes, parameters, platform,
                        jvm_name, type_argument
                 FROM symbols ORDER BY file_id, idx",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let file_id: i64 = row.get(0)?;
                let symbol = symbol_from_row(row)?;
                let Some(entry) = files.get_mut(&file_id) else {
                    continue;
                };
                match (symbol, entry.2.as_mut()) {
                    (Some(symbol), Some(stored)) => stored.facts.symbols.push(symbol),
                    _ => entry.2 = None,
                }
            }
        }
        {
            let mut statement = self.connection.prepare(
                "SELECT file_id, path, wildcard, is_static, alias FROM imports
                 ORDER BY file_id, idx",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let file_id: i64 = row.get(0)?;
                if let Some((_, _, Some(stored))) = files.get_mut(&file_id) {
                    stored.facts.imports.push(Import {
                        path: row.get(1)?,
                        wildcard: row.get(2)?,
                        is_static: row.get(3)?,
                        alias: row.get(4)?,
                    });
                }
            }
        }
        Ok(files
            .into_values()
            .filter_map(|(path, stamp, stored)| Some((path, (stamp, stored?))))
            .collect())
    }

    pub fn save(
        &mut self,
        changed: &[(&IndexedFile, CacheStamp)],
        removed: &[String],
    ) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        {
            let mut delete = transaction.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            for path in removed {
                delete.execute([path])?;
            }
            let mut insert_file = transaction.prepare_cached(
                "INSERT INTO files (path, mtime_ns, size, language, package, jvm_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            let mut insert_symbol = transaction.prepare_cached(
                "INSERT INTO symbols (file_id, idx, name, kind, range_start, range_end,
                                      name_start, name_end, parent, receiver, type_name,
                                      supertypes, parameters, platform, jvm_name,
                                      type_argument)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16)",
            )?;
            let mut insert_import = transaction.prepare_cached(
                "INSERT INTO imports (file_id, idx, path, wildcard, is_static, alias)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (file, stamp) in changed {
                delete.execute([&file.relative])?;
                insert_file.execute(params![
                    file.relative,
                    stamp.mtime_ns,
                    i64::try_from(stamp.size).unwrap_or(i64::MAX),
                    file.language.map(Language::as_str),
                    file.facts.package,
                    file.facts.jvm_name,
                ])?;
                let file_id = transaction.last_insert_rowid();
                for (idx, symbol) in file.facts.symbols.iter().enumerate() {
                    insert_symbol.execute(params![
                        file_id,
                        to_i64(idx),
                        symbol.name,
                        symbol.kind.as_str(),
                        to_i64(symbol.range.start.0),
                        to_i64(symbol.range.end.0),
                        to_i64(symbol.name_range.start.0),
                        to_i64(symbol.name_range.end.0),
                        symbol.parent.map(to_i64),
                        symbol.receiver,
                        symbol.type_name,
                        symbol.supertypes.join("\n"),
                        symbol.parameters.as_deref().map(encode_parameters),
                        symbol.platform.map(|platform| match platform {
                            Platform::Expect => "expect",
                            Platform::Actual => "actual",
                        }),
                        symbol.jvm_name,
                        symbol.type_argument,
                    ])?;
                }
                for (idx, import) in file.facts.imports.iter().enumerate() {
                    insert_import.execute(params![
                        file_id,
                        to_i64(idx),
                        import.path,
                        import.wildcard,
                        import.is_static,
                        import.alias,
                    ])?;
                }
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

/// `None` for a kind this version does not know, which makes the file parse again.
fn symbol_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Symbol>> {
    let kind: String = row.get(2)?;
    let Some(kind) = SymbolKind::from_name(&kind) else {
        return Ok(None);
    };
    let supertypes: String = row.get(10)?;
    let parameters: Option<String> = row.get(11)?;
    let platform: Option<String> = row.get(12)?;
    Ok(Some(Symbol {
        name: row.get(1)?,
        kind,
        range: offset(row.get(3)?)..offset(row.get(4)?),
        name_range: offset(row.get(5)?)..offset(row.get(6)?),
        parent: row
            .get::<_, Option<i64>>(7)?
            .and_then(|parent| usize::try_from(parent).ok()),
        supertypes: supertypes
            .split('\n')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect(),
        receiver: row.get(8)?,
        type_name: row.get(9)?,
        parameters: parameters.as_deref().map(decode_parameters),
        platform: match platform.as_deref() {
            Some("expect") => Some(Platform::Expect),
            Some("actual") => Some(Platform::Actual),
            _ => None,
        },
        jvm_name: row.get(13)?,
        type_argument: row.get(14)?,
    }))
}

/// One parameter per line as `name<TAB>type<TAB>flags`: `o` for optional, `v` for
/// variadic. Names and types are identifiers, so they contain neither separator.
fn encode_parameters(parameters: &[Parameter]) -> String {
    let lines: Vec<String> = parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}\t{}\t{}{}",
                parameter.name,
                parameter.type_name.as_deref().unwrap_or(""),
                if parameter.optional { "o" } else { "" },
                if parameter.variadic { "v" } else { "" },
            )
        })
        .collect();
    lines.join("\n")
}

fn decode_parameters(text: &str) -> Vec<Parameter> {
    text.lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next().unwrap_or("").to_owned();
            let type_name = fields.next().filter(|name| !name.is_empty());
            let flags = fields.next().unwrap_or("");
            Parameter {
                name,
                type_name: type_name.map(str::to_owned),
                optional: flags.contains('o'),
                variadic: flags.contains('v'),
            }
        })
        .collect()
}

fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn offset(value: i64) -> ByteOffset {
    ByteOffset(usize::try_from(value).unwrap_or(0))
}

fn remove_database(path: &Path) -> Result<(), StoreError> {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        match fs::remove_file(&name) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
