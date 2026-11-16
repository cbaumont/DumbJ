use std::path::Path;
use std::time::Duration;

use dumbj_lsp::{GotoKind, Place, from_lsp, goto, uri_to_path};
use dumbj_text::Buffer;
use gpui::{AppContext, Context, Task, Window};

use super::Workspace;
use super::navigation::{Chooser, ChooserItem, Location};

/// How long the exact answer may take before the dumb one is shown in its place.
const DUMB_AFTER: Duration = Duration::from_millis(300);
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);

struct ExactTarget {
    location: Location,
    label: String,
    detail: String,
}

enum Exact {
    Targets(Vec<ExactTarget>),
    /// The server points into a class file or another document that is not a file on disk.
    NotAFile,
    Failed(String),
}

enum Phase {
    Waiting,
    /// The dumb result was shown; `landed` is where it jumped, if it did.
    DumbShown {
        landed: Option<Location>,
    },
}

pub(super) struct SmartNav {
    generation: u64,
    kind: GotoKind,
    origin: Location,
    phase: Phase,
    _request: Task<()>,
    _timer: Task<()>,
}

fn exact_target(place: &Place) -> Option<ExactTarget> {
    let path = uri_to_path(&place.uri)?;
    let text = std::fs::read_to_string(&path).ok()?;
    Some(target_in(&path, &Buffer::new(&text), place))
}

fn target_in(path: &Path, buffer: &Buffer, place: &Place) -> ExactTarget {
    let offset = from_lsp(buffer, place.range.start);
    let line = buffer.point_at(offset).line;
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    ExactTarget {
        label: format!("{name}:{}", line.display_number()),
        detail: buffer.line(line).trim().to_owned(),
        location: Location {
            path: path.to_path_buf(),
            offset,
        },
    }
}

fn exact(result: Result<Vec<Place>, dumbj_lsp::LspError>) -> Exact {
    match result {
        Err(error) => Exact::Failed(error.to_string()),
        Ok(places) => places
            .iter()
            .map(exact_target)
            .collect::<Option<Vec<_>>>()
            .map_or(Exact::NotAFile, Exact::Targets),
    }
}

fn title(kind: GotoKind) -> &'static str {
    match kind {
        GotoKind::Definition => "Declarations",
        GotoKind::TypeDefinition => "Type declarations",
        GotoKind::Implementation => "Implementations",
    }
}

impl Workspace {
    /// Asks the language server of the active file. Returns false when there is none to
    /// ask, so the caller answers from the index.
    pub(super) fn smart_goto(
        &mut self,
        kind: GotoKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.smart.nav = None;
        let Some(origin) = self.current_location(cx) else {
            return false;
        };
        let (Some(server), Some(editor)) = (self.smart_server(&origin.path), self.active_editor())
        else {
            return false;
        };
        let Some(position) = editor.read(cx).lsp_caret() else {
            return false;
        };
        self.smart.generation += 1;
        let generation = self.smart.generation;
        let path = origin.path.clone();
        let answer = cx.background_spawn(async move {
            exact(goto(&server, kind, &path, position, GIVE_UP_AFTER))
        });
        let request = cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.exact_arrived(generation, answer, window, cx);
            });
        });
        let timer = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DUMB_AFTER).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.exact_is_slow(generation, window, cx);
            });
        });
        self.smart.nav = Some(SmartNav {
            generation,
            kind,
            origin,
            phase: Phase::Waiting,
            _request: request,
            _timer: timer,
        });
        true
    }

    fn dumb_goto(&mut self, kind: GotoKind, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.current_location(cx);
        match kind {
            GotoKind::Definition => self.goto_declaration_with(false, window, cx),
            GotoKind::Implementation => self.goto_implementation_with(false, window, cx),
            GotoKind::TypeDefinition => {
                self.status_message = Some("No type declaration found".to_owned());
            }
        }
        if self.chooser.is_none() && self.current_location(cx) != before {
            self.status_message = Some("best guess".to_owned());
        }
        cx.notify();
    }

    fn exact_is_slow(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.current_location(cx);
        let Some(nav) = self
            .smart
            .nav
            .as_mut()
            .filter(|nav| nav.generation == generation && matches!(nav.phase, Phase::Waiting))
        else {
            return;
        };
        if here.as_ref() != Some(&nav.origin) {
            self.smart.nav = None;
            return;
        }
        let kind = nav.kind;
        if kind == GotoKind::TypeDefinition {
            self.status_message = Some("Waiting for the language server…".to_owned());
            cx.notify();
            return;
        }
        self.smart.last_jump = None;
        self.dumb_goto(kind, window, cx);
        let landed = self.smart.last_jump.clone();
        if let Some(nav) = self.smart.nav.as_mut() {
            nav.phase = Phase::DumbShown { landed };
        }
    }

    fn exact_arrived(
        &mut self,
        generation: u64,
        answer: Exact,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(nav) = self.smart.nav.take_if(|nav| nav.generation == generation) else {
            return;
        };
        let here = self.current_location(cx);
        let targets = match answer {
            Exact::Targets(targets) if !targets.is_empty() => targets,
            other => {
                if let Exact::Failed(error) = &other {
                    tracing::debug!(%error, "the language server did not answer");
                }
                if matches!(nav.phase, Phase::Waiting) && here.as_ref() == Some(&nav.origin) {
                    self.dumb_goto(nav.kind, window, cx);
                }
                return;
            }
        };
        match nav.phase {
            Phase::Waiting if here.as_ref() == Some(&nav.origin) => {}
            Phase::DumbShown { landed: None } if self.chooser.is_some() => self.chooser = None,
            Phase::DumbShown {
                landed: Some(landed),
            } if here.as_ref() == Some(&landed) => {
                if let [only] = targets.as_slice()
                    && only.location == landed
                {
                    self.status_message = Some("exact".to_owned());
                    cx.notify();
                    return;
                }
            }
            _ => return,
        }
        self.show_exact(nav.kind, targets, window, cx);
    }

    fn show_exact(
        &mut self,
        kind: GotoKind,
        mut targets: Vec<ExactTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if targets.len() == 1 {
            if let Some(only) = targets.pop() {
                self.navigate_to(only.location, window, cx);
                self.status_message = Some("exact".to_owned());
            }
        } else {
            let items = targets
                .into_iter()
                .map(|target| ChooserItem::new(target.label, target.detail, target.location))
                .collect();
            self.chooser = Some(Chooser::smart(title(kind).to_owned(), items));
            let focus = self.focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn goto_type_declaration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smart_goto(GotoKind::TypeDefinition, window, cx) {
            self.status_message = Some("Go to Type Declaration needs smart mode".to_owned());
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use dumbj_lsp::path_to_uri;
    use lsp_types::{Position, Range};

    use super::*;

    #[test]
    fn a_place_becomes_a_byte_offset_with_its_line_as_detail() {
        let buffer = Buffer::new("class A {\n    val 𝒳 = b()\n}\n");
        let path = Path::new("/p/A.kt");
        let start = Position::new(1, 13);
        let place = Place {
            uri: path_to_uri(path),
            range: Range::new(start, start),
        };
        let target = target_in(path, &buffer, &place);
        assert_eq!(target.label, "A.kt:2");
        assert_eq!(target.detail, "val 𝒳 = b()");
        assert_eq!(
            &buffer.to_string()[target.location.offset.get()..][..3],
            "b()"
        );
    }

    #[test]
    fn a_class_file_uri_is_not_a_file() {
        let place = Place {
            uri: "jdt://contents/rt.jar/java.lang/String.class"
                .parse()
                .unwrap(),
            range: Range::default(),
        };
        assert!(matches!(exact(Ok(vec![place])), Exact::NotAFile));
    }
}
