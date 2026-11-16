# DumbJ architecture

Status: M0 approved on 2026-11-14, M1a (navigation core) on 2026-11-15. M1b in progress.

## Crate map

All crates live in `crates/`. Only `dumbj-ui` depends on GPUI; everything else is headless and unit-tested.

| Crate | Role | Depends on |
|---|---|---|
| `dumbj-text` | Rope `Buffer`, typed positions (`ByteOffset`, `LineIndex`, `Point`), edits, undo/redo `History` with typing groups, literal search, and `disk` (atomic writes, file stamps, line-ending detection) | none |
| `dumbj-syntax` | tree-sitter: language detection, cancellable parsing, incremental reparsing, range-limited highlighting; query files in `queries/`. Also declaration extraction (`FileFacts`, `Symbol`: kind, supertypes, extension receiver, declared type, parameters, expect/actual), reference classification, local scope resolution and a small expression model (`Qualifier`: calls with arguments, member reads, casts, literals, lambdas) for Java and Kotlin | text |
| `dumbj-index` | Dumb navigation, headless: immutable `Index` snapshot (files, symbols, lookup by name, FQN and package), parallel cancellable indexer, SQLite cache of per-file facts, project model (modules and source roots), Go to Declaration (`resolve`), Find Usages (`usages`), fuzzy search for Search Everywhere, Go to Class/File/Symbol and File Structure (`search`, `fuzzy`), and the dumb hierarchy (`hierarchy`: subtypes and supertypes, overriders, super members, gutter marks, callers). Example: `accuracy` (benchmark runner) | syntax, text; `ignore`, `rusqlite` |
| `dumbj-jars` | Library sources, headless: finds source jars in the Gradle and Maven caches and the JDK's `src.zip`, indexes them by class (`JarIndex`), reads entries in place through virtual paths (`lib.jar!/pkg/Class.java`) and locates a class or member in a source | syntax, text; `ignore`, `zip` |
| `dumbj-search` | Find in Files, headless: a parallel, cancellable line search (literal or regex, case, whole word, file mask) that honours `.gitignore` and skips binary and huge files | `ignore`, `regex`, `globset` |
| `dumbj-actions` | Action registry, IntelliJ keymap XML loader, action ID map, key dispatcher, `KEYMAP.md` generator | none |
| `dumbj-settings` | Themes as data (palette + role mapping); per-project state (`ProjectState`: recent files, recent locations, bookmarks) and its directory; user settings later | syntax |
| `dumbj-ui` | GPUI front end: workspace window, file tree, tabs, editor view, text input, popups, chooser, Find and Hierarchy tool windows | all of the above |
| `dumbj-app` | The `dumbj` binary: CLI, logging, starts the UI | ui |

Planned, not created yet: `dumbj-lsp`, `dumbj-vcs`, `dumbj-review`, `dumbj-platform`.

Data that ships in the binary lives at the repo root: `keymaps/` (vendored XML + `action-id-map.toml`), `themes/` (Catppuccin palettes + `roles.toml`), `assets/` (JetBrains Mono, Lucide icons). Each has a NOTICE or LICENSE naming its source and revision.

Inside `dumbj-ui`, logic that needs no GPUI types sits in plain modules (`editor_model`, `line_edit`, `tree_model`, `keys`) so it is tested without a window. The GPUI views (`workspace`, `editor`, `file_tree`, `text_input`) stay thin.

## Data flow

- **Keys.** GPUI key event → `keys::stroke_from_gpui` → `KeyDispatcher` (single strokes, chords, double-tapped modifiers) → first *enabled* `Action` → `Workspace::perform`, which routes by focus (popup field, find field, tree, editor). Unhandled keys continue to the focused text field's IME handler. Bindings come only from the resolved keymap: vendored XML → parent inheritance → `action-id-map.toml` → `Bindings`.
- **Opening a file.** `EditorView::open` spawns a background task: read → `Buffer` → `dumbj_syntax::parse`. The view shows "Loading…" until it completes.
- **Painting the editor.** Each frame shapes only the visible lines. Highlight spans come from one tree-sitter query over the visible range plus a margin, cached per buffer version.
- **Indexing.** Opening a folder makes `Workspace::start_indexing` spawn a background task that walks the project (honouring `.gitignore`), parses source files in parallel and builds an `Index`. File facts come from the SQLite cache in `$XDG_CACHE_HOME/dumbj/` when a file's path, mtime and size are unchanged; the cache is rebuilt if corrupt or written by another schema version, and indexing continues without it if it cannot be opened. The status bar shows "Indexing… N files", then "dumb". The finished `Index` is stored in the workspace as an `Arc`; until then the navigation actions are disabled. The index is an immutable snapshot, replaced as a whole when it is refreshed (see Index refresh).
- **Go to Declaration.** Ctrl+B or Ctrl+click → the editor takes the reference at the caret from its syntax tree → `dumbj_index::resolve` ranks candidates → `Decision::Jump` navigates and records the jump for Back/Forward; `Choose` opens a chooser popup headed "best guess"; `NotFound` does nothing. On a declaration the action shows its usages instead.
- **Search popups.** Search Everywhere (with All, Classes, Files and Symbols tabs), Go to Class, Go to File, Go to Symbol and File Structure share one popup. Each keystroke starts a fuzzy match over the `Index` snapshot on the background executor; a newer query cancels the older one.
- **Find Usages.** The target at the caret → `find_usages` scans the indexed files in the background, checks each identifier with the same resolver as Go to Declaration and labels it sure or maybe. Find Usages lists the results in the Find tool window, Show Usages in a popup; "maybe" results are marked.
- **Hierarchy.** `dumbj_index::hierarchy` derives everything from the supertypes recorded in the `Index`; the subtype map is built once per snapshot, on first use. Go to Implementation and Go to Super Method take the symbol at the caret and jump, or open a chooser when there are several. The editor asks for `gutter_marks` of its file and paints the implements/overrides icons; clicking one runs the same lookup. Type Hierarchy fills the Hierarchy tool window directly; Call Hierarchy runs `callers` (Find Usages on the method, grouped by enclosing declaration) in the background. All of it is dumb and labelled best guess.
- **Library sources.** Opening a project starts one background task that lists the source jars under `~/.gradle` and `~/.m2` and the JDK's `src.zip`, and builds a `JarIndex` from their entry names (no file is unpacked). When Go to Declaration finds nothing in the project, the imported or qualified class name is looked up there; jars named by the project's build files rank first. The entry is read straight from the jar into a read-only tab under a virtual path, and `locate` finds the class or member in it. Library files are not indexed, so navigation stops there.
- **Folding and sticky lines.** `SyntaxTree::folds` lists the multi-line blocks, comments and import groups of a file; the editor keeps the collapsed ones and maps display rows to buffer lines when it paints and moves the caret. `SyntaxTree::sticky_scopes` gives the declarations enclosing the first visible line, painted pinned at the top.
- **Find in Files.** The popup debounces typing, then runs `dumbj_search::find_in_files` in the background over the project or the current directory; a newer query cancels the older one. Results show in the Find tool window, grouped by file, labelled exact.
- **Project state.** Recent files, recent locations and bookmarks live in `ProjectState`, read in the background from `$XDG_STATE_HOME/dumbj/projects/<id>/state.toml` when a folder opens (the id hashes the canonical path and the repository's root commit). Every change saves a snapshot in the background, one write at a time, through a temporary file.
- **Editing.** Typed text arrives through GPUI's input handler (`EntityInputHandler` on `EditorView`, which shows the input method only the caret's line); editing keys are actions. Both call an `EditorModel` operation, which applies the change through `dumbj_text::History` and returns the `Edit`s. The view then shifts the syntax tree (`SyntaxTree::edit`), the folds, the gutter icons and, through an `Edited` event, the project's bookmarks and recent locations, and starts a background `reparse`; the shifted tree stays in use until the new one arrives, so highlighting never drops out. Undo and redo replay the recorded changes and restore the selection.
- **Saving.** `EditorView::save` snapshots the rope and writes it in the background with `disk::write_atomic` (temporary file, fsync, rename, directory fsync). The workspace calls it when the tab or the window loses focus and when a tab closes; the editor calls it 15 s after the last edit; quitting saves on the main thread. Only modified files are written. Each editor keeps the file's stamp (mtime and length) from its last read or write: a save that finds another stamp on disk does not write, and a check on tab or window activation reloads an unmodified file and raises the conflict bar for a modified one.
- **Index refresh.** After a save, a reload, or the window regaining focus, the workspace reruns the indexer 300 ms later. Unchanged files come from the SQLite cache, so the run costs a directory walk plus the changed files; the old snapshot stays in use until the new one replaces it.
- **Search.** The find field emits `TextChanged` → the editor searches a rope snapshot in the background → matches are painted and the status shows `current/total`.

## Threading model

- **UI thread:** GPUI's main thread. It only handles events, layout and paint. Per-frame work is bounded by the viewport (about 0.2 ms of layout for a 100k-line file in a release build).
- **Background executor:** GPUI's thread pool, reached with `cx.background_spawn`. File reads, parsing, directory listing and search run there and report back with `entity.update`.
- **Cancellation:** background work polls an `AtomicBool` (set when the view is dropped or the query changes); `parse` and `find_all` take a `cancelled` callback. Results from a superseded search are discarded.
- **Index work** runs on the background executor. The indexer walks and parses with the `ignore` crate's parallel walker inside one background task; Find Usages and the search popups each run one background task per query. Each task has an `AtomicBool` that is set when its handle is dropped (indexing) or when a newer query supersedes it (usages, search); the indexer and `find_usages` poll it and return `None` when cancelled. Go to Declaration, Go to Implementation and Go to Super Method resolve one symbol against the immutable `Index` directly on the UI thread, because each is a single lookup. Find in Files spreads its file reads over scoped threads inside its one background task.
- No second async runtime. The job system with priorities (BRIEF §8) does not exist yet: M1a uses one task per operation with its own cancel flag, as above.

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

### 2026-11-14: Screenshots via headless sway + grim
Milestone screenshots come from DumbJ running under a headless sway (wlroots) compositor, captured with `grim`. This also covers 1.25× and 1.5× output scales and works in CI. GNOME-specific checks (IBus, Mutter fractional scaling) stay manual.

### 2026-11-14: Tags index stored in SQLite
The on-disk cache of per-file facts uses SQLite via `rusqlite` (bundled), keyed by path, mtime and size, and is rebuilt when corrupt or written by another schema version. Navigation and search run on the in-memory `Index` built from it.

### 2026-11-14: Other revisions are read from git objects
Diff sides, PR heads and bases are served by a read-only "revision filesystem" backed by `gix`, which reads git objects directly. The working tree is never checked out for review. Navigation from such a revision stays in it and opens read-only tabs with a revision badge. "Open working-tree version" is an explicit action.

### 2026-11-14: PR refs go in a private namespace
PR refs go under `refs/dumbj/pr/<n>/` (BRIEF §4.5). Why: the objects must survive gc without cluttering the user's branches the way local branches would, and FETCH_HEAD loses them on restart.

### 2026-11-14: IDEA-style autosave, undo only
Autosave like IDEA, with undo and git as the only safety net (BRIEF §2.5). Why: IDEA muscle memory matters more than guarding against stray keystrokes, and a local-history feature is out of scope for a reading tool.

### 2026-11-14: Library sources via a global source-jar class index
Every `*-sources.jar` under `~/.gradle/caches/modules-2/` and `~/.m2/repository/` is indexed by class name and read in place through `lib.jar!/pkg/File.kt` paths, never extracted. The version the build files pin (`libs.versions.toml`, `build.gradle(.kts)`, `pom.xml`) opens directly; otherwise a chooser lists the versions. Amended 2026-11-15: the index lives in memory, not on disk, because listing 209 jars' entries takes 0.2 s in the background at start-up. Multiplatform jars without package directories are named from each file's `package` line. The JDK's `src.zip` is not indexed yet.
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

### 2026-11-14: GPUI spike result: pass
Measured on Fedora 45 with the M0 window, not a throwaway prototype.
- **Scrolling a 100k-line Java file:** on the GNOME Wayland session, debug build: median frame 10.0 ms, p95 10.3 ms, which is the display's refresh interval. DumbJ's own layout cost per frame is 0.2 ms median, 0.9 ms p99 in a release build. Pass. Re-measure with `DUMBJ_SCROLL_BENCHMARK=1 dumbj <file>`.
- **Fractional scaling:** crisp at 1.25× and 1.5× under headless sway (`wp_fractional_scale_v1`). Mutter's fractional scaling is a manual check. Pass.
- **X11:** renders and takes input through Xwayland. Pass.
- **Keymap capture:** GPUI delivers modifier-only changes (double Shift is detected by our dispatcher), plain key-downs for chords, and every modifier combination tried (Ctrl+Shift+], Alt+1, Ctrl+Shift+F12, Alt+Shift+G, Alt+Up, Ctrl+Alt+Shift+8). Pass, with the layout caveat below.
- **Popup and docked panel in IDEA's look:** Go to Line popup and Project tool window built from GPUI primitives. Pass.
- **IME:** text fields implement GPUI's input handler (`zwp_text_input_v3` on Wayland, XIM on X11) and handle committed text and pre-edit. The owner confirmed it works with IBus on GNOME Wayland. Pass.

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
The editor has a caret, selection, search and navigation but does not accept typing. Why: BRIEF §2.5 requires edits to come with undo and atomic autosave, and half of that would be worse than none. IME is proven in the Find and Go to Line fields.

### 2026-11-14: Kotlin grammar is tree-sitter-kotlin-sg
Evaluated in M1a on 1,531 Kotlin files (5.9 MB) from kotlinx.coroutines, Now in Android and Moshi with a since-removed evaluation example:

| Grammar | Files with errors | Bytes inside ERROR nodes | Speed |
|---|---|---|---|
| `tree-sitter-kotlin-sg` 0.4 (fwcd grammar, ast-grep fork) | 26 (1.7%) | 0.29% | 4.6 MB/s |
| `tree-sitter-kotlin-ng` 1.1 (tree-sitter-grammars) | 21 (1.4%) | 1.70% | 3.6 MB/s |

`-sg` wins: its errors stay local, while `-ng` loses whole class bodies (`Channel.kt`, `JsonReader.kt`). `-sg` also ships highlight and tags queries. Its known gap is an annotated function type as a parameter type (`content: @Composable () -> Unit`), common in Compose code; a test pins it so we notice when upstream fixes it, and symbol extraction must tolerate ERROR nodes. The two grammars export the same C symbol, so only one can be linked: `-ng` sits behind the `kotlin-ng-eval` feature for re-running the comparison. Highlight queries are vendored into `crates/dumbj-syntax/queries/` and tuned.

### 2026-11-14: Only LF ends a line
`ropey` is built without its Unicode line-break set, and `dumbj-text` treats a CR before LF as part of the break. Why: line numbers must agree with git, tree-sitter and LSP.

### 2026-11-14: cargo-deny tolerates unmaintained crates that GPUI pulls in
`unmaintained = "workspace"`: an unmaintained-crate advisory fails CI only for our direct dependencies. Vulnerability advisories and yanked crates always fail. Git sources are allowed only from `zed-industries` and `gfx-rs`.

### 2026-11-14: Editing lands in M1b
Typing, undo/redo with grouping, atomic autosave and the reload prompt for files changed on disk are part of M1b (the owner left the placement open). Why: M1b already carries editor-surface work, and M2's "rollback changed lines" needs a modifiable buffer. Multi-caret, comment toggle and move/duplicate line follow in M1b if it stays reviewable, otherwise they are split out.

### 2026-11-15: Dumb jump rule
Go to Declaration ranks candidates into tiers (local, scope, same file, explicit import, same package, wildcard import, same module, repo) and jumps only when exactly one candidate is in the best tier, its kind fits the reference, and nothing in the lookup was a guess; otherwise it opens a chooser sorted by rank. The accuracy benchmark measures this.
- A receiver that is a call result has the callee's return type; a local without a written type has the type of its initialiser; a type parameter is not a type.
- A Kotlin extension jumps directly when the receiver's type is known and the file can see the extension (imported or same package), and stays a guess otherwise. Its extension receiver acts as an implicit `this`. A member beats an extension of the same name.
- An interface gives way to a same-named factory function.
- Receivers of unknown type stay guesses. That includes `Any` and `Object` (smart casts) and lambda parameters without a written type.
Why: a wrong jump is worse than a chooser, but choosers where a human sees one obvious target make navigation slow; the benchmark shows these cases are decided safely.

### 2026-11-15: Overloads are picked conservatively
Overloads and Kotlin constructors are narrowed by the call's arguments: count, named arguments, defaults, `vararg` and a trailing lambda. Types remove a candidate only where a mismatch is certain, because library types are invisible to the index and a guess about them would produce wrong jumps. Among the survivors, a single overload whose parameter types equal the argument types wins; anything else stays a chooser. Why: a wrong jump is worse than a chooser (CLAUDE.md), and dropping the true overload because a library type looked different is the likeliest way to get one.

### 2026-11-15: Kotlin expect and actual are one target, paired by path
An `expect` declaration and its `actual`s count as one target; the `actual` in the source set the reference is written in wins, otherwise the `expect`. Declarations are paired by qualified name, extension receiver and parameter types, and the source set is found by the path the `actual` shares with the `expect` (`select_platform` in `resolve.rs`), not by evaluating the build's source-set configuration. Why: the project model never runs Gradle (BRIEF §5.1), and the directory names (`commonMain`, `jvmMain`) carry the information needed.

### 2026-11-15: The nightly benchmark fails on any wrong jump
`.github/workflows/accuracy.yml` runs the benchmark nightly and on demand, caches the corpus by the hash of `corpus/manifest.toml`, and fails on a malformed case or on any wrong jump. It does not fail on a lower percentage. Why: the jump rule says a wrong jump is worse than none, so one is a regression, while the correct and chooser counts are expected to move as the engine improves.

### 2026-11-15: M1b assumptions
- Replace in Files edits many files at once, so it waits until editing has settled; M1b ships Find in Files only.
- Implements/overrides gutter icons and hierarchies are dumb (from supertypes in the index) and labelled best guess; BRIEF §4.2 calls them LSP-backed, but §5.1 puts them in M1, and smart mode replaces them in M4.
- Bookmarks and recent locations are per-project state in the XDG state directory (decision "Per-project state in XDG dirs"), never inside the repository.
- If M1b grows past one reviewable milestone, editing (phase 7) is split into its own milestone, as the decision "Editing lands in M1b" allows.

### 2026-11-15: Editing is its own milestone, M1c
Typing, undo/redo, autosave, the reload prompt and index refresh on change (M1b phase 7) leave M1b and form M1c, between M1b and M2. This supersedes "Editing lands in M1b". Why: M1b already holds hierarchy, interop, library sources, the editor surface, Find in Files, recent lists and bookmarks, which is as much as one review can take, and none of it needs a modifiable buffer; M2's "rollback changed lines" still gets one before it starts.

### 2026-11-16: Editing design (M1c)
- **One caret.** Multi-caret and column selection are left for a later milestone; the model's operations take and return edits in a form that extends to several carets.
- **The tree is shifted, then reparsed.** An edit shifts the existing tree at once and a background incremental parse replaces it. Why: parsing on the UI thread would break "never block the UI thread" on large files, and dropping the tree until the parse finishes makes highlighting flicker on every key.
- **The input method sees one line.** Offsets given to the platform are UTF-16 units within the caret's line. Why: whole-document UTF-16 offsets cost a scan of the file per keystroke.
- **Autosave triggers:** tab switch, window deactivation, tab close, quit, and 15 s idle (IDEA's default). There is no Save action; the tab shows a dot while a file is modified.
- **Disk conflicts are never resolved silently.** A save never overwrites a file whose stamp changed; the user picks "Reload from disk" (undoable) or "Keep my edits". No three-way merge.
- **Files that cannot round-trip are read-only:** library sources inside jars, and files that are not valid UTF-8 (other encodings are not supported yet).
- **Formatting follows the file:** the line ending and the indent unit are detected from the file's first 2000 lines; EditorConfig is not read yet.
- **Index refresh reruns the cached indexer** instead of patching the snapshot. Why: the snapshot is immutable and shared with background searches, and the cache already makes a rerun proportional to what changed. The edited file's own index entry is as old as its last save.
