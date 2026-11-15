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
    Xml,
    Properties,
    Json,
    Toml,
    Groovy,
    Markdown,
    Shell,
    Dockerfile,
    Sql,
}

impl Language {
    pub const ALL: &[Self] = &[
        Self::Java,
        Self::Kotlin,
        Self::Yaml,
        Self::Xml,
        Self::Properties,
        Self::Json,
        Self::Toml,
        Self::Groovy,
        Self::Markdown,
        Self::Shell,
        Self::Dockerfile,
        Self::Sql,
    ];

    /// Picks the language from a file's name, or else its extension. Only the path is
    /// looked at, never the content, so a script without extension is recognised by name.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if let Some(language) = Self::from_file_name(name) {
            return Some(language);
        }
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "java" => Some(Self::Java),
            "kt" | "kts" => Some(Self::Kotlin),
            "yaml" | "yml" => Some(Self::Yaml),
            // Maven's pom.xml and Android's layouts are XML; so are IDE project files.
            "xml" | "xsd" | "xsl" | "xslt" | "svg" | "wsdl" | "pom" | "fxml" => Some(Self::Xml),
            "dockerfile" => Some(Self::Dockerfile),
            "properties" => Some(Self::Properties),
            "json" | "jsonc" | "geojson" | "webmanifest" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "groovy" | "gradle" => Some(Self::Groovy),
            "md" | "markdown" | "mdx" => Some(Self::Markdown),
            "sh" | "bash" | "zsh" | "ksh" => Some(Self::Shell),
            "sql" => Some(Self::Sql),
            _ => None,
        }
    }

    /// Files recognised by their whole name, which has no useful extension.
    fn from_file_name(name: &str) -> Option<Self> {
        match name {
            // The Gradle and Maven wrapper scripts have a `#!/bin/sh` line, no extension.
            "gradlew" | "mvnw" => Some(Self::Shell),
            // `Dockerfile`, `Dockerfile.dev`, `Containerfile`; `app.dockerfile` is by extension.
            "Dockerfile" | "Containerfile" => Some(Self::Dockerfile),
            _ if name.starts_with("Dockerfile.") => Some(Self::Dockerfile),
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
            Self::Xml => "XML",
            Self::Properties => "Properties",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Groovy => "Groovy",
            Self::Markdown => "Markdown",
            Self::Shell => "Shell",
            Self::Dockerfile => "Dockerfile",
            Self::Sql => "SQL",
        }
    }

    /// A stable lowercase identifier for storing the language outside the process; see
    /// [`Self::from_id`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Java => "java",
            Self::Kotlin => "kotlin",
            Self::Yaml => "yaml",
            Self::Xml => "xml",
            Self::Properties => "properties",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Groovy => "groovy",
            Self::Markdown => "markdown",
            Self::Shell => "shell",
            Self::Dockerfile => "dockerfile",
            Self::Sql => "sql",
        }
    }

    /// The language stored under `id` by [`Self::as_str`], or `None` for an unknown id.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|language| language.as_str() == id)
    }

    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            #[cfg(feature = "kotlin-sg")]
            Self::Kotlin => tree_sitter_kotlin_sg::LANGUAGE.into(),
            #[cfg(not(feature = "kotlin-sg"))]
            Self::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
            Self::Properties => tree_sitter_properties::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Groovy => dekobon_tree_sitter_groovy::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::Shell => tree_sitter_bash::LANGUAGE.into(),
            Self::Dockerfile => tree_sitter_dockerfile_updated::language(),
            Self::Sql => tree_sitter_sequel::LANGUAGE.into(),
        }
    }

    fn highlights_source(self) -> &'static str {
        match self {
            Self::Java => include_str!("../queries/java/highlights.scm"),
            Self::Kotlin => include_str!("../queries/kotlin/highlights.scm"),
            Self::Yaml => include_str!("../queries/yaml/highlights.scm"),
            Self::Xml => include_str!("../queries/xml/highlights.scm"),
            Self::Properties => include_str!("../queries/properties/highlights.scm"),
            Self::Json => include_str!("../queries/json/highlights.scm"),
            Self::Toml => include_str!("../queries/toml/highlights.scm"),
            Self::Groovy => include_str!("../queries/groovy/highlights.scm"),
            Self::Markdown => include_str!("../queries/markdown/highlights.scm"),
            Self::Shell => include_str!("../queries/shell/highlights.scm"),
            Self::Dockerfile => include_str!("../queries/dockerfile/highlights.scm"),
            Self::Sql => include_str!("../queries/sql/highlights.scm"),
        }
    }

    /// The compiled highlight query, built once per language.
    pub(crate) fn highlights_query(self) -> Result<&'static Query, SyntaxError> {
        static QUERIES: [OnceLock<Result<Query, String>>; Language::ALL.len()] =
            [const { OnceLock::new() }; Language::ALL.len()];
        // `ALL` lists the variants in declaration order (see the test below).
        let slot = &QUERIES[self as usize];
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
        assert_eq!(detect("pom.xml"), Some(Language::Xml));
        assert_eq!(detect("README.md"), Some(Language::Markdown));
        assert_eq!(detect("gradlew"), Some(Language::Shell));
        assert_eq!(detect("Dockerfile"), Some(Language::Dockerfile));
        assert_eq!(detect("Dockerfile.dev"), Some(Language::Dockerfile));
        assert_eq!(detect("app.dockerfile"), Some(Language::Dockerfile));
        assert_eq!(detect("Dockerfiles"), None);
        assert_eq!(detect("schema.SQL"), Some(Language::Sql));
        assert_eq!(detect("Makefile"), None);
    }

    #[test]
    fn every_language_round_trips_through_its_id() {
        for &language in Language::ALL {
            assert_eq!(Language::from_id(language.as_str()), Some(language));
        }
        assert_eq!(Language::from_id("cobol"), None);
    }

    #[test]
    fn all_lists_the_languages_in_declaration_order() {
        for (index, &language) in Language::ALL.iter().enumerate() {
            assert_eq!(language as usize, index);
        }
    }

    #[test]
    fn every_highlight_query_compiles() {
        for &language in Language::ALL {
            language.highlights_query().unwrap();
        }
    }
}
