# DumbJ architecture

Status: pre-M0. Crate map, data flow and threading model get filled in during M0.

## Crate map

_TBD in M0._ Planned: `dumbj-text`, `dumbj-syntax`, `dumbj-index`, `dumbj-lsp`, `dumbj-search`, `dumbj-vcs`, `dumbj-review`, `dumbj-actions`, `dumbj-settings`, `dumbj-platform`, `dumbj-ui`, `dumbj-app`. Only `dumbj-ui` depends on GPUI.

## Threading model

_TBD in M0._

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
