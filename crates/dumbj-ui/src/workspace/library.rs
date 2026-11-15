use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dumbj_index::Index;
use dumbj_index::LibraryTarget;
use dumbj_jars::{
    ClassSource, JarIndex, default_roots, discover, discover_jdk, locate, named_by_build,
    read_virtual,
};
use dumbj_syntax::Language;
use gpui::{AppContext, Context, Window};

use super::Workspace;
use super::navigation::{Chooser, ChooserItem, Location};

struct Found {
    source: ClassSource,
    location: Location,
}

fn open_in_library(source: &ClassSource, target: &LibraryTarget) -> Option<Found> {
    let path = source.virtual_path();
    let language = Language::from_path(&path)?;
    let text = read_virtual(&path).ok()?;
    let class = target.class.rsplit('.').next().unwrap_or(&target.class);
    let offset = locate(&text, language, class, target.member.as_deref());
    Some(Found {
        source: source.clone(),
        location: Location { path, offset },
    })
}

const BUILD_FILES: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "libs.versions.toml",
    "gradle.properties",
    "pom.xml",
];

fn build_text(index: &Index) -> String {
    index
        .files()
        .filter(|(_, file)| {
            file.path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| BUILD_FILES.contains(&name))
        })
        .filter_map(|(_, file)| std::fs::read_to_string(&file.path).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

impl Workspace {
    pub(super) fn start_jar_indexing(&mut self, cx: &mut Context<Self>) {
        if self.jars.is_some() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let jars = cx
                .background_spawn(async move {
                    let cancelled = AtomicBool::new(false);
                    let mut jars = discover(&default_roots(), &cancelled)?;
                    jars.extend(discover_jdk());
                    JarIndex::build(jars, &cancelled)
                })
                .await;
            let _ = this.update(cx, |this, _| {
                if let Some(jars) = jars {
                    tracing::info!(
                        jars = jars.jar_count(),
                        elapsed = ?started.elapsed(),
                        "library sources indexed"
                    );
                    this.jars = Some(Arc::new(jars));
                }
            });
        })
        .detach();
    }

    /// Go to Declaration found nothing in the project: look in the library sources.
    pub(super) fn goto_library(
        &mut self,
        name: String,
        targets: Vec<LibraryTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(jars) = self.jars.clone() else {
            self.status_message = Some(format!(
                "Cannot find declaration of {name}; library sources are still being indexed"
            ));
            cx.notify();
            return;
        };
        let targets_named_a_class = !targets.is_empty();
        let Some((target, sources)) = targets.into_iter().find_map(|target| {
            let sources = jars.find(&target.class);
            (!sources.is_empty()).then_some((target, sources))
        }) else {
            self.status_message = Some(if targets_named_a_class {
                format!("No sources available for {name}")
            } else {
                format!("Cannot find declaration of {name}")
            });
            cx.notify();
            return;
        };
        let handle = window.window_handle();
        let index = self.index.clone();
        cx.spawn(async move |this, cx| {
            let found: Vec<Found> = cx
                .background_spawn(async move {
                    let build = index.as_deref().map_or_else(String::new, build_text);
                    let pinned: Vec<&ClassSource> = sources
                        .iter()
                        .filter(|source| named_by_build(&source.jar, &build))
                        .collect();
                    let chosen: Vec<&ClassSource> = if pinned.len() == 1 {
                        pinned
                    } else {
                        sources.iter().collect()
                    };
                    chosen
                        .into_iter()
                        .filter_map(|source| open_in_library(source, &target))
                        .collect()
                })
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.show_library(&name, found, window, cx));
            });
        })
        .detach();
    }

    fn show_library(
        &mut self,
        name: &str,
        found: Vec<Found>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match found.as_slice() {
            [] => self.status_message = Some(format!("Cannot read the sources of {name}")),
            [only] => self.navigate_to(only.location.clone(), window, cx),
            _ => {
                let items = found
                    .into_iter()
                    .map(|found| {
                        let jar = &found.source.jar;
                        ChooserItem::new(
                            format!("{} {}", jar.artifact, jar.version),
                            found.source.entry.clone(),
                            found.location,
                        )
                    })
                    .collect();
                self.chooser = Some(Chooser::new(format!("Sources of {name}"), items));
                let focus = self.focus.clone();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }
}
