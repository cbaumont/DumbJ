# DumbJ architecture

Status: M0 (spike and skeleton) done, awaiting review.

## Crate map

All crates live in `crates/`. Only `dumbj-ui` depends on GPUI; everything else is headless and unit-tested.

| Crate | Role | Depends on |
|---|---|---|
| `dumbj-text` | Rope `Buffer`, typed positions (`ByteOffset`, `LineIndex`, `Point`), edits, literal search | none |
| `dumbj-syntax` | tree-sitter: language detection, cancellable parsing, range-limited highlighting; query files in `queries/` | text |
| `dumbj-actions` | Action registry, IntelliJ keymap XML loader, action ID map, key dispatcher, `KEYMAP.md` generator | none |
| `dumbj-settings` | Themes as data (palette + role mapping); user settings later | syntax |
| `dumbj-ui` | GPUI front end: workspace window, file tree, tabs, editor view, text input, popups | all of the above |
| `dumbj-app` | The `dumbj` binary: CLI, logging, starts the UI | ui |

Planned, not created yet: `dumbj-index`, `dumbj-lsp`, `dumbj-search`, `dumbj-vcs`, `dumbj-review`, `dumbj-platform`.

Data that ships in the binary lives at the repo root: `keymaps/` (vendored XML + `action-id-map.toml`), `themes/` (Catppuccin palettes + `roles.toml`), `assets/` (JetBrains Mono, Lucide icons). Each has a NOTICE or LICENSE naming its source and revision.

Inside `dumbj-ui`, logic that needs no GPUI types sits in plain modules (`editor_model`, `line_edit`, `tree_model`, `keys`) so it is tested without a window. The GPUI views (`workspace`, `editor`, `file_tree`, `text_input`) stay thin.

## Data flow

- **Keys.** GPUI key event → `keys::stroke_from_gpui` → `KeyDispatcher` (single strokes, chords, double-tapped modifiers) → first *enabled* `Action` → `Workspace::perform`, which routes by focus (popup field, find field, tree, editor). Unhandled keys continue to the focused text field's IME handler. Bindings come only from the resolved keymap: vendored XML → parent inheritance → `action-id-map.toml` → `Bindings`.
- **Opening a file.** `EditorView::open` spawns a background task: read → `Buffer` → `dumbj_syntax::parse`. The view shows "Loading…" until it completes.
- **Painting the editor.** Each frame shapes only the visible lines. Highlight spans come from one tree-sitter query over the visible range plus a margin, cached per buffer version.
- **Search.** The find field emits `TextChanged` → the editor searches a rope snapshot in the background → matches are painted and the status shows `current/total`.

## Threading model

- **UI thread:** GPUI's main thread. It only handles events, layout and paint. Per-frame work is bounded by the viewport (about 0.2 ms of layout for a 100k-line file in a release build).
- **Background executor:** GPUI's thread pool, reached with `cx.background_spawn`. File reads, parsing, directory listing and search run there and report back with `entity.update`.
- **Cancellation:** background work polls an `AtomicBool` (set when the view is dropped or the query changes); `parse` and `find_all` take a `cancelled` callback. Results from a superseded search are discarded.
- No second async runtime. The job system with priorities (BRIEF §8) arrives with the index in M1a; until then tasks are per view.

## Decisions

Each entry gives the decision and why. Where a decision changes the spec, the details live in `docs/BRIEF.md` and the entry points to the section.

### 2026-11-14: Licence is GPL-3.0-or-later
DumbJ is GPL-3.0-or-later (BRIEF §7). Why: it allows adapting small pieces of Zed's GPL code, and permissive dependencies remain usable. Cost: no Mac App Store.

### 2026-11-14: Zed code may be adapted in small pieces
Small attributed pieces only, never whole crates (BRIEF §3). Why: it saves effort on proven algorithms without DumbJ turning into a Zed fork.

### 2026-11-14: Own widgets and own editor on GPUI
The component library (tool windows, tabs, tree, splitter, popups, diff view, gutter) and the editor (text layout, carets, IME, folding, inlays, inline threads) are written in-house on GPUI primitives. Longbridge's `gpui-component` (Apache-2.0) is a reading reference, not a dependency, so that only one fast-moving GPUI-coupled dependency has to stay in step with our pin.

### 2026-11-14: CI is GitHub Actions in a Fedora container
Remote: `github.com/cbaumont/DumbJ`. Jobs run in a `fedora:latest` container with GPUI's system deps: fmt, clippy (`-D warnings`), nextest, cargo-deny and a build, cached with `Swatinem/rust-cache`. The real-repo accuracy benchmark runs nightly and on demand, not on every push.

### 2026-11-14: Toolchain via rustup
The toolchain is installed with user-local rustup so that `rust-toolchain.toml` pinning applies. GPUI's system deps and `mold` come from dnf.

### 2026-11-14: GPUI spike pass/fail policy
Hard blockers, which stop work and go to the owner: no working IME on GNOME Wayland, no way to detect double-Shift or capture two-stroke chords, or scrolling far below 60 fps with no plausible fix. Any other failed criterion is recorded here with a workaround plan, and work continues.

### 2026-11-14: Screenshots via headless sway + grim
Milestone screenshots come from DumbJ running under a headless sway (wlroots) compositor, captured with `grim`. This also covers 1.25× and 1.5× output scales and works in CI. GNOME-specific checks (IBus, Mutter fractional scaling) stay manual.

### 2026-11-14: M0 scope details
The third grammar is YAML. The M0 actions are CloseContent, NextTab, PreviousTab, ActivateProjectToolWindow, HideAllWindows, GotoLine, Find, FindNext, FindPrevious, EditorSelectWord (word level only until M1), and Open Folder. None of them need the index.

### 2026-11-14: Tags index stored in SQLite
The on-disk symbol/tags index uses SQLite via `rusqlite` (bundled). It is transactional for incremental re-indexing, can be inspected with the `sqlite3` CLI, and is rebuilt automatically if found corrupt. Search Everywhere fuzzy matching runs on in-memory structures loaded from it. Index entries are keyed by git blob OID where possible, so revisions share work.

### 2026-11-14: Other revisions are read from git objects
Diff sides, PR heads and bases are served by a read-only "revision filesystem" backed by `gix`, which reads git objects directly. The working tree is never checked out for review. Navigation from such a revision stays in it and opens read-only tabs with a revision badge. "Open working-tree version" is an explicit action.

### 2026-11-14: PR refs go in a private namespace
PR refs go under `refs/dumbj/pr/<n>/` (BRIEF §4.5). Why: the objects must survive gc without cluttering the user's branches the way local branches would, and FETCH_HEAD loses them on restart.

### 2026-11-14: IDEA-style autosave, undo only
Autosave like IDEA, with undo and git as the only safety net (BRIEF §2.5). Why: IDEA muscle memory matters more than guarding against stray keystrokes, and a local-history feature is out of scope for a reading tool.

### 2026-11-14: Dumb jump rule
Go to Declaration jumps directly only when exactly one candidate is in the highest rank tier (scope-local > same file > explicit import > same package > same module > repo) and its kind matches the reference position. Otherwise the chooser opens, sorted by rank. A wildcard import with collisions, an extension function, or member access on an unknown receiver always gets the chooser. The accuracy benchmark measures this.

### 2026-11-14: Library sources via a global source-jar class index
Every `*-sources.jar` under `~/.gradle/caches/modules-2/` and `~/.m2/repository/` is indexed lazily by package and class name, and cached by jar path and mtime. Versions named in build files or `libs.versions.toml` are preferred, then the newest. When several versions match, the result is labelled best guess and shows the version.

### 2026-11-14: HTTP is blocking ureq, no tokio
The review crate makes GitHub calls with blocking `ureq` (rustls) on background threads driven by GPUI's executor. No async runtime is added.

### 2026-11-14: Per-project state in XDG dirs
Index caches go in `$XDG_CACHE_HOME/dumbj/` and are safe to delete. Layout, bookmarks, viewed flags, review notes and pending comments go in `$XDG_STATE_HOME/dumbj/projects/<id>/`, where `<id>` is a hash of the canonical repo path plus its root commit. Nothing is written inside the repo.

### 2026-11-14: One process, one window per project
A single process hosts every open project, one window each, sharing the jar index, the LSP pool and settings.

### 2026-11-14: Accuracy corpus as pinned SHAs
A manifest lists about 4 OSS JVM repos at fixed commits, shallow-cloned and cached in CI. Expected Go to Declaration targets live as TOML in this repo. Unit and snapshot tests use small hand-written fixture projects.

### 2026-11-14: M1 split into M1a and M1b
M1 ships as M1a (navigation core) then M1b (the rest), with a review stop after each (BRIEF §9). Why: as one milestone it is too large, and the navigation core is the part most likely to be wrong, so it gets reviewed first.

### 2026-11-14: GPUI spike result: pass, IME on GNOME still to be confirmed by hand
Measured on Fedora 45 with the M0 window, not a throwaway prototype.
- **Scrolling a 100k-line Java file:** on the GNOME Wayland session, debug build: median frame 10.0 ms, p95 10.3 ms, which is the display's refresh interval. DumbJ's own layout cost per frame is 0.2 ms median, 0.9 ms p99 in a release build. Pass. Re-measure with `DUMBJ_SCROLL_BENCHMARK=1 dumbj <file>`.
- **Fractional scaling:** crisp at 1.25× and 1.5× under headless sway (`wp_fractional_scale_v1`). Mutter's fractional scaling is a manual check. Pass.
- **X11:** renders and takes input through Xwayland. Pass.
- **Keymap capture:** GPUI delivers modifier-only changes (double Shift is detected by our dispatcher), plain key-downs for chords, and every modifier combination tried (Ctrl+Shift+], Alt+1, Ctrl+Shift+F12, Alt+Shift+G, Alt+Up, Ctrl+Alt+Shift+8). Pass, with the layout caveat below.
- **Popup and docked panel in IDEA's look:** Go to Line popup and Project tool window built from GPUI primitives. Pass.
- **IME:** text fields implement GPUI's input handler (`zwp_text_input_v3` on Wayland, XIM on X11), and committed text and pre-edit are handled. It has **not** been exercised with IBus on GNOME yet; that is a manual check and a hard blocker if it fails.

### 2026-11-14: GPUI pinned to the Zed v1.28.1 commit
`gpui` and `gpui_platform` come from `github.com/zed-industries/zed` at `e8bc16ac…` (tag v1.28.1). The crates.io release (0.2.2) no longer builds: its `xattr 0.2.3` dependency needs a constant that current `libc` removed. A release tag is the most-tested revision available.

### 2026-11-14: Our own key dispatcher, not GPUI's keymap
Key matching lives in `dumbj-actions::KeyDispatcher`, fed from one `capture_key_down` listener on the window root. Why: IntelliJ lets several actions share a shortcut and picks by context, double-tapped modifiers are not expressible in GPUI's keymap, and the logic is testable headless and survives GPUI API churn.

### 2026-11-14: Shifted symbols assume a US layout for now
GPUI reports Ctrl+Shift+] as `ctrl-}` with Shift cleared, while keymaps name the physical key. `keys.rs` maps shifted US symbols back (`}` → Shift+`]`). On other layouts those few bindings (brackets, digits with Shift) may not match. Plan: use the key's unshifted keysym once GPUI exposes it, or upstream that.

### 2026-11-14: The vendored XML decides: Find Next is Ctrl+G
BRIEF §6.1 and the plugin README give Ctrl+Alt+G, but the plugin's XML lists `FindNext` twice (F3, then Ctrl+G) and IntelliJ keeps the later entry, so Ctrl+G it is. It shares Ctrl+G with Select Next Occurrence; the dispatcher's "first enabled action" rule resolves that. The loader also reproduces IntelliJ's `MacOSDefaultKeymap`: keymaps named `Mac OS X…` swap Control and Meta in what they inherit.

### 2026-11-14: Caret and text-editing keys are actions too
Arrow keys, Home/End, selection, clipboard, Enter and Escape are registered actions (`Scope::Text`) bound from `EditorLeft`, `$Copy` and so on, rather than hard-coded in widgets. The focused component (editor, text field, tree) interprets them.

### 2026-11-14: The M0 editor is read-only
The editor has a caret, selection, search and navigation but does not accept typing. Why: BRIEF §2.5 requires edits to come with undo and atomic autosave, and half of that would be worse than none. BRIEF §9 does not assign editing to a milestone; the owner needs to place it. IME is proven in the Find and Go to Line fields.

### 2026-11-14: Kotlin grammar is tree-sitter-kotlin-sg, provisionally
The fwcd grammar as maintained by ast-grep. It ships highlight and tags queries; `tree-sitter-kotlin-ng` ships none. M1a's grammar evaluation makes the final call. Highlight queries are vendored into `crates/dumbj-syntax/queries/` and already tuned for Kotlin.

### 2026-11-14: Only LF ends a line
`ropey` is built without its Unicode line-break set, and `dumbj-text` treats a CR before LF as part of the break. Why: line numbers must agree with git, tree-sitter and LSP.

### 2026-11-14: cargo-deny tolerates unmaintained crates that GPUI pulls in
`unmaintained = "workspace"`: an unmaintained-crate advisory fails CI only for our direct dependencies. Vulnerability advisories and yanked crates always fail. Git sources are allowed only from `zed-industries` and `gfx-rs`.
