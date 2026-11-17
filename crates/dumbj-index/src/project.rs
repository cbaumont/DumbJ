//! A module is a directory with a build file (`build.gradle`, `build.gradle.kts`,
//! `pom.xml`) or one named by `settings.gradle(.kts)` or a parent POM. Source roots are
//! inferred from package declarations: the root of `src/main/java/com/shop/A.java` in
//! package `com.shop` is `src/main/java`. That covers custom source sets, Kotlin
//! multiplatform and Android variants without evaluating any build logic.

use std::collections::{BTreeMap, BTreeSet};

use crate::index::{FileReader, IndexedFile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceKind {
    Main,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceRoot {
    pub path: String,
    pub kind: SourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// The Gradle path (`:core:model`) or directory name; `:` for the root project.
    pub name: String,
    /// The module directory relative to the project root; empty for the root.
    pub dir: String,
    pub source_roots: Vec<SourceRoot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectModel {
    modules: Vec<Module>,
}

const BUILD_FILES: [&str; 3] = ["build.gradle.kts", "build.gradle", "pom.xml"];
const SETTINGS_FILES: [&str; 2] = ["settings.gradle.kts", "settings.gradle"];

/// The project paths named by `include` statements in a Gradle settings script, Groovy or
/// Kotlin: `include(":app", ":core:model")`, `include ':app', ':lib'`.
pub(crate) fn gradle_includes(settings: &str) -> Vec<String> {
    let mut includes = Vec::new();
    let mut rest = settings;
    while let Some(at) = rest.find("include") {
        let before = rest.get(..at).unwrap_or("");
        rest = rest.get(at + "include".len()..).unwrap_or("");
        // Not `includeBuild`, `includeFlat`, or an identifier ending in "include".
        let starts_word = !before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let ends_word = rest.starts_with(|c: char| c == '(' || c.is_whitespace());
        if !starts_word || !ends_word {
            continue;
        }
        let arguments = match rest.trim_start().strip_prefix('(') {
            Some(inside) => inside.split(')').next().unwrap_or(""),
            None => rest.lines().next().unwrap_or(""),
        };
        let mut quoted = arguments.split(['"', '\'']);
        quoted.next();
        // Every second piece is inside quotes.
        includes.extend(
            quoted
                .step_by(2)
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
        );
    }
    includes
}

pub(crate) fn maven_modules(pom: &str) -> Vec<String> {
    pom.split("<module>")
        .skip(1)
        .filter_map(|rest| rest.split("</module>").next())
        .map(|module| module.trim().to_owned())
        .filter(|module| !module.is_empty())
        .collect()
}

fn parent_dir(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, child: &str) -> String {
    let child = child.trim_matches('/');
    if dir.is_empty() {
        child.to_owned()
    } else {
        format!("{dir}/{child}")
    }
}

fn source_kind(root: &str) -> SourceKind {
    let is_test = root.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        part == "test" || part == "tests" || part.ends_with("test") || part == "testfixtures"
    });
    if is_test {
        SourceKind::Test
    } else {
        SourceKind::Main
    }
}

fn source_root_of(file: &IndexedFile) -> Option<String> {
    let dir = parent_dir(&file.relative);
    let package_path = file.facts.package.as_ref()?.replace('.', "/");
    if package_path.is_empty() {
        return Some(dir.to_owned());
    }
    let root = dir.strip_suffix(&package_path)?;
    // The package must start at a directory boundary.
    (root.is_empty() || root.ends_with('/')).then(|| root.trim_end_matches('/').to_owned())
}

impl ProjectModel {
    /// Works out the modules of a project from its indexed files. Reads the settings and
    /// POM files among them through `reader`.
    #[must_use]
    pub fn discover(files: &[IndexedFile], reader: &FileReader) -> Self {
        let mut dirs: BTreeMap<String, Option<String>> = BTreeMap::from([(String::new(), None)]);
        for file in files {
            let (dir, name) = (
                parent_dir(&file.relative),
                file.relative.rsplit('/').next().unwrap_or(""),
            );
            if BUILD_FILES.contains(&name) {
                dirs.entry(dir.to_owned()).or_default();
            }
            let declared = if SETTINGS_FILES.contains(&name) {
                reader
                    .read(&file.path)
                    .map_or_else(Vec::new, |text| gradle_includes(&text))
            } else if name == "pom.xml" {
                reader
                    .read(&file.path)
                    .map_or_else(Vec::new, |text| maven_modules(&text))
            } else {
                continue;
            };
            for path in declared {
                // Gradle `:core:model` lives in `core/model` unless the build says otherwise.
                let module_dir = join(dir, &path.trim_start_matches(':').replace(':', "/"));
                let gradle_path = path.starts_with(':').then(|| path.clone());
                let entry = dirs.entry(module_dir).or_default();
                *entry = entry.take().or(gradle_path);
            }
        }
        let mut modules: Vec<Module> = dirs
            .into_iter()
            .map(|(dir, gradle_path)| {
                let name = gradle_path.unwrap_or_else(|| {
                    if dir.is_empty() {
                        ":".to_owned()
                    } else {
                        dir.rsplit('/').next().unwrap_or(&dir).to_owned()
                    }
                });
                Module {
                    name,
                    dir,
                    source_roots: Vec::new(),
                }
            })
            .collect();

        let model = Self {
            modules: modules.clone(),
        };
        let mut roots: BTreeMap<ModuleId, BTreeSet<SourceRoot>> = BTreeMap::new();
        for file in files {
            if let (Some(module), Some(root)) =
                (model.module_of_path(&file.relative), source_root_of(file))
            {
                let kind = source_kind(&root);
                roots
                    .entry(module)
                    .or_default()
                    .insert(SourceRoot { path: root, kind });
            }
        }
        for (id, found) in roots {
            if let Some(module) = modules.get_mut(id.0 as usize) {
                module.source_roots = found.into_iter().collect();
            }
        }
        Self { modules }
    }

    /// The modules, sorted by directory; the root project comes first.
    #[must_use]
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    #[must_use]
    pub fn module(&self, id: ModuleId) -> Option<&Module> {
        self.modules.get(id.0 as usize)
    }

    #[must_use]
    pub fn module_of_path(&self, relative: &str) -> Option<ModuleId> {
        self.modules
            .iter()
            .enumerate()
            .filter(|(_, module)| {
                module.dir.is_empty()
                    || relative
                        .strip_prefix(&module.dir)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|(_, module)| module.dir.len())
            .and_then(|(index, _)| Some(ModuleId(u32::try_from(index).ok()?)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dumbj_syntax::FileFacts;

    use super::*;

    fn file(relative: &str, package: Option<&str>) -> IndexedFile {
        IndexedFile {
            path: PathBuf::from("/nonexistent").join(relative),
            relative: relative.to_owned(),
            language: None,
            facts: FileFacts {
                package: package.map(str::to_owned),
                ..FileFacts::default()
            },
        }
    }

    #[test]
    fn parses_gradle_includes_in_both_dialects() {
        assert_eq!(
            gradle_includes("include(\":app\", \":core:model\")"),
            [":app", ":core:model"]
        );
        assert_eq!(
            gradle_includes("include ':app', ':lib'\ninclude ':other'"),
            [":app", ":lib", ":other"]
        );
        assert_eq!(
            gradle_includes("rootProject.name = \"x\"\ninclude(\n    \":a\",\n    \":b\",\n)\n"),
            [":a", ":b"],
            "arguments over several lines"
        );
        assert_eq!(
            gradle_includes("includeBuild(\"build-logic\")\ninclude(\":a\")"),
            [":a"]
        );
        assert_eq!(
            gradle_includes("// nothing here\nval preinclude = 1"),
            [] as [String; 0]
        );
        assert_eq!(
            gradle_includes("include(projects)"),
            [] as [String; 0],
            "non-literal arguments are skipped"
        );
    }

    #[test]
    fn parses_maven_modules() {
        let pom = "<project><modules>\n<module>api</module>\n <module> services/web </module>\n</modules></project>";
        assert_eq!(maven_modules(pom), ["api", "services/web"]);
        assert_eq!(maven_modules("<project/>"), [] as [String; 0]);
    }

    #[test]
    fn modules_come_from_build_files_and_roots_from_packages() {
        let files = [
            file("build.gradle.kts", None),
            file("app/build.gradle.kts", None),
            file(
                "app/src/main/java/com/shop/app/Cart.java",
                Some("com.shop.app"),
            ),
            file(
                "app/src/test/java/com/shop/app/CartTest.java",
                Some("com.shop.app"),
            ),
            file(
                "app/src/androidTest/kotlin/com/shop/app/UiTest.kt",
                Some("com.shop.app"),
            ),
            file("core/model/pom.xml", None),
            file(
                "core/model/src/main/kotlin/com/shop/core/Money.kt",
                Some("com.shop.core"),
            ),
            file("core/model/src/commonMain/kotlin/Top.kt", None),
            file("scripts/Tool.java", Some("")),
            file("docs/readme.md", None),
        ];
        let model = ProjectModel::discover(&files, &FileReader::default());
        let summary: Vec<String> = model
            .modules()
            .iter()
            .map(|module| {
                let roots: Vec<String> = module
                    .source_roots
                    .iter()
                    .map(|root| format!("{}:{:?}", root.path, root.kind))
                    .collect();
                format!("{} [{}] {}", module.name, module.dir, roots.join(" "))
            })
            .collect();
        assert_eq!(
            summary,
            [
                ": [] scripts:Main",
                "app [app] app/src/androidTest/kotlin:Test app/src/main/java:Main app/src/test/java:Test",
                "model [core/model] core/model/src/main/kotlin:Main",
            ]
        );
        let module_name = |path: &str| {
            model
                .module_of_path(path)
                .and_then(|id| model.module(id))
                .map(|m| m.name.as_str())
        };
        assert_eq!(
            module_name("app/src/main/java/com/shop/app/Cart.java"),
            Some("app")
        );
        assert_eq!(
            module_name("application/Other.java"),
            Some(":"),
            "a prefix is not a parent"
        );
        assert_eq!(
            module_name("core/model/src/main/kotlin/com/shop/core/Money.kt"),
            Some("model")
        );
        assert_eq!(module_name("core/README.md"), Some(":"));
    }

    #[test]
    fn a_package_that_does_not_match_the_path_gives_no_root() {
        let model = ProjectModel::discover(
            &[file("src/odd/place/A.java", Some("com.shop"))],
            &FileReader::default(),
        );
        assert_eq!(model.modules()[0].source_roots, []);
        let model = ProjectModel::discover(
            &[file("src/xcom/shop/A.java", Some("com.shop"))],
            &FileReader::default(),
        );
        assert_eq!(
            model.modules()[0].source_roots,
            [],
            "the package must start at a directory boundary"
        );
    }
}
