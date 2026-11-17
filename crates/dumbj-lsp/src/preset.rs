use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use lsp_types::{
    CallHierarchyClientCapabilities, ClientCapabilities, ClientInfo, GeneralClientCapabilities,
    GotoCapability, HoverClientCapabilities, InitializeParams, InlayHintClientCapabilities,
    MarkupKind, PositionEncodingKind, PublishDiagnosticsClientCapabilities, SemanticTokenModifier,
    SemanticTokenType, SemanticTokensClientCapabilities, SemanticTokensClientCapabilitiesRequests,
    SemanticTokensFullOptions, TextDocumentClientCapabilities, TextDocumentSyncClientCapabilities,
    TokenFormat, TypeHierarchyClientCapabilities, WindowClientCapabilities, WorkspaceFolder,
};
use serde_json::json;

use crate::path_to_uri;
use crate::server::ServerCommand;

const GRADLE_FILES: [&str; 4] = [
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];
const JDTLS_MIN_JAVA: u32 = 21;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    Jdtls,
    KotlinLsp,
}

impl ServerKind {
    pub const ALL: [ServerKind; 2] = [ServerKind::Jdtls, ServerKind::KotlinLsp];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ServerKind::Jdtls => "jdtls",
            ServerKind::KotlinLsp => "kotlin-lsp",
        }
    }

    #[must_use]
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "java" => Some(ServerKind::Jdtls),
            "kt" | "kts" => Some(ServerKind::KotlinLsp),
            _ => None,
        }
    }

    #[must_use]
    pub fn language_id(self) -> &'static str {
        match self {
            ServerKind::Jdtls => "java",
            ServerKind::KotlinLsp => "kotlin",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Environment {
    pub path: Option<OsString>,
    pub java_home: Option<PathBuf>,
    /// Directories holding one JDK per subdirectory, such as sdkman's and `/usr/lib/jvm`.
    pub jdk_dirs: Vec<PathBuf>,
}

impl Environment {
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            java_home: std::env::var_os("JAVA_HOME").map(PathBuf::from),
            jdk_dirs: [
                std::env::var_os("SDKMAN_DIR")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".sdkman"))
                    })
                    .map(|sdkman| sdkman.join("candidates/java")),
                Some(PathBuf::from("/usr/lib/jvm")),
            ]
            .into_iter()
            .flatten()
            .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Detection {
    Ready(ServerCommand),
    Unavailable(String),
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn find_on_path(name: &str, env: &Environment) -> Option<PathBuf> {
    let path = env.path.as_ref()?;
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn find_java(env: &Environment) -> Option<PathBuf> {
    if let Some(home) = &env.java_home {
        let java = home.join("bin").join("java");
        if java.is_file() {
            return Some(java);
        }
    }
    find_on_path("java", env)
}

fn jdk_home(java: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(java).ok()?;
    Some(real.parent()?.parent()?.to_path_buf())
}

fn jdk_major(home: &Path) -> Option<u32> {
    let release = fs::read_to_string(home.join("release")).ok()?;
    let line = release
        .lines()
        .find_map(|line| line.trim().strip_prefix("JAVA_VERSION="))?;
    let version = line.trim().trim_matches('"');
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// The JDK homes in `env.jdk_dirs`, by canonical path.
fn installed_jdks(env: &Environment) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = env
        .jdk_dirs
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| jdk_home(&entry.path().join("bin").join("java")))
        .collect();
    homes.sort();
    homes.dedup();
    homes
}

/// The newest JDK of at least `minimum`, or the newest version found when none is.
fn newest_jdk(
    env: &Environment,
    preferred: Option<PathBuf>,
    minimum: u32,
) -> Result<Option<PathBuf>, u32> {
    let mut best: Option<(u32, PathBuf)> = None;
    for home in preferred.into_iter().chain(installed_jdks(env)) {
        let Some(major) = jdk_major(&home) else {
            best.get_or_insert((0, home));
            continue;
        };
        if best.as_ref().is_none_or(|(found, _)| major > *found) {
            best = Some((major, home));
        }
    }
    match best {
        Some((major, _)) if major != 0 && major < minimum => Err(major),
        best => Ok(best.map(|(_, home)| home)),
    }
}

#[must_use]
pub fn detect(kind: ServerKind, root: &Path, data_dir: &Path, env: &Environment) -> Detection {
    let markers: &[&str] = match kind {
        ServerKind::Jdtls => &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        ServerKind::KotlinLsp => &GRADLE_FILES,
    };
    if !markers.iter().any(|marker| root.join(marker).is_file()) {
        let needs = match kind {
            ServerKind::Jdtls => "no Gradle or Maven build files",
            ServerKind::KotlinLsp => "no Gradle build files",
        };
        return Detection::Unavailable(needs.to_owned());
    }
    let label = kind.label();
    let Some(program) = find_on_path(label, env) else {
        return Detection::Unavailable(format!("{label} is not on PATH"));
    };
    let java = find_java(env);
    if java.is_none() && installed_jdks(env).is_empty() {
        return Detection::Unavailable("no JDK found".to_owned());
    }
    let preferred = java.as_deref().and_then(jdk_home);
    // jdtls snapshots move to new JDKs ahead of its stated minimum, so it gets the newest.
    let home = match kind {
        ServerKind::Jdtls => match newest_jdk(env, preferred, JDTLS_MIN_JAVA) {
            Ok(home) => home,
            Err(major) => {
                return Detection::Unavailable(format!(
                    "jdtls needs JDK {JDTLS_MIN_JAVA}, found {major}"
                ));
            }
        },
        ServerKind::KotlinLsp => preferred.or_else(|| installed_jdks(env).pop()),
    };
    let args = match kind {
        // The initialization option alone still lets jdtls write .project, .classpath and
        // .settings into the project: the redirection is set up when the JVM starts.
        ServerKind::Jdtls => vec![
            "--jvm-arg=-Djava.import.generatesMetadataFilesAtProjectRoot=false".to_owned(),
            "-data".to_owned(),
            data_dir.join("jdtls").to_string_lossy().into_owned(),
        ],
        ServerKind::KotlinLsp => vec![
            "--stdio".to_owned(),
            "--system-path".to_owned(),
            data_dir.join("kotlin-lsp").to_string_lossy().into_owned(),
        ],
    };
    let env = home.map_or_else(Vec::new, |home| {
        vec![("JAVA_HOME".to_owned(), home.to_string_lossy().into_owned())]
    });
    Detection::Ready(ServerCommand {
        program,
        args,
        env,
        cwd: root.to_path_buf(),
    })
}

fn goto() -> GotoCapability {
    GotoCapability {
        dynamic_registration: None,
        link_support: None,
    }
}

fn capabilities() -> ClientCapabilities {
    let token_types = [
        SemanticTokenType::NAMESPACE,
        SemanticTokenType::TYPE,
        SemanticTokenType::CLASS,
        SemanticTokenType::ENUM,
        SemanticTokenType::INTERFACE,
        SemanticTokenType::STRUCT,
        SemanticTokenType::TYPE_PARAMETER,
        SemanticTokenType::PARAMETER,
        SemanticTokenType::VARIABLE,
        SemanticTokenType::PROPERTY,
        SemanticTokenType::ENUM_MEMBER,
        SemanticTokenType::EVENT,
        SemanticTokenType::FUNCTION,
        SemanticTokenType::METHOD,
        SemanticTokenType::MACRO,
        SemanticTokenType::KEYWORD,
        SemanticTokenType::MODIFIER,
        SemanticTokenType::COMMENT,
        SemanticTokenType::STRING,
        SemanticTokenType::NUMBER,
        SemanticTokenType::REGEXP,
        SemanticTokenType::OPERATOR,
        SemanticTokenType::DECORATOR,
    ]
    .to_vec();
    let token_modifiers = [
        SemanticTokenModifier::DECLARATION,
        SemanticTokenModifier::DEFINITION,
        SemanticTokenModifier::READONLY,
        SemanticTokenModifier::STATIC,
        SemanticTokenModifier::DEPRECATED,
        SemanticTokenModifier::ABSTRACT,
        SemanticTokenModifier::ASYNC,
        SemanticTokenModifier::MODIFICATION,
        SemanticTokenModifier::DOCUMENTATION,
        SemanticTokenModifier::DEFAULT_LIBRARY,
    ]
    .to_vec();
    ClientCapabilities {
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..Default::default()
        }),
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(vec![PositionEncodingKind::UTF16]),
            ..Default::default()
        }),
        text_document: Some(TextDocumentClientCapabilities {
            synchronization: Some(TextDocumentSyncClientCapabilities::default()),
            definition: Some(goto()),
            type_definition: Some(goto()),
            implementation: Some(goto()),
            references: Some(lsp_types::ReferenceClientCapabilities::default()),
            hover: Some(HoverClientCapabilities {
                dynamic_registration: None,
                content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
            }),
            call_hierarchy: Some(CallHierarchyClientCapabilities::default()),
            type_hierarchy: Some(TypeHierarchyClientCapabilities::default()),
            inlay_hint: Some(InlayHintClientCapabilities::default()),
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities::default()),
            semantic_tokens: Some(SemanticTokensClientCapabilities {
                dynamic_registration: None,
                requests: SemanticTokensClientCapabilitiesRequests {
                    range: None,
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                },
                token_types,
                token_modifiers,
                formats: vec![TokenFormat::RELATIVE],
                overlapping_token_support: None,
                multiline_token_support: None,
                server_cancel_support: None,
                augments_syntax_tokens: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[must_use]
#[allow(deprecated)]
pub fn initialize_params(kind: ServerKind, root: &Path) -> InitializeParams {
    let uri = path_to_uri(root);
    let name = root
        .file_name()
        .map_or_else(|| "root".to_owned(), |n| n.to_string_lossy().into_owned());
    let initialization_options = match kind {
        ServerKind::Jdtls => Some(json!({
            "settings": {"java": {
                "import": {"generatesMetadataFilesAtProjectRoot": false},
                "autobuild": {"enabled": false},
            }},
            "extendedClientCapabilities": {
                "progressReportProvider": false,
                "classFileContentsSupport": true,
            },
        })),
        ServerKind::KotlinLsp => None,
    };
    InitializeParams {
        process_id: Some(std::process::id()),
        root_uri: Some(uri.clone()),
        workspace_folders: Some(vec![WorkspaceFolder { uri, name }]),
        client_info: Some(ClientInfo {
            name: "DumbJ".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
        capabilities: capabilities(),
        initialization_options,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "dumbj-lsp-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn touch(&self, relative: &str, mode: u32) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            path
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn reason(detection: Detection) -> String {
        match detection {
            Detection::Unavailable(reason) => reason,
            Detection::Ready(command) => panic!("unexpectedly ready: {command:?}"),
        }
    }

    struct Setup {
        dir: Dir,
        root: PathBuf,
        data: PathBuf,
        env: Environment,
    }

    fn setup(java_version: Option<&str>) -> Setup {
        let dir = Dir::new();
        dir.write("project/build.gradle.kts", "");
        dir.touch("bin/jdtls", 0o755);
        dir.touch("bin/kotlin-lsp", 0o755);
        dir.touch("jdk/bin/java", 0o755);
        if let Some(version) = java_version {
            dir.write("jdk/release", &format!("JAVA_VERSION=\"{version}\"\n"));
        }
        let env = Environment {
            path: Some(dir.0.join("bin").into_os_string()),
            java_home: Some(dir.0.join("jdk")),
            jdk_dirs: vec![dir.0.join("jdks")],
        };
        Setup {
            root: dir.0.join("project"),
            data: dir.0.join("data"),
            env,
            dir,
        }
    }

    #[test]
    fn server_for_path() {
        assert_eq!(
            ServerKind::for_path(Path::new("A.java")),
            Some(ServerKind::Jdtls)
        );
        assert_eq!(
            ServerKind::for_path(Path::new("a/B.kt")),
            Some(ServerKind::KotlinLsp)
        );
        assert_eq!(
            ServerKind::for_path(Path::new("c.gradle.kts")),
            Some(ServerKind::KotlinLsp)
        );
        assert_eq!(ServerKind::for_path(Path::new("a.rs")), None);
        assert_eq!(ServerKind::for_path(Path::new("Makefile")), None);
    }

    #[test]
    fn markers_are_required() {
        let s = setup(Some("21.0.1"));
        fs::remove_file(s.root.join("build.gradle.kts")).unwrap();
        assert_eq!(
            reason(detect(ServerKind::Jdtls, &s.root, &s.data, &s.env)),
            "no Gradle or Maven build files"
        );
        s.dir.write("project/pom.xml", "");
        assert!(matches!(
            detect(ServerKind::Jdtls, &s.root, &s.data, &s.env),
            Detection::Ready(_)
        ));
        assert_eq!(
            reason(detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env)),
            "no Gradle build files"
        );
    }

    #[test]
    fn binary_must_be_executable_on_path() {
        let mut s = setup(Some("21.0.1"));
        assert!(matches!(
            detect(ServerKind::Jdtls, &s.root, &s.data, &s.env),
            Detection::Ready(_)
        ));
        s.dir.touch("bin/jdtls", 0o644);
        assert_eq!(
            reason(detect(ServerKind::Jdtls, &s.root, &s.data, &s.env)),
            "jdtls is not on PATH"
        );
        s.env.path = None;
        assert_eq!(
            reason(detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env)),
            "kotlin-lsp is not on PATH"
        );
    }

    #[test]
    fn a_jdk_is_required() {
        let mut s = setup(None);
        s.env.java_home = None;
        assert_eq!(
            reason(detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env)),
            "no JDK found"
        );
    }

    #[test]
    fn jdk_found_on_path_when_java_home_is_unset() {
        let mut s = setup(Some("21"));
        s.env.java_home = None;
        s.dir.touch("bin/java", 0o755);
        let Detection::Ready(command) = detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env)
        else {
            panic!("not ready");
        };
        assert!(command.env.is_empty() || command.env[0].0 == "JAVA_HOME");
    }

    #[test]
    fn jdtls_needs_a_new_jdk() {
        let s = setup(Some("17.0.9"));
        assert_eq!(
            reason(detect(ServerKind::Jdtls, &s.root, &s.data, &s.env)),
            "jdtls needs JDK 21, found 17"
        );
        assert!(matches!(
            detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env),
            Detection::Ready(_)
        ));
        s.dir.write("jdk/release", "JAVA_VERSION=\"1.8.0_292\"\n");
        assert_eq!(
            reason(detect(ServerKind::Jdtls, &s.root, &s.data, &s.env)),
            "jdtls needs JDK 21, found 8"
        );
    }

    #[test]
    fn jdtls_takes_the_newest_installed_jdk() {
        let s = setup(Some("21.0.8"));
        s.dir.touch("jdks/25/bin/java", 0o755);
        s.dir.write("jdks/25/release", "JAVA_VERSION=\"25.0.3\"\n");
        s.dir.touch("jdks/17/bin/java", 0o755);
        s.dir.write("jdks/17/release", "JAVA_VERSION=\"17.0.1\"\n");
        let Detection::Ready(jdtls) = detect(ServerKind::Jdtls, &s.root, &s.data, &s.env) else {
            panic!("not ready");
        };
        let newest = std::fs::canonicalize(s.dir.0.join("jdks/25")).unwrap();
        assert_eq!(jdtls.env[0].1, newest.to_string_lossy());
        let Detection::Ready(kotlin) = detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env)
        else {
            panic!("not ready");
        };
        assert!(
            kotlin.env[0].1.ends_with("jdk"),
            "kotlin-lsp keeps JAVA_HOME"
        );
    }

    #[test]
    fn an_installed_jdk_is_enough_without_java_home_or_path() {
        let mut s = setup(None);
        s.env.java_home = None;
        s.env.path = Some(s.dir.0.join("bin").into_os_string());
        std::fs::remove_file(s.dir.0.join("jdk/bin/java")).unwrap();
        s.dir.touch("jdks/25/bin/java", 0o755);
        assert!(matches!(
            detect(ServerKind::Jdtls, &s.root, &s.data, &s.env),
            Detection::Ready(_)
        ));
    }

    #[test]
    fn missing_release_file_accepts_the_jdk() {
        let s = setup(None);
        assert!(matches!(
            detect(ServerKind::Jdtls, &s.root, &s.data, &s.env),
            Detection::Ready(_)
        ));
    }

    #[test]
    fn ready_commands() {
        let s = setup(Some("21.0.8"));
        let home = fs::canonicalize(s.dir.0.join("jdk")).unwrap();
        let java_home = ("JAVA_HOME".to_owned(), home.to_string_lossy().into_owned());
        let Detection::Ready(jdtls) = detect(ServerKind::Jdtls, &s.root, &s.data, &s.env) else {
            panic!("not ready");
        };
        assert_eq!(jdtls.program, s.dir.0.join("bin/jdtls"));
        assert_eq!(
            jdtls.args,
            [
                "--jvm-arg=-Djava.import.generatesMetadataFilesAtProjectRoot=false".to_owned(),
                "-data".to_owned(),
                format!("{}/jdtls", s.data.display())
            ]
        );
        assert_eq!(jdtls.env, std::slice::from_ref(&java_home));
        assert_eq!(jdtls.cwd, s.root);
        let Detection::Ready(kotlin) = detect(ServerKind::KotlinLsp, &s.root, &s.data, &s.env)
        else {
            panic!("not ready");
        };
        assert_eq!(
            kotlin.args,
            [
                "--stdio".to_owned(),
                "--system-path".to_owned(),
                format!("{}/kotlin-lsp", s.data.display())
            ]
        );
        assert_eq!(kotlin.env, [java_home]);
    }

    #[test]
    fn jdtls_params_never_write_into_the_project() {
        let params = initialize_params(ServerKind::Jdtls, Path::new("/work/proj"));
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(
            value["initializationOptions"]["settings"]["java"]["import"]["generatesMetadataFilesAtProjectRoot"],
            false
        );
        assert_eq!(value["rootUri"], "file:///work/proj");
        assert_eq!(value["workspaceFolders"][0]["uri"], "file:///work/proj");
        assert_eq!(value["clientInfo"]["name"], "DumbJ");
        assert_eq!(value["capabilities"]["window"]["workDoneProgress"], true);
        assert_eq!(
            value["capabilities"]["general"]["positionEncodings"],
            serde_json::json!(["utf-16"])
        );
        let text = &value["capabilities"]["textDocument"];
        assert_eq!(
            text["semanticTokens"]["requests"]["full"],
            serde_json::json!(true)
        );
        assert!(text["definition"].get("linkSupport").is_none());
        assert_eq!(
            text["hover"]["contentFormat"],
            serde_json::json!(["markdown", "plaintext"])
        );
    }

    #[test]
    fn kotlin_params_have_no_initialization_options() {
        let params = initialize_params(ServerKind::KotlinLsp, Path::new("/work/proj"));
        assert!(params.initialization_options.is_none());
    }
}
