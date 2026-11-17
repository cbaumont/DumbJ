# DumbJ architecture

Status: M0 to M4 complete (2026-11-17).

## Crate map

All crates live in `crates/`. Only `dumbj-ui` depends on GPUI; everything else is headless and unit-tested.

| Crate | Role | Depends on |
|---|---|---|
| `dumbj-text` | Rope `Buffer`, typed positions (`ByteOffset`, `LineIndex`, `Point`), edits, undo/redo `History` with typing groups, literal search, and `disk` (atomic writes, file stamps, line-ending detection) | none |
| `dumbj-syntax` | tree-sitter: language detection, cancellable parsing, incremental reparsing, range-limited highlighting; query files in `queries/`. Also declaration extraction (`FileFacts`, `Symbol`: kind, supertypes, extension receiver, declared type, parameters, expect/actual), reference classification, local scope resolution and a small expression model (`Qualifier`: calls with arguments, member reads, casts, literals, lambdas) for Java and Kotlin | text |
| `dumbj-index` | Dumb navigation, headless: immutable `Index` snapshot (files, symbols, lookup by name, FQN and package), parallel cancellable indexer, SQLite cache of per-file facts, project model (modules and source roots), Go to Declaration (`resolve`), Find Usages (`usages`), fuzzy search for Search Everywhere, Go to Class/File/Symbol and File Structure (`search`, `fuzzy`), and the dumb hierarchy (`hierarchy`: subtypes and supertypes, overriders, super members, gutter marks, callers). Example: `accuracy` (benchmark runner) | syntax, text; `ignore`, `rusqlite` |
| `dumbj-jars` | Library sources, headless: finds source jars in the Gradle and Maven caches and the JDK's `src.zip`, indexes them by class (`JarIndex`), reads entries in place through virtual paths (`lib.jar!/pkg/Class.java`) and locates a class or member in a source | syntax, text; `ignore`, `zip` |
| `dumbj-search` | Find in Files, headless: a parallel, cancellable line search (literal or regex, case, whole word, file mask) that honours `.gitignore` and skips binary and huge files | `ignore`, `regex`, `globset` |
| `dumbj-vcs` | Git, headless: repository discovery, branch, HEAD id and a file's HEAD text through `gix`; changed and untracked files through `git status --porcelain=v2`; line hunks between two texts (`diff`, on `imara-diff`) | `gix`, `imara-diff` |
| `dumbj-review` | GitHub review, headless: `RepoId` from remote URLs, the `gh` token, a blocking GraphQL `Client` (`ureq`, system certificate store): pull-request search and details (checks, commits), review threads, replies, resolve, pending reviews and submit, viewed files, and `post_comments`, which sends drafts in order. Tests run against a mock server on `127.0.0.1`; `examples/pulls` and `examples/threads` check the real API | `ureq`, `rustls`, `serde_json` |
| `dumbj-lsp` | LSP client, headless: blocking and thread-based, no async runtime. `Server` (over any `Read`/`Write` pair or a spawned child, killed when the last handle drops): requests with timeout and `$/cancelRequest` on drop, notifications, the built-in answers to server requests, `Exited` once; `Content-Length` framing; UTF-16 position conversion against `dumbj-text` `Buffer`; `file:` URIs; restart `Backoff`; `preset` (`ServerKind`, `detect` of jdtls/kotlin-lsp from markers, `PATH` and JDK release file without spawning, `initialize_params`); `Session` (supervised server: readiness from progress and `language/status`, full-text document sync, restart with backoff, stop on drop) | `lsp-types`, `dumbj-text` |
| `dumbj-actions` | Action registry, IntelliJ keymap XML loader, action ID map, key dispatcher, `KEYMAP.md` generator | none |
| `dumbj-settings` | Themes as data (palette + role mapping); per-project state (`ProjectState`: recent files, recent locations, bookmarks) and its directory; user settings later | syntax |
| `dumbj-ui` | GPUI front end: workspace window, file tree, tabs, editor view, text input, popups, chooser, Find and Hierarchy tool windows | all of the above |
| `dumbj-app` | The `dumbj` binary: CLI, logging, starts the UI | ui |

Planned, not created yet: `dumbj-platform`.

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
- **Git status.** Opening a project, regaining window focus and every save make the workspace rediscover the repository in the background: the branch goes to the status bar, `statuses` colours the Project tree, and each open editor receives its file's HEAD text. The editor diffs its buffer against that text in the background (200 ms after typing stops) and paints the hunks as gutter change markers; Next/Previous Change and Rollback Lines work on those hunks.
- **Pull requests.** The Pull Requests popup asks `dumbj-review` for the open pull requests of the first remote on a GitHub host (`origin` preferred), signed in with `gh auth token`. Opening one runs `Repository::fetch_pull` (refs under `refs/dumbj/pr/<n>/`) and starts a review of `base...head` that carries the pull request. The workspace then indexes the head commit from git objects (`workspace/head_index.rs`, `dumbj_index::index_revision`): the head side of each diff and the tabs reached from it open from revision paths (`<workdir>!<commit>/<path>`) and resolve against that index. Threads, details and viewed files are read in one background task (`workspace/threads.rs`); each thread becomes a block of rows below its line (`editor/blocks.rs`). Comments are written in a scratch editor (`workspace/compose.rs`), saved as drafts in the project state and sent by `Client::post_comments`; every change on GitHub is followed by reading the threads again.
- **Smart mode.** The first open working-tree file of a language makes `workspace/smart.rs` run `dumbj_lsp::detect` in the background and start a `Session` (jdtls for Java, kotlin-lsp for Kotlin). The session's threads flag state changes, which a 200 ms poll turns into repaints; the status bar shows the state for the active file's language. Open editors are sent to the server in full 300 ms after a change, keyed on the buffer version. Revision tabs, library sources and files outside the project are never sent.
- **Exact navigation.** With a ready server for the active file, Go to Declaration, Type Declaration and Implementation send `dumbj_lsp::goto` on the background executor and start a 300 ms timer. An answer before the timer is shown alone; otherwise the dumb result is shown first and the exact one replaces it when it arrives, provided the caret is still where the action or the dumb jump left it. Targets are converted from UTF-16 against the file's text in the same background task. No server, no answer, or an answer outside the file system leaves the dumb result.
- **Exact hierarchy.** With a ready server, Type Hierarchy and Call Hierarchy prepare the item at the caret in the background (`workspace/hierarchy/exact.rs`): the type tree is the chain of first supertypes down to it, the call tree its root. Rows keep the server's item and expand lazily, one request per row (subtypes, incoming or outgoing calls). The dumb tree is computed at once but shown only if the exact one has not arrived after 300 ms.
- **Diagnostics.** A session keeps the last `publishDiagnostics` per file and drops them all when its server goes away. Its change flag makes the workspace hand every open working file its diagnostics; the editor converts them to byte ranges against its buffer (hints are left out), underlines them in the severity's colour and shows the message at the caret in the status bar. F2 / Shift+F2 move between them. They are read-only: no quick fixes.
- **Semantic tokens and inlay hints.** Once a server is ready and has a file's current text, the workspace asks for the file's semantic tokens and inlay hints in one background task and hands them to the editor with the buffer version they describe; an editor whose text has moved on ignores them. Known token types become `HighlightKind`s laid over tree-sitter's spans when the highlight cache is rebuilt. Inlays are inserts in `DisplayLine`, the same mapping that expands tabs, so the caret, selection, hit testing and highlights stay in buffer terms. Tokens, inlays and diagnostics are shifted by each edit (and dropped where it touches them) until fresh ones arrive.
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

### 2026-11-16: Git status comes from the git CLI, objects from gix
`dumbj-vcs` reads refs and blobs with `gix` (default features off, `sha1` only) and runs `git --no-optional-locks status --porcelain=v2` for the changed-file list. Why: the CLI applies ignore rules, the index and rename detection exactly as the user's git does, and `--no-optional-locks` keeps it from writing to the repository; BRIEF §3 allows the CLI where gix is weaker. `gix` adds about 60 crates; a clean debug build of the workspace took 1 min 47 s with it on the development machine (not measured without).

### 2026-11-16: Revisions other than HEAD are read with the git CLI
`Repository::text_at`, `resolve`, `merge_base`, `changed_files` and `log` run `git --no-optional-locks` (`rev-parse`, `cat-file`, `diff`, `log`); only HEAD's blobs go through `gix`. Why: revision syntax (`HEAD~2`, tags, prefixes), rename detection and log filters behave exactly as the user's git does, and the gix equivalents would need features that are switched off to keep the dependency small.

### 2026-11-16: The diff viewer is two real editors
A diff tab shows two `EditorView`s side by side, the revision on the left (read-only) and the working file on the right, without filler lines, as IDEA does. The workspace computes the hunks in the background from both buffers, gives each editor its `DiffMarks` (line bands, changed words, gap lines) and keeps the scroll positions level by mapping the top line through the hunks. Why: navigation, search, folding and selection work on both sides for free, and the right side stays editable. The right editor is the same entity as the file's own tab, so unsaved text and undo history are shared; the cost is that the two tabs also share a scroll position. Ctrl+D (`Compare.SameVersion`) opens the diff from the Project tree only, because the same shortcut is Duplicate Line in the editor.

### 2026-11-16: DumbJ's own actions live in keymaps that extend the vendored ones
`keymaps/dumbj-linux.xml` ("DumbJ", parent "macOS For All") and `keymaps/dumbj-macos.xml` ("DumbJ macOS", parent IntelliJ's macOS keymap) are the default keymaps. They bind actions IntelliJ does not have, under `DumbJ.*` ids, in the same XML format, so one loader, one id map and one remapping mechanism serve both kinds. A test fails if one of their shortcuts is taken by any action of the parent keymap, mapped or not.

### 2026-11-16: Local review state
A review is a range in git's syntax (`base`, `base..head`, `base...head`); the working tree is the head when none is given. Viewed marks and notes are kept in the project state (`dumbj_settings::Review`), outside the repository, one review per project: starting a review of another range replaces it. A viewed mark stores the blob id of the file's head side (`Repository::content_ids`, which hashes working files without writing objects), so the file is unviewed again once that content changes. Notes are exported by copying Markdown to the clipboard. Why: DumbJ never writes inside the repository (BRIEF §2.5).

### 2026-11-16: Checkout and fetch are the only git writes, done through the CLI
`Repository::checkout` runs `git switch` (with `--track` for a remote-tracking branch that has no local branch yet) and `fetch` runs `git fetch --all --prune` with `GIT_TERMINAL_PROMPT=0`, so it fails rather than waits for a password. Every open file is saved, on the UI thread, before checkout starts, so git sees the user's last edit and refuses rather than overwrites; DumbJ never passes `--force` or stashes. After either, the workspace refreshes the repository, the Project tree, the open editors (from disk), the log and the index.

### 2026-11-16: Annotate is read at HEAD and mapped through the change markers
Blame runs on HEAD's version of the file (`git blame --porcelain HEAD`), not on the buffer, and each buffer line takes the annotation of the HEAD line the change hunks map it to; added and modified lines stay blank. Why: blaming the working file would need the buffer written to disk or fed through `--contents`, and the hunks are already computed for the gutter markers.

### 2026-11-16: GitHub sign-in reuses the gh CLI
M3 takes the GitHub token from `gh auth token` instead of implementing the device flow or storing tokens. Why: the owner already uses `gh`, it handles hosts, scopes and keyring storage, and it keeps credentials out of DumbJ. BRIEF §4.5's own sign-in (tokens, device flow, several accounts) is deferred.


### 2026-11-16: M3 design (GitHub review)
- **API: GitHub GraphQL over blocking `ureq`**, in a new headless crate `dumbj-review`, with `serde_json` for the payloads. This confirms the 2026-11-14 HTTP decision. Why not `gh api`: review threads, pending reviews and viewed state need many calls, a child process per call is slow and hard to cancel, and BRIEF §10's mock GitHub server needs a client whose endpoint can be pointed at `127.0.0.1`. `gh` stays the source of the token only.
- **PR refs are a fetch.** Opening a PR runs `git fetch <remote> +refs/pull/<n>/head:refs/dumbj/pr/<n>/head` and fetches the base commit into `refs/dumbj/pr/<n>/base`. This writes refs and objects under `.git`, which the read-only rule allows as a fetch (BRIEF §2.5, §4.5); nothing in the working tree, the index or the user's branches changes. "Prune review refs" deletes `refs/dumbj/pr/`.
- **A PR is a local review** of `refs/dumbj/pr/<n>/base...refs/dumbj/pr/<n>/head`, plus the PR's number and host in the review state. The M2 review window, diff viewer and viewed marks are reused.
- **The PR head is indexed from git objects, not checked out.** `dumbj-index` builds a second `Index` for the head commit: files that are identical in the working tree reuse the facts of the working-tree snapshot, the rest are read with `git cat-file --batch` and parsed. Its files carry virtual paths (`<workdir>!<commit>/<path>`, in the manner of the jar paths), which open as read-only revision tabs. Why: the 2026-11-14 decision that reviews never touch the working tree stands, and BRIEF §4.5 wants navigation across the whole head. PR checkout as a branch switch is not offered in M3.
- **Inline threads are block rows in the editor's display map.** The fold map that turns buffer lines into display rows also carries blocks: a block sits below a buffer line and takes a whole number of rows, which the editor paints itself (author, wrapped body, state). Why whole rows: scrolling, the scrollbar and diff scroll sync all count rows already.
- **Pending comments live in the project state** next to the local review (`dumbj_settings::Review`), are written before any network call and are removed only when GitHub confirms the submitted review.

### 2026-11-16: The PR list is a popup and M3's shortcuts are chords
Pull requests are listed in a popup with filter tabs (Tab cycles them) and a search field that queries GitHub, not in a tool window as BRIEF §4.5 says. Why: it is one keystroke away from anywhere and the Review tool window is taken by the changed files once a PR is open. The pull request actions are two-stroke chords under Ctrl+Alt+Shift+P (`P` list, `O` open in browser), because nearly every single Ctrl+Alt+Shift letter is bound in one of the vendored keymaps and that prefix is free in all of them.

### 2026-11-16: Review and Pull Requests are separate tool windows
Supersedes the popup above, at the owner's request. The left stripe has a Review button (Alt+0) and a Pull Requests button (Alt+8); Commit and Services, which IntelliJ binds there, do not exist in DumbJ, so `keymaps/dumbj-*.xml` unbinds them. Review shows the changed files of a local review, or a Start Review button. Pull Requests shows the list (filters, search) until a pull request is opened, then its changed files; `‹` or Backspace goes back to the list and Escape in the list returns to the files. There is still one review at a time: opening a pull request replaces a local review and the reverse.

### 2026-11-16: Menus are lists of actions
The menu bar and the editor's context menu (`workspace/menu.rs`) hold nothing but `Action`s: titles and shortcuts come from the registry and the bindings, what can run is `is_enabled` at the moment the menu opens. The bar lists every window action (a test fails when one is missing) and greys out what cannot run; the context menu shows only what can. While a menu is open it takes the keys (arrows, Enter, Escape, through the keymap). In a diff opened from a review the gutter offers a `+` on the hovered line, which runs Add Comment or Add Note. Mouse input cannot be simulated in the headless screenshot setup (no virtual pointer), so the right click, the bar clicks and the `+` are checked by hand only; the menus themselves are checked through their keyboard actions.

### 2026-11-16: Comments are written in a scratch editor and sent through drafts
The comment field is an `EditorView` over a buffer that belongs to no file (`EditorView::scratch`): multi-line editing, undo and Markdown highlighting come for free. Sending stores a `DraftComment` in the project state first, then posts it (`Client::post_comments`: the pending review is started when needed; "send now" publishes the comment in a review of its own, unless a pending review exists, which GitHub would attach it to anyway). A draft is removed only when GitHub confirms; a refused one stays in the diff as "not sent yet" and reopens on click. Enter must insert a line break and every Enter combination with one or two modifiers is bound in the vendored keymaps, so Send is Ctrl+Alt+Shift+Enter.

### 2026-11-16: M4 design (smart mode)
- **`dumbj-lsp` is blocking and thread-based.** One reader thread per server parses `Content-Length` frames and routes responses to waiting requests over channels; writes go through a mutex on the child's stdin. Callers block on a background-executor thread, as the GitHub client does. Why: no second async runtime (BRIEF §8.1). Dropping a request handle sends `$/cancelRequest`.
- **Protocol types come from `lsp-types`** (MIT) instead of hand-written structs: M4 uses some twenty requests and their capabilities, and hand-copies of the specification are where field-name bugs come from.
- **Positions are converted at the boundary only.** `dumbj-lsp` turns `ByteOffset`/`Point` into UTF-16 `Position`s against a line's text and back; nothing else sees UTF-16 (BRIEF §8.1).
- **A server starts only when its binary is on `PATH`, a JDK is found and the project has its markers** (`jdtls`: Maven or Gradle build files; `kotlin-lsp`: Gradle build files), and can be switched off per project in the project state. Its workspace data goes under `$XDG_CACHE_HOME/dumbj/lsp/<project-id>/`; jdtls is told not to generate metadata files at the project root, so nothing is written inside the repository.
- **Dumb stays the engine of record.** Every navigation action computes the dumb result as before; with a ready server it also asks for the exact one off the UI thread. An answer within 300 ms is used alone; after 300 ms the dumb result is shown and replaced when the exact one arrives (BRIEF §4.3.1). Revision paths, library files and scratch buffers never go to a server. A crashed or hung server is restarted with backoff (1 s doubling to 60 s, five tries) and the project is `dumb` meanwhile.
- **Documents are synchronised in full.** `didOpen` for each open working-tree editor, `didChange` with the whole text 300 ms after typing stops, `didClose` with the tab. Why: both servers accept full sync, the files are source files of ordinary size, and incremental sync buys nothing a reader notices.

### 2026-11-17: What the live servers taught (M4)
- **jdtls gets the newest JDK found** in `JAVA_HOME`, `PATH`, sdkman's candidates and `/usr/lib/jvm`. Why: a snapshot required JDK 25 while documenting 21 as its minimum; the newest JDK satisfies both. kotlin-lsp keeps the user's `JAVA_HOME`, since it bundles its own runtime and the JDK is for the project.
- **jdtls's metadata redirection is a JVM argument.** `-Djava.import.generatesMetadataFilesAtProjectRoot=false` goes through `--jvm-arg`; as an initialization option alone, jdtls still wrote `.project`, `.classpath` and `.settings` into the project.
- **Build-tool imports still write their own caches into the project:** `target/classes` (m2e, for Maven projects) and `.gradle/` (Gradle's project cache, when kotlin-lsp imports). These are what the user's own builds create and are normally ignored by git, but they break BRIEF §2.5's "never write inside the repo" for smart mode. Smart mode can be switched off per project (Ctrl+Alt+Shift+P, M). The owner accepted this on 2026-11-17: they are folders the user's own builds create anyway.
- **Servers are told about files changed on disk** (`workspace/didChangeWatchedFiles`): whenever the index is rebuilt or a server process connects, the stamps (modification time, length) of source and build files are compared with the list last sent to that server, kept in `<lsp data>/<server>.files`. Why: without it jdtls never saw files created by a branch switch, even after a restart. A missing list means the server has just imported everything, so nothing is sent.
- **Types the protocol crate gets wrong are our own.** lsp-types 0.97 declares `TypeHierarchyItem.tags` as one tag; `dumbj_lsp::TypeItem` and its three requests replace it. Other servers' answers are decoded with lsp-types until a live check shows otherwise.
- **Non-LSP output on stdout is skipped** between messages, since jdtls prints start-up errors there.

