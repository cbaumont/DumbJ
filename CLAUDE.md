# DumbJ

A fast, native, keyboard-first **code reading and code review editor** with IntelliJ IDEA's navigation, look and shortcuts. **Not an IDE**: no build, run, debug, refactor or plugins. Linux (Fedora, Wayland) first; macOS later.

Full spec: `docs/BRIEF.md`. Where it conflicts with the Decisions in `docs/ARCHITECTURE.md`, the Decisions win. Read the sections relevant to your current task, not the whole file every time.

## Current milestone
M3: GitHub review (BRIEF §9, §4.5), **phase 4 next** (started 2026-11-16). M2 (git and local review) was completed on 2026-11-16 and its review package posted; M1c was completed on 2026-11-16, M1b approved on 2026-11-15, M1a on 2026-11-15, M0 on 2026-11-14. Decisions are in `docs/ARCHITECTURE.md`. At each milestone end write the review package (summary, run instructions, test status, known gaps, screenshots) and push; the owner asked on 2026-11-15 not to be asked for approval between milestones unless strictly necessary, so then continue with the next one.

### M3 phases
Design: "M3 design (GitHub review)" in `docs/ARCHITECTURE.md`. Tick a phase here when it is committed.
1. [x] `dumbj-review`: token from `gh auth token`, remote URL → host/owner/repo, GraphQL client on `ureq`, PR list query, mock-server tests.
2. [x] PR refs in `dumbj-vcs` (`refs/dumbj/pr/<n>/{head,base}`, prune) and the head-revision index in `dumbj-index` with read-only revision tabs.
3. [x] PR list popup (Ctrl+Alt+Shift+P, P: review requested, assigned, mine, all; search), open a PR as a review, Open PR in Browser. Left for phase 7: a Prune review refs action (`Repository::prune_pull_refs` exists).
4. [ ] Threads: fetch, block rows in the editor, fold/expand, resolved and outdated state, Next/Previous Comment Thread.
5. [ ] Writing: Add Review Comment (line or range, suggestion block), reply, resolve/unresolve, pending comments in the project state, Start/Submit Review (Comment, Approve, Request changes).
6. [ ] Viewed sync, PR overview (description, checks, commits, commit-by-commit review), Copy Permalink.
7. [ ] Docs, FEATURES, screenshots, review package, push.

Settled with the owner:
- **Auth:** reuse the `gh` CLI's token (`gh auth token`, per host); no device flow or token storage of our own in M3. If `gh` is missing or logged out, say so in the UI and point to `gh auth login`.
- **Test repository:** `https://github.com/cbaumont/set-the-play-core` (public, Kotlin, one commit, no PRs). The owner allows creating branches, PRs, comments and reviews there freely; touch no other repository on GitHub.

Groundwork from M2: `workspace/review.rs`, `diff.rs` and `review_notes.rs`; `dumbj_settings::Review` (viewed marks keyed on blob ids, notes with path/side/lines/excerpt); `Repository::fetch` and `git::run_raw_env` for network git without prompts; DumbJ-only actions are declared in `keymaps/dumbj-*.xml` and the clash test guards them.

M2 holds: `dumbj-vcs` (gix for HEAD blobs, the git CLI for everything else: status, revisions, changed files, content ids, log with refs and filters, blame, checkout, fetch; `imara-diff` hunks and word changes; a lane graph); gutter change markers, Next/Previous Change, Rollback Lines; the diff viewer (`workspace/diff.rs`: two real editors for any base/head pair, word highlights, F7/Shift+F7, Ctrl+Tab, ignore whitespace, scroll sync); local review (`workspace/review.rs`, `review_notes.rs`: `base`, `base..head`, `base...head`, viewed tracking keyed on the head blob id, one-line notes copied as Markdown); the Git tool window (`workspace/git_log.rs`, Alt+9: graph, refs, filter field, commit files, Show History and Show History for Selection); Annotate and the branch popup with checkout and fetch (`workspace/branches.rs`). DumbJ-only actions get their shortcuts from `keymaps/dumbj-linux.xml` and `dumbj-macos.xml`. Its gaps: no unified diff or collapse unchanged; no standalone revision tabs; the review file list is flat and notes are one line; log dates are UTC and capped at 2000 commits; Annotate has no age colours or commit popup; the branch popup cannot compare branches; the ignore-whitespace toggle is mouse-only; F7 is untested in the app (`wtype` cannot send it).

M1c holds: typing through the input method handler, undo/redo with typing groups (`dumbj_text::History`), Enter with auto-indent, Tab/Shift+Tab, cut/paste, duplicate/delete/move line, line comments, incremental reparse, atomic autosave (tab switch, focus loss, close, quit, 15 s idle), reload or a reload/keep bar for files changed on disk, read-only library and non-UTF-8 files, index refresh after saves and on window focus. Its gaps: no multi-caret, column selection, move statement, block comments, bracket auto-close, EditorConfig or other encodings; the conflict bar is mouse-only and has no merge; the edited file's index entry is as old as its last save; the editing commit (`c07a78e`) is larger than the commit rule allows.

M1b holds: hierarchy (Go to Implementation/Super Method, gutter icons, Type/Call Hierarchy window), Java↔Kotlin interop (accessors, facades, `@JvmName`, `INSTANCE`), library sources (source jars and the JDK `src.zip`, read in place), editor surface (folding, sticky lines, Extend/Shrink Selection, usage highlighting, parameter colouring), Tier 1 support files, `libs.versions.toml` navigation, Find in Files (`dumbj-search`), Recent Files/Locations and Bookmarks (per-project state in `$XDG_STATE_HOME/dumbj/projects/<id>/state.toml`), Kotlin scope functions and typed collection/Flow lambdas in the resolver.

Accuracy benchmark (2026-11-15): 287 cases, 86.1% correct, 0 wrong (Java 92.9%, Kotlin 82.5%). Run `scripts/fetch-corpus.sh`, then `cargo run --release -p dumbj-index --example accuracy`; it also runs nightly (`.github/workflows/accuracy.yml`), failing on any wrong jump. Remaining chooser causes: indexing (`a[i].b`), smart casts, named-lambda parameters of other calls, Java `Class` arguments against Kotlin `Type` parameters.

Known gaps: no Go to Type Declaration or Go to Test; Call Hierarchy shows callers only and cannot be cancelled; Go to Declaration and Go to Implementation run on the UI thread (single lookups); no code navigation inside library files; Markdown inline syntax is not highlighted; Find in Files toggles are mouse-only; no Bookmarks tool window, Last Edit Location or Switcher.

Working notes: the owner wants self-contained work delegated to subagents on cheaper models to limit spend, and hates comments (keep only a non-obvious why). Gate every commit on `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo nextest run`, chained with `&&`. Push at milestone ends and good checkpoints. For screenshots, `wtype` mangles some keys (F7 arrives as `q`, Shift is dropped on Ctrl+Shift+digit; F3 and Ctrl+Shift+letter work): drive Find Usages through Ctrl+B on a declaration, move the caret with Ctrl+L `line:column`, switch tabs with Ctrl+E then Enter (Ctrl+Shift+] is mangled), and set `XDG_STATE_HOME` to a scratch directory so the run does not touch real project state. `wtype` joins separate text arguments with spaces, so type a query as one argument. The window loses focus when a `wtype` run ends, which autosaves: to test unsaved states, keep everything in one `KEYS` string and change files from a background `sleep`.

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
- GitHub API check: `cargo run -p dumbj-review --example pulls -- <remote-url>` (uses the `gh` token)
- Scroll benchmark: `DUMBJ_SCROLL_BENCHMARK=1 cargo run --release -- <file>`

## Docs (only these are maintained by hand)
- `docs/ARCHITECTURE.md`: crate map, threading, and a dated **Decisions** section.
- `docs/FEATURES.md`: a terse `[x]` / `[~]` / `[ ]` checklist mapped to BRIEF §4 and §6.2.
- This file: keep "Current milestone" and "Commands" up to date.

Generated, never edit by hand: `docs/KEYMAP.md`, `THIRD_PARTY_LICENSES`. No CHANGELOG before the first release.
