# Project brief: DumbJ — a code-reading and code-review editor inspired by IntelliJ IDEA

> Name: **DumbJ**, a joke on IntelliJ. It also fits IntelliJ's "dumb mode" (navigation that works before indexing finishes), which is how this editor works without a language server.

## 1. What you are building

A fast, native desktop **text editor built for reading, navigating and reviewing code**. Its UX, visuals and keyboard model are modelled on JetBrains IntelliJ IDEA.

It is **not an IDE**. Leave these out entirely: build systems, run/debug configurations, a debugger, project SDK setup, refactorings that rewrite code across files, code generation, and a plugin marketplace. Editing is supported and should feel good, but it comes second. The product exists to make it fast and pleasant to understand an unfamiliar codebase and to review changes to it.

**Target platforms:** Fedora Linux (Workstation, GNOME, Wayland first, X11 second) is the only required target for v1. The architecture must not block **macOS** and **other Linux distributions** (Ubuntu/Debian, Arch; Flatpak and AppImage packaging) later. Windows is not planned, but don't make choices that rule it out without a good reason.

## 2. Guiding principles

1. **Navigation is the product.** Every symbol should be one keystroke from its definition, usages, implementations and history.
2. **Keyboard first, IntelliJ muscle memory.** A long-time IDEA user should feel at home straight away. On Linux the default keymap is **"macOS For All"** (IntelliJ's macOS keymap with ⌘ rebound to non-⌘ keys). On macOS it is IntelliJ's standard macOS keymap. Muscle memory carries over between the two platforms (Section 6).
3. **Fast on big repos.** Opening a 1M-line monorepo must not freeze the UI. All indexing, parsing, git and LSP work happens off the UI thread and can be cancelled.
4. **Dumb first, smart when possible.** DumbJ must be useful on any checkout instantly, with nothing installed and no build: tree-sitter powers highlighting, structure and heuristic navigation for every language. Language servers are an **optional precision layer**, started only when one is installed and the project looks ready for it (e.g. `settings.gradle(.kts)`/`pom.xml` for jdtls and kotlin-lsp). The UI always shows which mode a result came from (Section 4.3.1).
5. **Read-mostly, never lose data.** Opening, navigating and reviewing never modify files on disk. Edits autosave like IDEA, can be undone, and are saved atomically. *(Amended 2026-11-14; see ARCHITECTURE Decisions.)*
6. **Portable core, thin platform layer.** All OS-specific code sits behind small interfaces (file watching, clipboard, keychain, fonts, menus, packaging).

## 3. Technology (recommended default — confirm or override in ARCHITECTURE.md)

The stack is decided: **Rust + GPUI, native desktop**. Before writing feature code, do a short, focused **validation spike** proving GPUI meets the criteria under **UI** below on Fedora. Record the result in the **Decisions** section of `docs/ARCHITECTURE.md`. Use these components:

- **Language:** Rust (stable).
- **Text model:** rope (`ropey` or equivalent) with incremental edits.
- **Parsing / highlighting / structure:** `tree-sitter` with per-language grammars and query files (highlights, locals, tags, folds, indents).
- **Semantic navigation:** a built-in **LSP client** (`lsp-types` + your own JSON-RPC transport). Language servers are discovered on `PATH` and configured per language (v1: **jdtls** for Java and JetBrains' **kotlin-lsp** for Kotlin; others later: rust-analyzer, gopls, typescript-language-server, basedpyright, clangd…). Never bundle them.
- **Fallback symbol index:** a tree-sitter "tags" index stored on disk (SQLite or a similar embedded store) so Go-to-Symbol and Find Usages still work without a language server.
- **Text search:** ripgrep's libraries (`grep-searcher`, `ignore`) so `.gitignore` is respected.
- **Git:** `gitoxide` (`gix`) where it supports the operation, falling back to the `git` CLI for anything it doesn't.
- **UI: native, GPU-rendered, using GPUI** (Zed's UI framework, Apache-2.0, Linux + macOS). No web view, no Electron, no Tauri.
  - **Validation spike must prove, on Fedora GNOME:** smooth scrolling of a 100k-line file; correct rendering with Wayland fractional scaling (125%/150%) and on X11; IME input (e.g. IBus) works; the keymap layer can capture double-Shift, two-stroke chords and every modifier combination macOS For All needs; and a popup plus a docked panel can be built to match IDEA's look.
  - **Pinning:** GPUI's API changes and it is published from the Zed repo, so pin it to a specific git revision (or crates.io version if one is available) and upgrade deliberately, never on the fly. Wrap GPUI in the `ui` crate only; no other crate may depend on it.
  - **Widgets:** build DumbJ's own small component library on top of GPUI (tree view, tabs, splitter, tool window, popup/list chooser, diff view, gutter). Zed's code and `gpui-component` can be used for reference. DumbJ is GPL-3.0-or-later, so small adapted pieces of Zed code are allowed with attribution; never vendor Zed crates wholesale. *(Amended 2026-11-14.)*
  - **Plan B, only if the spike finds a real blocker:** Tauri 2 + CodeMirror 6, with the same Rust core crates unchanged. Stop and ask me before switching.
- **Code-host integration:** GitHub REST/GraphQL first, then GitLab, behind a `ReviewProvider` trait. Store tokens in the OS keychain (libsecret on Linux, Keychain on macOS); never write them in plain text.
- **Config:** human-editable TOML in `$XDG_CONFIG_HOME/dumbj/` (`~/Library/Application Support/dumbj/` on macOS), reloaded live.

## 4. Features

### 4.1 Workspace and window layout (IDEA look and feel)

- Open a folder or a git repository as a project. Remember recent projects, open tabs, splits, tool-window layout and caret positions per project.
- IDEA-style layout: editor area with tabs and splits (vertical/horizontal, drag tabs between splits), **tool windows** docked left/right/bottom with stripe buttons, a navigation bar (breadcrumb of path → symbol), and a status bar (branch, line:col, encoding, line endings, indent, LSP/indexing status).
- Tool windows for v1: **Project** (file tree with git status colours), **Structure**, **Find**, **Git** (log and branches), **Review** (PRs and comments), **Bookmarks**, **Problems** (LSP diagnostics, read-only), **Hierarchy**.
- Distraction-free and "Hide all tool windows" modes.
- Themes: the **default theme is Catppuccin Mocha** (https://catppuccin.com, MIT licence). Use the official palette from `catppuccin/palette` exactly; don't approximate the hex values. For syntax colours, follow Catppuccin's style guide and its official editor ports (e.g. `catppuccin/jetbrains`, `catppuccin/zed`) so code looks the way Catppuccin users expect. Map the palette onto every UI surface too: tool windows, popups, gutters, diff colours (added = green, removed = red, modified = blue/yellow on the base colour), review-comment threads, blame age gradient, search hits and usage highlights (read vs write). Check text contrast in the diff view and in popups. Also ship **Catppuccin Latte** as the light theme; follow the system dark/light setting by default, choosing between Mocha and Latte. Frappé and Macchiato are nice extras. Themes are data files (one palette file plus a mapping from semantic roles to palette colours), so other themes such as a Darcula-style one can be added later without code changes. Include the Catppuccin licence and credit in `THIRD_PARTY_LICENSES`.
- Fonts: JetBrains Mono as the default (it is OFL-licensed, so it can be bundled), with ligatures on/off, line-height and size settings.
- Don't copy JetBrains icons, logos or other proprietary assets. Recreate the *feel* with original or openly licensed icons (e.g. Lucide or Codicons, checking their licences).

### 4.2 Editor surface

- Fast rendering of large files (≥ 100k lines scroll smoothly at 60 fps or better). Huge or minified files (> ~20 MB, or very long lines) open in a degraded mode that turns heavy features off.
- Syntax highlighting comes from tree-sitter and is complete without a language server: keywords, literals, comments, functions, types, attributes, macros and so on, in every language. Add **scope-aware highlighting** from tree-sitter `locals` queries (parameters, locals and their usages coloured consistently within a file) so the dumb mode gets as close to IDEA's colouring as it can. When a language server is running, its **semantic tokens** (fields vs locals vs statics, mutable variables, deprecated strikethrough, unresolved references) are layered on top without flicker.
- Gutter: line numbers, fold markers, git change markers (added/modified/deleted; click to see the diff inline and revert), bookmark icons, review-comment markers, and "implements/overrides" gutter icons that navigate on click (LSP-backed).
- **Inlay hints** (parameter names, inferred types) when the language server provides them; can be toggled.
- **Inline blame** for the caret line, plus a full **Annotate (git blame)** gutter that can be toggled, with age-based colouring and "annotate previous revision".
- Sticky lines (the enclosing scopes pinned to the top while scrolling).
- Indent guides, matching-bracket highlighting, rainbow brackets (optional), whitespace rendering, right-margin guide, soft wrap per file type.
- Code folding (tree-sitter folds), including "fold all comments / imports".
- **Highlight usages of the symbol at the caret** within the file, with distinct read/write colours.
- Minimap/scrollbar marks for errors, search hits, usages, changes and review comments.
- Hover popups for quick documentation and definition preview.
- Editing basics: multi-caret, column selection, **Extend/Shrink selection** (syntax-aware via tree-sitter), comment toggle, move line/statement, duplicate/delete line, undo/redo with history grouping, auto-indent, EditorConfig support, and file encodings and line-ending detection. No autocomplete is needed in v1; LSP completion can be a later, opt-in feature.

### 4.3 Navigation (the heart of the product)

All of these are popups in the IDEA style: fuzzy matching with CamelHump / initials matching, preview, open-in-split, and navigating straight from the keyboard.

- **Search Everywhere** (double-Shift), with tabs for All / Files / Symbols / Actions / Text.
- Go to File, Go to Class/Type, Go to Symbol, Go to Line:Column, **Find Action**.
- **Go to Declaration/Definition**, Go to Type Declaration, Go to Implementation(s), Go to Super Method, Go to Test (by naming convention, configurable).
- **Find Usages** (a tool window grouped by file/directory, with read/write filters and preview) and **Show Usages** (an inline popup).
- **Quick Definition** (peek) and **Quick Documentation** popups.
- **File Structure** popup (filterable outline of the current file) and the Structure tool window.
- **Call Hierarchy** and **Type Hierarchy** (LSP when available) in the Hierarchy tool window.
- **Navigate Back / Forward** across files (a proper navigation history like IDEA's, not just the last edit).
- **Recent Files**, **Recent Locations** (with code snippets), **Last Edit Location**, and a **Switcher** (Ctrl+Tab).
- **Find in Files / Replace in Files**: regex, case, whole-word, file mask, scope (project, directory, changed files, PR files), results with preview, and keyboard navigation through hits.
- **Bookmarks**: anonymous and mnemonic (0–9, A–Z), with descriptions and a Bookmarks tool window. Stored per project.
- **Copy Reference** / Copy Path / Copy permalink (a GitHub/GitLab URL to the line or range at the current commit).
- Navigation bar (Alt+Home) to jump through the folder → file → symbol hierarchy.

### 4.3.1 Dumb navigation (tree-sitter) and smart navigation (LSP)

The dumb engine is the main engine, not a fallback. Make it as good as it can be:

- **Scope-aware resolution** within a file using `locals` queries: a local variable or parameter always jumps to its own definition, never to a symbol elsewhere with the same name.
- **Import following:** Java and Kotlin packages, imports (including wildcard, static and aliased `import … as …`) and same-package visibility in v1 (details in Section 5). Other languages later (Rust `use`/`mod`, Go packages, TS/JS imports with `tsconfig.json` aliases, Python imports, C/C++ `#include`). Resolve the import to a file, then to a symbol in that file.
- **Ranking** when there are several candidates: same file > imported file > same directory/package > same module/crate > rest of the repo. Kind matters too (a call site prefers functions/methods, a type position prefers types). If one candidate is clearly best, jump to it; otherwise show IDEA's chooser popup, sorted by rank.
- **Member access heuristics:** for `x.foo()`, prefer `foo` defined in types whose names appear near `x`'s declaration or in the enclosing class/impl.
- **Find Usages (dumb):** an identifier search through the index combined with tree-sitter (excluding comments and strings by default, with a toggle), grouped and ranked like the smart version.
- **Implementations / hierarchy (dumb):** syntactic `implements`/`extends`/`impl Trait for`/interface embedding, from the tags index.
- **Mode labelling:** every navigation result, popup and Find Usages list shows whether it is **exact** (LSP) or a **best guess** (dumb). Use a subtle badge and a status-bar indicator such as "dumb" / "smart" / "smart (indexing…)", in the spirit of IntelliJ's dumb mode. If an LSP request takes longer than ~300 ms, show the dumb result immediately and replace it when the exact result arrives.
- **Language-server lifecycle:** auto-start only when the server binary is on `PATH` and its project markers are present. Can be turned off per project. On a branch switch (e.g. checking out a PR), keep the dumb mode working while the server re-indexes.
- **Measure quality:** keep a fixture corpus of real open-source JVM repos (e.g. a multi-module Spring project, a coroutine-heavy Kotlin library, an Android app, a mixed Java/Kotlin codebase) with expected Go-to-Declaration targets. Report dumb-mode accuracy per language in CI so improvements can be tracked.

### 4.4 Version control and local review

- Git log tool window: graph, filters (branch, author, path, date, text), commit details, and the files changed by each commit.
- Branch popup: view and switch branches, compare branch with the current one, compare with the merge base.
- **Diff viewer** (central to the product):
  - Side-by-side and unified modes, with word/character-level highlighting.
  - Syntax highlighting **and full navigation work inside diffs**: Go to Declaration, Find Usages and the rest all work from either side.
  - Ignore whitespace / line-ending options, collapse unchanged regions, and sync scrolling.
  - Next/previous difference, and next/previous file in a multi-file diff.
  - Diff of: working tree vs HEAD, any two commits, branch vs branch, branch vs merge base, staged vs unstaged, and any file vs clipboard.
- **Local review mode:** pick a base and a head (branch, commit or range) and get a "changes" tree on the left and the diff on the right. Track **viewed/unviewed** per file (reset automatically when a file changes again), and write local review notes on lines or ranges (exportable as Markdown).
- File history for one file and for a selection ("Show History for Selection").
- Read-only by default; the only git operations that change things are checkout/switch and fetch. No commit, rebase or push UI in v1.

### 4.5 Remote code review (GitHub first, GitLab second)

- Sign in with a personal access token or the device flow. Several accounts and hosts are allowed (GitHub Enterprise, self-hosted GitLab).
- **Review tool window:** a list of PRs/MRs (assigned to me, review requested, authored, all) with filters and search.
- Opening a PR fetches its refs into `refs/dumbj/pr/<n>/{head,base}`, reads the revisions straight from git objects (no checkout), and shows the local review mode above, with:
  - Existing review threads rendered **inline in the diff** (folded and expandable), with resolved/outdated state.
  - Add a single comment or start a **pending review**. Comment on a line or a multi-line range, reply, resolve/unresolve, and insert suggested changes using GitHub's ```suggestion``` blocks.
  - Submit a review as Comment / Approve / Request changes, with a summary.
  - Per-file "viewed" state synced with GitHub's viewed flag.
  - PR description and timeline (Markdown rendered), CI/check status, and the list of commits, with the option to review commit by commit.
- Navigation (go to definition, usages, hierarchy) must work across the **whole checked-out head revision**, not only the changed files. That is the main advantage over reviewing in a browser.
- Pending comments survive a restart and a lost network connection (stored locally until submitted).

### 4.6 Settings and keymap

- A settings UI (searchable, IDEA-style) backed by the TOML files.
- **Keymap editor:** search actions by name or by pressing a shortcut, find conflicts, two-stroke shortcuts (e.g. `Ctrl+K, Ctrl+C`), and import/export.
- Built-in keymaps: **macOS For All** as the default on Linux, **IntelliJ macOS** as the default on macOS, and **IntelliJ Default (GNOME/XWin)** as an option on Linux. VS Code and Emacs-ish keymaps can come later.
- Keymaps are data files using an **IntelliJ-compatible keymap XML format** (`<keymap name parent>` / `<action id>` / `<keyboard-shortcut first-keystroke second-keystroke>` / `<mouse-shortcut>`), with `parent` inheritance resolved on load. Users can import a keymap exported from their own IDEA install.
- Every command is a named **action** in one registry (id, name, default shortcuts per platform, context/when-clause). Menus, Find Action, the keymap and the command palette all read from this registry.

## 5. Language support: Java and Kotlin first

v1 is **built for Java and Kotlin codebases**. These two get IntelliJ-grade attention. Everything else gets good-enough highlighting and structure until a later milestone.

### 5.1 Tier 1: Java and Kotlin (full dumb navigation + smart mode)

- **Grammars:** `tree-sitter-java` (mature). For Kotlin, evaluate the available tree-sitter-kotlin grammars early in M1 against a corpus of real Kotlin (coroutines, DSLs and builders, annotations, string templates, `when`, contracts, context receivers) and record the choice in ARCHITECTURE.md. Fix or work around grammar gaps with golden tests; don't silently accept broken parses.
- **Project model without running a build tool:** parse `settings.gradle(.kts)`, `build.gradle(.kts)` and `pom.xml` (including multi-module/aggregator POMs) to find modules and source roots (`src/main/java`, `src/main/kotlin`, `src/test/…`, custom `sourceSets`, Android `src/<variant>/`). Never invoke Gradle or Maven. Fall back to detecting source roots from `package` declarations when the build files are too dynamic to read.
- **Resolution rules:**
  - Packages, imports (explicit, wildcard, static, Kotlin `import … as …`), same-package visibility and implicit imports (`java.lang.*`, `kotlin.*`, `kotlin.collections.*` and so on).
  - Nested and inner classes, and qualified names.
  - Inheritance: resolve members through `extends`/`implements`/`:` chains found in the index.
- **Java ↔ Kotlin interop in dumb mode:**
  - Kotlin properties ↔ Java `getFoo()`/`isFoo()`/`setFoo()`.
  - Top-level Kotlin functions ↔ Java `FileNameKt.fn()`, and `@file:JvmName`.
  - `companion object` members ↔ `Foo.Companion.x()`, plus `@JvmStatic`/`@JvmField`/`@JvmName`/`@JvmOverloads`.
  - Kotlin `object` singletons ↔ `INSTANCE`.
  - Find Usages crosses the language boundary in both directions.
- **Kotlin's harder cases:** extension functions and properties (match on the receiver type where it can be inferred cheaply, otherwise rank candidates), implicit receivers (`this`, scope functions `let`/`apply`/`with`/`run`/`also`, `it`), type aliases, delegated properties, data class generated members (`copy`, `componentN`), sealed hierarchies and `expect`/`actual`. When dumb mode can't be sure, show a ranked chooser labelled **best guess**. Never jump confidently to the wrong target.
- **Hierarchy is core, not extra:** Go to Implementation(s), Go to Super Method, overriding/implementing gutter icons, Type Hierarchy and a basic dumb Call Hierarchy for Java and Kotlin are all in M1.
- **Library sources:** when a symbol resolves to a dependency, find its source jar in `~/.gradle/caches/modules-2/` or `~/.m2/repository/` and open the source **read-only** inside the jar (no extraction to the project). If no source jar exists, show a stub outline built from the class signature if cheap, otherwise a clear "no sources available" message. Never run a decompiler.
- **Smart mode (M4):** **jdtls** for Java and JetBrains' **kotlin-lsp** for Kotlin. Note that kotlin-lsp is currently pre-alpha and focused on JVM-only Gradle projects. Both need a JDK and a project import, so they are always optional. Detect a missing JDK or an unsupported project type and stay in dumb mode with a clear status message.
- **Java/Kotlin-flavoured UI details:** IDEA-style structure icons (class, interface, enum, record, object, data class, sealed, annotation, function, property, field), visibility markers, a test/main source distinction in the Project view, and Go to Test using the `FooTest`/`FooTests`/`FooSpec`/`FooIT` conventions across `src/test`.

### 5.2 Tier 1 support files (good highlighting and structure, light navigation)

Gradle Kotlin DSL (`.gradle.kts`, via the Kotlin grammar), Groovy (`.gradle`), Maven `pom.xml` and XML, `.properties`, YAML (Spring `application.yml`), JSON, TOML (`libs.versions.toml` version catalogs, with navigation from `libs.foo.bar` in build files to the catalog entry), SQL, Markdown, Shell and Dockerfile.

### 5.3 Tier 2: everything else (later)

Rust, Go, TypeScript/JavaScript, Python, C/C++, C#, Ruby, PHP, Swift, HTML and CSS. Until their milestone they get only highlighting, folding, File Structure and symbol-index navigation. Adding a language must still mean adding a grammar plus query files plus an optional LSP preset, with no core code changes. Keep the language-specific resolution code behind a `LanguageResolver` trait so Tier 2 languages slot in later.

## 6. Default keymap: "macOS For All" on Linux, IntelliJ macOS on macOS

### 6.1 Source of truth

The Linux default keymap is the **"macOS For All"** JetBrains keymap plugin:

- Marketplace: https://plugins.jetbrains.com/plugin/13968-macos-for-all
- Source (Apache-2.0): https://github.com/samvtran/jetbrains-macos-keybindings-for-all

It takes IntelliJ's macOS keymap and rebinds it so it never uses the ⌘ key. Mostly ⌘ → Ctrl and ⌘+Alt → Ctrl+Alt. Tool windows are ⌘1…⌘0 → Alt+1…Alt+0. A few bindings move to avoid clashes (e.g. Find Next ⌘G → Ctrl+Alt+G, Find Previous ⌘⇧G → Alt+Shift+G). The macOS Emacs-style Control bindings (Ctrl+N/P/A/E…) are dropped in favour of the arrow keys and Home/End.

**Don't hand-transcribe shortcuts.** Do this:

1. Vendor the plugin's keymap XML into `keymaps/macos-for-all.xml`, along with its licence and a NOTICE/attribution entry. Also vendor its parent keymap(s) from `JetBrains/intellij-community` (`platform/platform-resources/src/keymaps/`, e.g. `$default.xml` and the macOS keymap; also Apache-2.0). The plugin only defines what differs from its parent, so the parent is needed to get the full set.
2. Write a loader that resolves `parent` inheritance and maps **IntelliJ action IDs → DumbJ action IDs** through one explicit table (`keymaps/action-id-map.toml`). Unmapped IntelliJ actions are ignored but listed in a debug report.
3. Use the same loader for IntelliJ's own macOS keymap (the default on macOS) and for keymaps users import.
4. Generate `docs/KEYMAP.md` from the resolved keymaps (action → Linux shortcut → macOS shortcut) and check it in. CI fails if the file is out of date.
5. Add a test that checks a few known bindings so that regressions are caught: Search Everywhere = double Shift, Go to Declaration = Ctrl+B, Find Usages = Alt+F7, File Structure = Ctrl+F12, Project tool window = Alt+1, Find Next = Ctrl+Alt+G. **If the vendored XML disagrees with any of these, the XML is right.** Fix the test and tell me.

### 6.2 Actions that must exist and be bound (IntelliJ action IDs in brackets)

Every action below must be implemented and must take its shortcut from the resolved keymap:

- **Search & go to:** Search Everywhere [`SearchEverywhere`], Find Action [`GotoAction`], Go to Class [`GotoClass`], Go to File [`GotoFile`], Go to Symbol [`GotoSymbol`], Go to Line [`GotoLine`], Recent Files [`RecentFiles`], Recent Locations [`RecentLocations`], Switcher [`Switcher`].
- **Code navigation:** Go to Declaration [`GotoDeclaration`], Go to Type Declaration [`GotoTypeDeclaration`], Go to Implementation [`GotoImplementation`], Go to Super Method [`GotoSuperMethod`], Go to Test [`GotoTest`], Find Usages [`FindUsages`], Show Usages [`ShowUsages`], Highlight Usages in File [`HighlightUsagesInFile`], Quick Definition [`QuickImplementations`], Quick Documentation [`QuickJavaDoc`], Parameter Info [`ParameterInfo`], File Structure [`FileStructurePopup`], Type Hierarchy [`TypeHierarchy`], Call Hierarchy [`CallHierarchy`], Back / Forward [`Back`, `Forward`], Last Edit Location [`JumpToLastChange`], Navigation Bar [`ShowNavBar`], Select In [`SelectIn`], Next / Previous Error [`GotoNextError`, `GotoPreviousError`], Jump to Source [`EditSource`].
- **Find:** Find / Replace [`Find`, `Replace`], Find Next / Previous [`FindNext`, `FindPrevious`], Find in Files / Replace in Files [`FindInPath`, `ReplaceInPath`].
- **Bookmarks:** [`ToggleBookmark`, `ToggleBookmarkWithMnemonic`, `ShowBookmarks`, `GotoBookmark0`…`GotoBookmark9`].
- **Selection & editing:** Extend / Shrink Selection [`EditorSelectWord`, `EditorUnSelectWord`], Add / Remove Next Occurrence [`SelectNextOccurrence`, `UnselectPreviousOccurrence`], Select All Occurrences [`SelectAllOccurrences`], Clone Caret Above / Below [`EditorCloneCaretAbove`, `EditorCloneCaretBelow`], Column Mode [`EditorToggleColumnMode`], Line / Block Comment [`CommentByLineComment`, `CommentByBlockComment`], Duplicate / Delete Line [`EditorDuplicate`, `EditorDeleteLine`], Move Statement / Line [`MoveStatementUp`, `MoveStatementDown`, `MoveLineUp`, `MoveLineDown`], Folding [`ExpandRegion`, `CollapseRegion`, `ExpandAllRegions`, `CollapseAllRegions`].
- **Windows & tabs:** Close Tab [`CloseContent`], Previous / Next Tab [`PreviousTab`, `NextTab`], tool windows [`ActivateProjectToolWindow`, `ActivateFindToolWindow`, `ActivateStructureToolWindow`, `ActivateVersionControlToolWindow`, `ActivateBookmarksToolWindow`, `ActivateProblemsViewToolWindow`], Hide All Tool Windows [`HideAllWindows`], Jump to Last Tool Window [`JumpToLastWindow`], Settings [`ShowSettings`].
- **VCS & diff:** Next / Previous Change Marker [`VcsShowNextChangeMarker`, `VcsShowPrevChangeMarker`], Rollback Lines [`Vcs.RollbackChangedLines`], Show Diff [`Compare.SameVersion` / `ShowDiff`, whichever the keymap uses], Next / Previous Difference [`NextDiff`, `PreviousDiff`], VCS Quick List [`Vcs.QuickListPopupAction`], Recent Changes [`RecentChanges`], Annotate [`Annotate`], Copy Reference [`CopyReference`].

Some action IDs above may be slightly off or may have been renamed in newer IntelliJ versions. Use the IDs that actually appear in the vendored XML and record any differences in `action-id-map.toml`.

**Review-specific actions** (DumbJ-only, not in the IntelliJ keymap): Add Review Comment, Start/Submit Review, Next/Previous Comment Thread, Resolve Thread, Toggle File Viewed, Next Unviewed File, Open PR in Browser, Copy Permalink. Pick defaults in the spirit of macOS For All (Ctrl-based, no Super). They must not clash with any binding in the resolved keymap (add a test for this), can be remapped, and must appear in Find Action.

### 6.3 Fedora/GNOME conflicts

A macOS-style keymap on GNOME has some known clashes. Examples: Ctrl+Alt+Left/Right (Navigate Back/Forward in some IntelliJ keymaps) can clash with workspace switching depending on GNOME settings; Alt+F7/Alt+F8 (move/resize window on older or custom GNOME setups); Alt+` (GNOME's "switch windows of an application"); and Ctrl+Alt+Shift combinations used by extensions. On first launch, read the active GNOME keybindings (`gsettings get org.gnome.desktop.wm.keybindings …`, `org.gnome.mutter.*`, `org.gnome.shell.keybindings`), compare them with the resolved keymap, and show a one-time dialog. The dialog lists the conflicts and offers to (a) show the `gsettings` commands that free them, or (b) rebind the clashing actions in DumbJ. Never change system settings without permission. Use the same mechanism later for macOS system shortcuts and for KDE.

## 7. Non-functional requirements

- **Performance budgets** (measure them in CI with benchmarks):
  - Cold start to a usable window: < 500 ms.
  - Opening a 10k-line file: < 50 ms. Typing latency: < 16 ms at p99.
  - Search Everywhere on a 200k-file repo: first results in < 100 ms after the initial index.
  - Initial tags index of a 1M-LOC repo in the background without blocking the UI; incremental re-index on change in < 1 s.
- **Memory:** < 500 MB for a 1M-LOC repo, not counting language servers.
- **Robustness:** a language server crashing or hanging never takes down the app (restart it with backoff and show its status). Corrupt index → rebuild it on its own.
- **Accessibility:** full keyboard operation, screen-reader labels where the UI toolkit allows, respect for system font scaling and HiDPI/fractional scaling.
- **Privacy:** no telemetry. Network access only to the code hosts the user configures.
- **Licensing:** DumbJ is GPL-3.0-or-later. All dependencies must have GPL-3-compatible licences (no GPL-2.0-only). Generate a `THIRD_PARTY_LICENSES` file (see Section 8.1).

## 8. Architecture expectations

- Crates/modules with clear boundaries, for example: `core-text` (rope, edits, undo), `syntax` (tree-sitter), `index` (tags/symbol/file index), `lsp`, `search`, `vcs`, `review` (provider trait + github + gitlab), `actions` (registry + keymap), `settings`, `platform` (OS abstractions), and `ui`.
- `ui` depends on the others; nothing depends on `ui`. Core crates must be testable headless.
- One job system for background work, with cancellation and priorities (the visible file comes first).
- Write `docs/ARCHITECTURE.md` early and keep it short and current: the crate map, data flow, threading model, and a **Decisions** section with one dated entry (decision + why, 2–5 lines) per significant choice. No separate ADR folder.

### 8.1 Rust engineering standards

**Workspace layout**
- One Cargo workspace: crates live in `crates/dumbj-<name>` (e.g. `dumbj-text`, `dumbj-syntax`, `dumbj-index`), with the binary in `crates/dumbj-app`.
- Use `[workspace.package]` and `[workspace.dependencies]` so versions are declared once, and `[workspace.lints]` so every crate shares the same lint settings.
- Commit `Cargo.lock`, since this is an application.
- Pin the toolchain in `rust-toolchain.toml` (stable, with `rustfmt` and `clippy`), and set `rust-version` (the minimum supported Rust version) in the workspace.

**Lints and style**
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` in CI, with `clippy::pedantic` warnings enabled at workspace level (allow the noisiest ones explicitly, with a comment).
- Deny `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic` and `clippy::dbg_macro` in library crates. Allow them in tests and in clearly impossible cases with `#[expect(..., reason = "...")]`.
- Use `#![forbid(unsafe_code)]` in every crate except the few that truly need unsafe code (e.g. platform or FFI). In those, every `unsafe` block has a `// SAFETY:` comment.

**Errors and logging**
- Library crates define their own error types with `thiserror`. The app crate uses `anyhow` (or `eyre`) at the edges and adds context when it propagates errors.
- Errors shown to the user are clear messages, never `Debug` dumps.
- Use `tracing` for logging and spans (no `println!`/`eprintln!`), with a log file in the XDG state directory and a debug log panel in the app.

**Concurrency**
- Use the UI framework's executor for UI-related async work. Don't add a second general-purpose async runtime. If a dependency requires `tokio` (e.g. HTTP for code review), confine it to the crate that needs it, behind a narrow interface.
- Make CPU-heavy work (parsing, indexing, diffing) cancellable on a worker pool (`rayon` or the job system), checking cancellation regularly.
- Prefer passing messages over shared mutable state. Never hold a lock across an `.await` or a long computation.

**API design**
- Keep public APIs small; use `pub(crate)` by default.
- Use newtypes for IDs and offsets (`ByteOffset`, `LineCol`, `BufferId`) so byte, character and UTF-16 positions can't be mixed up. LSP uses UTF-16 positions; convert at the boundary only.
- Avoid deep generic hierarchies and heavy macros that slow down compiling. Keep the dependency tree lean and justify each new dependency in its PR or commit message.

**Testing**
- Run tests with `cargo nextest`.
- Use `insta` snapshot tests for tree-sitter queries, highlighting, navigation results and diff output.
- Use `proptest` for the rope, edits, undo and diff algorithms.
- Fuzz the parsers of external input with `cargo fuzz`: git output, LSP messages and the keymap XML.
- Benchmark with `criterion` or `divan` against the performance budgets in Section 7. CI flags regressions.

**Supply chain and licences**
- Run `cargo deny` in CI (licence allow-list, security advisories, banned or duplicate crates).
- Generate `THIRD_PARTY_LICENSES` with `cargo about`, adding the vendored non-Rust assets: Catppuccin, the keymaps and the fonts.

**Builds**
- Release profile: `lto = "thin"`, `codegen-units = 1`, `strip = "debuginfo"`, with split debug info kept for crash reports.
- Make development builds fast: optimise dependencies at `opt-level = 1`, and document using the `mold` linker on Fedora.
- CI caches with `Swatinem/rust-cache`.

## 9. Delivery plan — work in milestones, stop for review after each

Do one milestone at a time. At the end of each, give me: a summary, how to run it, what's tested, known gaps, and screenshots of the UI. **Wait for my go-ahead before starting the next one.**

1. **M0 — GPUI validation and skeleton:** GPUI spike result recorded in ARCHITECTURE.md, repo layout, `CLAUDE.md` + `docs/FEATURES.md` + `docs/ARCHITECTURE.md` in place, Cargo workspace and CI per Section 8.1 (fmt, clippy, nextest, cargo-deny, build on Fedora), a window that opens a folder, file tree, tabs, a basic editor with tree-sitter highlighting for Java, Kotlin and one other language (e.g. YAML), Catppuccin Mocha theme, action registry + IntelliJ-XML keymap loader with macOS For All vendored and ~10 actions bound.
2. **M1 — Dumb navigation for Java and Kotlin** *(delivered as M1a, the navigation core, then M1b, the rest, with a review stop after each; split in ARCHITECTURE Decisions 2026-11-14)*: Kotlin grammar evaluation, the Gradle/Maven project model, tags index, everything in Sections 4.3.1 and 5.1 except smart mode (scope resolution, import following, Java↔Kotlin interop, ranking, dumb Find Usages, implementations, overriding gutter icons, type and call hierarchy, library source jars), Tier 1 support files (5.2), Search Everywhere, Go to File/Symbol/Class, File Structure, Find in Files, navigate back/forward, Recent Files/Locations, bookmarks, extend/shrink selection, folding, sticky lines, scope-aware highlighting, usage highlighting, and the dumb-mode accuracy benchmark.
3. **M2 — Git and local review:** gutter markers, blame/annotate, log, file history, the diff viewer with full (dumb) navigation inside diffs, local review mode with viewed tracking and local notes.
4. **M3 — GitHub review:** auth, PR list, PR checkout, inline threads, pending reviews, suggestions, submit, viewed sync, checks. At this point DumbJ is a complete review tool without any language server.
5. **M4 — Smart mode (LSP):** client, jdtls and kotlin-lsp presets with JDK/project auto-detection, exact Go to Declaration/Implementation/Type, Find/Show Usages, Quick Definition/Docs, hierarchies, inlay hints, semantic tokens, read-only diagnostics, exact/best-guess labelling, and the fallback to dumb results when the server is slow.
6. **M5 — Polish and packaging:** settings UI, keymap editor, Catppuccin Latte (light) and the other flavours, GNOME conflict detection, performance pass against the budgets, RPM + Flatpak for Fedora, and a macOS build that compiles and runs in CI (no polish needed yet).
7. **M6 — Tier 2 languages:** resolvers and LSP presets for the Tier 2 languages (Section 5.3), in an order to be agreed.
8. **Later:** GitLab, more keymaps, AppImage/.deb, macOS polish and signing.

## 10. How to work

- If something in this brief is ambiguous or seems wrong, list your assumptions at the start of the milestone and continue. Stop and ask only for decisions that would be expensive to reverse (stack, storage format, public config format).
- Write tests alongside features: unit tests for the core crates, golden tests for tree-sitter queries, integration tests against fixture git repos, and a mock GitHub server for the review flow.
- Keep `docs/FEATURES.md` as a terse checklist: one line per feature from Section 4 and per action from Section 6.2, with a status (`[x]` done, `[~]` partial, `[ ]` not started) and at most a few words of note. No prose.
- Keep the "Current milestone" line in `CLAUDE.md` up to date.
- **Docs to maintain by hand: only `CLAUDE.md`, `docs/ARCHITECTURE.md` and `docs/FEATURES.md`.** `docs/KEYMAP.md` and `THIRD_PARTY_LICENSES` are generated. Don't create a CHANGELOG (git history and the milestone summaries cover it until the first release) or any other docs unless I ask.
- Prefer fewer, well-finished features over many half-working ones. A navigation action that is sometimes wrong is worse than one that is missing.
