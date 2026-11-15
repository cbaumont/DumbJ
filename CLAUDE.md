# DumbJ

A fast, native, keyboard-first **code reading and code review editor** with IntelliJ IDEA's navigation, look and shortcuts. **Not an IDE**: no build, run, debug, refactor or plugins. Linux (Fedora, Wayland) first; macOS later.

Full spec: `docs/BRIEF.md`. Where it conflicts with the Decisions in `docs/ARCHITECTURE.md`, the Decisions win. Read the sections relevant to your current task, not the whole file every time.

## Current milestone
M1b: the rest of M1 (see BRIEF §9), **in progress, close to its review stop**. M1a was approved on 2026-11-15; M0 on 2026-11-14. Decisions are in `docs/ARCHITECTURE.md`. Stop after each milestone with a summary, run instructions, test status, known gaps and screenshots, then wait for approval.

### M1b handover (2026-11-15)
Phases 1–4 and 6 are done, committed and pushed: hierarchy (Go to Implementation/Super Method, gutter icons, Type/Call Hierarchy window), Java↔Kotlin interop (accessors, facades, `@JvmName`, `INSTANCE`), library sources (source jars and the JDK `src.zip`, read in place), editor surface (folding, sticky lines, Extend/Shrink Selection, usage highlighting, parameter colouring), Tier 1 support files, `libs.versions.toml` navigation. Beyond the plan: Kotlin scope functions and typed collection/Flow lambdas in the resolver.

Left to do, in this order:
1. **Merge phase 5.** Branch `worktree-agent-ac66455a0311c8d69` (worktree in `.claude/worktrees/`, 7 commits, not yet merged or reviewed) adds Find in Files (new crate `dumbj-search`), Recent Files/Locations and Bookmarks with per-project state in `$XDG_STATE_HOME/dumbj/projects/<id>/state.toml`. It branched before the folding, sticky-line and usage-highlight work, so expect conflicts in `crates/dumbj-ui/src/editor.rs` (its bookmark gutter marker and `go_to` change), `workspace.rs`, `crates/dumbj-actions/src/registry.rs`, `keymaps/action-id-map.toml`, `docs/KEYMAP.md` (regenerate rather than merge) and `docs/FEATURES.md`. Review it, merge with `--no-ff`, run the gate, screenshot each feature, then remove the worktree and branch. Its known gaps: Find in Files toggles are mouse-only, no Bookmarks tool window, no Last Edit Location or Switcher.
2. **Split editing out.** Phase 7 (typing, undo/redo, autosave, reload prompt, index refresh on change) was not started. M1b grew past one reviewable milestone, so record a decision making editing its own milestone (the 2026-11-14 decision "Editing lands in M1b" and the "M1b assumptions" entry allow this) and leave it out of M1b.
3. **Docs.** `docs/FEATURES.md` lacks lines for interop, library sources and Go to Implementation's partial state; `docs/ARCHITECTURE.md` crate map lacks `dumbj-jars` (and `dumbj-search` after the merge); add the data flow for hierarchy, library sources and folding.
4. **Milestone review package:** summary, screenshots (`scripts/screenshot.sh`), known gaps, then stop for approval and push.

Accuracy benchmark (2026-11-15): 287 cases, 86.1% correct, 0 wrong (Java 92.9%, Kotlin 82.5%). Run `scripts/fetch-corpus.sh`, then `cargo run --release -p dumbj-index --example accuracy`; it also runs nightly (`.github/workflows/accuracy.yml`, never yet seen succeeding on GitHub: check it), failing on any wrong jump. Remaining chooser causes: indexing (`a[i].b`), smart casts, named-lambda parameters of other calls, Java `Class` arguments against Kotlin `Type` parameters.

Known gaps to list at review: no Go to Type Declaration or Go to Test; Call Hierarchy shows callers only; the index is not refreshed when files change; Go to Declaration and Go to Implementation run on the UI thread (single lookups); no code navigation inside library files; Markdown inline syntax is not highlighted.

Working notes: the owner wants self-contained work delegated to subagents on cheaper models to limit spend, and hates comments (keep only a non-obvious why). Gate every commit on `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo nextest run`, chained with `&&`. Push at milestone ends and good checkpoints. For screenshots, `wtype` mangles some keys (F7 arrives as `q`, Shift is dropped on Ctrl+Shift+digit): drive Find Usages through Ctrl+B on a declaration.

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
