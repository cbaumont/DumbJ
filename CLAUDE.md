# DumbJ

A fast, native, keyboard-first **code reading and code review editor** with IntelliJ IDEA's navigation, look and shortcuts. **Not an IDE**: no build, run, debug, refactor or plugins. Linux (Fedora, Wayland) first; macOS later.

Full spec: `docs/BRIEF.md` (git-ignored; local only). Where it conflicts with the Decisions in `docs/ARCHITECTURE.md`, the Decisions win. Read the sections relevant to your current task, not the whole file every time.

## Current milestone
M0: Spike and skeleton (see BRIEF §9), not started; awaiting go-ahead. Decisions from the 2026-11-14 design review are in `docs/ARCHITECTURE.md`. Stop after each milestone with a summary, run instructions, test status, known gaps and screenshots, then wait for approval.

## Core rules
- **Java and Kotlin first** (BRIEF §5). They get IntelliJ-grade navigation, including Java↔Kotlin interop, Gradle/Maven project structure and library source jars. Other languages get highlighting and structure only until M6.
- **Dumb first, smart when possible.** Tree-sitter navigation must work everywhere with nothing installed. LSP is an optional precision layer. Label results as exact or best guess.
- **Never block the UI thread.** All indexing, parsing, git and LSP work runs in the background and can be cancelled.
- **Read-only by default.** Never modify the user's files or git state except for edits (IDEA-style autosave, atomic writes), explicit checkout, and fetch. PR refs go only under `refs/dumbj/pr/<n>/{head,base}`. Other revisions are read from git objects, never checked out. Per-project state lives in XDG dirs, never inside the repo.
- **Keymap:** "macOS For All" on Linux, IntelliJ macOS on macOS, loaded from the vendored IntelliJ XML in `keymaps/`. Never hard-code shortcuts.
- **Theme:** Catppuccin Mocha (default) and Latte, using exact palette values.
- **Stack: Rust + GPUI, a native desktop app** (no web view). GPUI is pinned to a specific revision and used only in the `ui` crate. Widgets and the editor are our own (gpui-component is reference only, not a dependency).
- **Licence: GPL-3.0-or-later.** Small adapted pieces of Zed code are allowed with a header comment naming the source file and revision. Never vendor Zed crates wholesale. Dependencies must be GPL-3-compatible (no GPL-2.0-only).
- **`ui` depends on core crates, never the reverse.** Core crates must be testable headless.
- **Rust standards (BRIEF §8.1):** workspace lints, no `unwrap`/`expect`/`panic` in library crates, `thiserror` in libraries and `anyhow` in the app, `tracing` not `println!`, no `unsafe` without a `// SAFETY:` comment, newtypes for offsets, and `cargo deny` + `nextest` in CI.
- Fewer, finished features beat many half-working ones. A wrong navigation result is worse than none.

## Commands
<!-- Fill in during M0 once the stack is chosen -->
- Build: `TODO`
- Run: `TODO`
- Test: `TODO`
- Lint/format: `TODO`
- Regenerate KEYMAP.md: `TODO`

## Docs (only these are maintained by hand)
- `docs/ARCHITECTURE.md`: crate map, threading, and a dated **Decisions** section.
- `docs/FEATURES.md`: a terse `[x]` / `[~]` / `[ ]` checklist mapped to BRIEF §4 and §6.2.
- This file: keep "Current milestone" and "Commands" up to date.

Generated, never edit by hand: `docs/KEYMAP.md`, `THIRD_PARTY_LICENSES`. No CHANGELOG before the first release.
