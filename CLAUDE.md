# DumbJ

A fast, native, keyboard-first **code reading and code review editor** with IntelliJ IDEA's navigation, look and shortcuts. **Not an IDE**: no build, run, debug, refactor or plugins. Linux (Fedora, Wayland) first; macOS later.

Full spec: `docs/BRIEF.md`. Where it conflicts with the Decisions in `docs/ARCHITECTURE.md`, the Decisions win. Read the sections relevant to your current task, not the whole file every time.

## Current milestone
M1b: the rest of M1 (see BRIEF §9), **in progress**. M1a was approved on 2026-11-15 (the nightly accuracy workflow has not yet had a successful run on GitHub: check it). M0 was approved on 2026-11-14. Decisions are in `docs/ARCHITECTURE.md`. Stop after each milestone with a summary, run instructions, test status, known gaps and screenshots, then wait for approval.

M1b phases, in this order (each a series of small commits):
1. **Hierarchy:** Go to Implementation(s), Go to Super Method, overriding/implementing gutter icons, Type Hierarchy, dumb Call Hierarchy.
2. **Java↔Kotlin interop:** properties ↔ `getX`/`isX`/`setX`, `FileKt` facades and `@file:JvmName`, `Companion` and `@JvmStatic`/`@JvmField`/`@JvmName`/`@JvmOverloads`, `INSTANCE`; usages across the boundary. Add benchmark cases.
3. **Library source jars:** the global source-jar class index, read-only viewing inside the jar, "no sources available".
4. **Editor surface:** folding, sticky lines, Extend/Shrink selection, usage highlighting, scope-aware highlighting.
5. **Navigation extras:** Find in Files, Recent Files / Recent Locations, Bookmarks.
6. **Tier 1 support files (BRIEF §5.2):** grammars, highlighting and structure; `libs.versions.toml` navigation.
7. **Editing** (ARCHITECTURE "Editing lands in M1b"): typing, undo/redo, atomic autosave, reload prompt, index refresh on change.

Accuracy benchmark (2026-11-15): 264 cases, 84.1% correct, 14.4% chooser, 1.5% missed, 0 wrong. Run `scripts/fetch-corpus.sh`, then `cargo run --release -p dumbj-index --example accuracy`; it also runs nightly (`.github/workflows/accuracy.yml`), failing on any wrong jump. Known causes of the remaining choosers, for after the review: lambda parameter types (`it`), indexing (`a[i].b`), smart casts, scope functions (`apply { }`), and Java `Class` arguments not matching a Kotlin `Type` parameter.

Working notes: the owner wants self-contained work delegated to subagents on cheaper models to limit spend. Gate every commit on `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo nextest run`, chained with `&&`. For screenshots, `wtype` sends F7 as `q`: drive Find Usages through Ctrl+B on a declaration instead of Alt+F7.

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
