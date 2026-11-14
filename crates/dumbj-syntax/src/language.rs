//! The languages DumbJ has a grammar for.

use std::path::Path;
use std::sync::OnceLock;

use tree_sitter::Query;

use crate::SyntaxError;

/// A language with a tree-sitter grammar and highlight query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Java,
    Kotlin,
    Yaml,
}

impl Language {
    pub const ALL: &[Self] = &[Self::Java, Self::Kotlin, Self::Yaml];

    /// Picks the language from a file name's extension.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "java" => Some(Self::Java),
            "kt" | "kts" => Some(Self::Kotlin),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }

    /// The name shown to the user.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Java => "Java",
            Self::Kotlin => "Kotlin",
            Self::Yaml => "YAML",
        }
    }

    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Kotlin => tree_sitter_kotlin_sg::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
        }
    }

    fn highlights_source(self) -> &'static str {
        match self {
            Self::Java => include_str!("../queries/java/highlights.scm"),
            Self::Kotlin => include_str!("../queries/kotlin/highlights.scm"),
            Self::Yaml => include_str!("../queries/yaml/highlights.scm"),
        }
    }

    /// The compiled highlight query, built once per language.
    pub(crate) fn highlights_query(self) -> Result<&'static Query, SyntaxError> {
        static QUERIES: [OnceLock<Result<Query, String>>; 3] =
            [OnceLock::new(), OnceLock::new(), OnceLock::new()];
        let slot = match self {
            Self::Java => &QUERIES[0],
            Self::Kotlin => &QUERIES[1],
            Self::Yaml => &QUERIES[2],
        };
        slot.get_or_init(|| {
            Query::new(&self.grammar(), self.highlights_source()).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| SyntaxError::Query {
            language: self,
            message: message.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_language_from_extension() {
        let detect = |name: &str| Language::from_path(Path::new(name));
        assert_eq!(detect("src/Main.java"), Some(Language::Java));
        assert_eq!(detect("App.kt"), Some(Language::Kotlin));
        assert_eq!(detect("build.gradle.kts"), Some(Language::Kotlin));
        assert_eq!(detect("ci.YML"), Some(Language::Yaml));
        assert_eq!(detect("config.yaml"), Some(Language::Yaml));
        assert_eq!(detect("README.md"), None);
        assert_eq!(detect("Makefile"), None);
    }

    #[test]
    fn every_highlight_query_compiles() {
        for &language in Language::ALL {
            language.highlights_query().unwrap();
        }
    }
}
