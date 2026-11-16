# DumbJ

A fast, native, keyboard-first **code reading and code review editor** with IntelliJ IDEA's navigation, look and shortcuts. **Not an IDE**: no build, run, debug, refactor or plugins. Linux (Fedora, Wayland) first; macOS later.

Full spec: `docs/BRIEF.md`. Where it conflicts with the Decisions in `docs/ARCHITECTURE.md`, the Decisions win. Read the sections relevant to your current task, not the whole file every time.

## Current milestone
M2: Git and local review (BRIEF §9, §4.4), **in progress** (started 2026-11-16). M1c (editing) was completed on 2026-11-16 and its review package posted; M1b was approved on 2026-11-15, M1a on 2026-11-15, M0 on 2026-11-14. Decisions are in `docs/ARCHITECTURE.md`. At each milestone end write the review package (summary, run instructions, test status, known gaps, screenshots) and push; the owner asked on 2026-11-15 not to be asked for approval between milestones unless strictly necessary, so then continue with the next one.

### M2 plan
1. **Done.** `dumbj-vcs` (headless): repository discovery, branch, HEAD text of a file (`gix`), statuses, line hunks (`imara-diff`).
2. **Done.** Gutter change markers against HEAD, refreshed on edits and saves; Next/Previous Change (`VcsShowNextChangeMarker`, `VcsShowPrevChangeMarker`); Rollback Changed Lines (`Vcs.RollbackChangedLines`); branch in the status bar; git status colours in the Project tree.
   `dumbj-vcs` also has revisions, `changed_files`, `log`, `word_changes` and `side_by_side`.
3. **Partly done.** Revision filesystem: `EditorView::open_revision` reads a file at a revision (read-only) and the diff's left pane shows it under a revision label; standalone revision tabs come with the log (phase 6), which is what opens them.
4. **Partly done.** Diff viewer (`workspace/diff.rs`): side by side against HEAD from the Project tree (Ctrl+D), word-level highlighting, Next/Previous Difference (F7, Shift+F7), Go to Opposite Pane (Ctrl+Tab), ignore whitespace, scroll sync, navigation inside both sides. Still missing: unified view, collapse unchanged, next/previous file (needs the changes tree of phase 5), diffs between two arbitrary revisions (`DiffTab` always takes HEAD).
5. Local review mode: base and head pickers, changes tree, viewed tracking (reset when a file changes), local notes exported as Markdown.
6. Git log tool window (graph, filters, commit details, changed files) and file history, including history for a selection.
7. Annotate (blame) in the gutter; branch popup with checkout and fetch (the only operations that change git state).


M1c holds: typing through the input method handler, undo/redo with typing groups (`dumbj_text::History`), Enter with auto-indent, Tab/Shift+Tab, cut/paste, duplicate/delete/move line, line comments, incremental reparse, atomic autosave (tab switch, focus loss, close, quit, 15 s idle), reload or a reload/keep bar for files changed on disk, read-only library and non-UTF-8 files, index refresh after saves and on window focus. Its gaps: no multi-caret, column selection, move statement, block comments, bracket auto-close, EditorConfig or other encodings; the conflict bar is mouse-only and has no merge; the edited file's index entry is as old as its last save; the editing commit (`c07a78e`) is larger than the commit rule allows.

M1b holds: hierarchy (Go to Implementation/Super Method, gutter icons, Type/Call Hierarchy window), Java↔Kotlin interop (accessors, facades, `@JvmName`, `INSTANCE`), library sources (source jars and the JDK `src.zip`, read in place), editor surface (folding, sticky lines, Extend/Shrink Selection, usage highlighting, parameter colouring), Tier 1 support files, `libs.versions.toml` navigation, Find in Files (`dumbj-search`), Recent Files/Locations and Bookmarks (per-project state in `$XDG_STATE_HOME/dumbj/projects/<id>/state.toml`), Kotlin scope functions and typed collection/Flow lambdas in the resolver.

Accuracy benchmark (2026-11-15): 287 cases, 86.1% correct, 0 wrong (Java 92.9%, Kotlin 82.5%). Run `scripts/fetch-corpus.sh`, then `cargo run --release -p dumbj-index --example accuracy`; it also runs nightly (`.github/workflows/accuracy.yml`), failing on any wrong jump. Remaining chooser causes: indexing (`a[i].b`), smart casts, named-lambda parameters of other calls, Java `Class` arguments against Kotlin `Type` parameters.

Known gaps: no Go to Type Declaration or Go to Test; Call Hierarchy shows callers only and cannot be cancelled; Go to Declaration and Go to Implementation run on the UI thread (single lookups); no code navigation inside library files; Markdown inline syntax is not highlighted; Find in Files toggles are mouse-only; no Bookmarks tool window, Last Edit Location or Switcher.

Working notes: the owner wants self-contained work delegated to subagents on cheaper models to limit spend, and hates comments (keep only a non-obvious why). Gate every commit on `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo nextest run`, chained with `&&`. Push at milestone ends and good checkpoints. For screenshots, `wtype` mangles some keys (F7 arrives as `q`, Shift is dropped on Ctrl+Shift+digit; F3 and Ctrl+Shift+letter work): drive Find Usages through Ctrl+B on a declaration, move the caret with Ctrl+L `line:column`, switch tabs with Ctrl+E then Enter (Ctrl+Shift+] is mangled), and set `XDG_STATE_HOME` to a scratch directory so the run does not touch real project state. The window loses focus when a `wtype` run ends, which autosaves: to test unsaved states, keep everything in one `KEYS` string and change files from a background `sleep`.

## Core rules
- **Java and Kotlin first** (BRIEF §5). They get IntelliJ-grade navigation, including Java↔Kotlin interop, Gradle/Maven project structure and library source jars. Other languages get highlighting and structure only until M6.
- **Dumb first, smart when possible.** Tree-sitter navigation must work everywhere with nothing installed. LSP is an optional precision layer. Label results as exact or best guess.
- **Never block the UI thread.** All indexing, parsing, git and LSP work runs in the background and can be cancelled.
- **Read-only by default.** Change the user's files or git state only through edits, explicit checkout and fetch, and never write inside the repo (BRIEF §2.5, §4.5).
- **Keymap:** "macOS For All" on Linux, IntelliJ macOS on macOS, loaded from the vendored IntelliJ XML in `keymaps/`. Never hard-code shortcuts.
- **Theme:** Catppuccin Mocha (default) and Latte, using exact palette values.
- **Stack: Rust + GPUI, a native desktop app** (no web view). GPUI is pinned to a specific revision and used only in the `ui` crate. Widgets and the editor are our own (gpui-component is reference only, not a dependency).
- **Licence: GPL-3.0-or-later.** Zed code may be adapted only in small, attributed pieces (BRIEF §3, §7).
- **`ui` depends on core crates, never the reverse.** Core crates must be testable headless.
- **Rust standards (BRIEF §8.1):** workspace lints, no `unwrap`/`expect`/`panic` in library crates, `thiserror` in libraries and `anyhow` in the app, `tracing` not `println!`, no `unsafe` without a `// SAFETY:` comment, newtypes for offsets, and `cargo deny` + `nextest` in CI.
- **Commit in small, incremental chunks.** Commit as you go, not at the end of a milestone. Each commit is one logical change that builds and passes fmt, clippy and tests, is small enough to review in one sitting, and never mixes refactoring with feature work. Commit messages say what changed and why, and justify any new dependency.
- Fewer, finished features beat many half-working ones. A wrong navigation result is worse than none.

## Commands
Rust comes from rustup (`~/.cargo/bin` must be on `PATH`); the toolchain is pinned in `rust-toolchain.toml`. Linking needs `clang` and `mold`.
- Build: `cargo build` (release: `cargo build --release`)
- Run: `cargo run -- <folder-or-file>`; more logs with `DUMBJ_LOG=debug`
- Test: `cargo nextest run`
- Lint/format: `cargo fmt --all` · `cargo clippy --all-targets -- -D warnings` · `cargo deny check`
- Regenerate KEYMAP.md: `DUMBJ_REGENERATE_KEYMAP=1 cargo nextest run -p dumbj-actions keymap_md_is_up_to_date`
- Update highlight snapshots: `cargo insta test --accept -p dumbj-syntax`
- Screenshot: `scripts/screenshot.sh out.png <path>` (headless sway; `SCALE`, `KEYS`, `X11=1`)
- Scroll benchmark: `DUMBJ_SCROLL_BENCHMARK=1 cargo run --release -- <file>`

## Docs (only these are maintained by hand)
- `docs/ARCHITECTURE.md`: crate map, threading, and a dated **Decisions** section.
- `docs/FEATURES.md`: a terse `[x]` / `[~]` / `[ ]` checklist mapped to BRIEF §4 and §6.2.
- This file: keep "Current milestone" and "Commands" up to date.

Generated, never edit by hand: `docs/KEYMAP.md`, `THIRD_PARTY_LICENSES`. No CHANGELOG before the first release.
