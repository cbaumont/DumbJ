# DumbJ features

`[x]` done · `[~]` partial · `[ ]` not started. Sections follow `docs/BRIEF.md`.

## 4.1 Workspace and window layout
- [~] Open a folder as a project — CLI path, Open Folder; no recent projects or saved state
- [~] Editor tabs — open, close, next/previous; no splits or drag
- [~] Tool windows with stripe — Project only, left side
- [ ] Navigation bar
- [~] Status bar — breadcrumb, branch, line:col, language
- [~] Project tool window — tree with git status colours on files; folders are not coloured
- [ ] Structure tool window
- [x] Find tool window — Find Usages and Find in Files results, grouped by file
- [ ] Git tool window
- [~] Review tool window — local review only: the changed files of a base/head range, flat list
- [ ] Bookmarks tool window
- [ ] Problems tool window
- [~] Hierarchy tool window — below the editor; no docking choice
- [~] Hide all tool windows; no distraction-free mode
- [x] Catppuccin Mocha theme, as data files
- [~] Catppuccin Latte — loads and is tested; not selectable, no system dark/light
- [~] JetBrains Mono bundled, ligatures off; no font settings
- [x] Openly licensed icons (Lucide)

## 4.2 Editor surface
- [x] Large files scroll smoothly (100k lines)
- [~] Degraded mode — no highlighting over 20 MB, long lines cut; no notice shown
- [~] Tree-sitter highlighting — Java, Kotlin, Groovy, YAML, XML, JSON, TOML, .properties, Markdown (block level only), shell, Dockerfile, SQL
- [~] Scope-aware highlighting (locals) — parameters and their uses; locals keep the default colour
- [ ] Semantic tokens (LSP)
- [~] Gutter — line numbers, fold markers, implements/overrides icons, bookmarks, change markers against HEAD
- [ ] Inlay hints
- [ ] Inline blame / Annotate
- [x] Sticky lines
- [ ] Indent guides, bracket matching, whitespace, right margin, soft wrap
- [~] Code folding — no "fold all comments" action
- [x] Highlight usages at caret
- [~] Scrollbar — draggable thumb; no marks
- [ ] Hover popups
- [~] Editing — typing (IME), undo/redo with typing groups, Enter with auto-indent, Tab/Shift+Tab, cut/paste, duplicate/delete/move line, line comments; no multi-caret, column selection, move statement, block comments, bracket auto-close, EditorConfig or encodings other than UTF-8
- [x] Autosave — atomic; on tab switch, focus loss, close, quit and 15 s idle
- [~] Files changed on disk — reload when unmodified, reload/keep prompt when modified (mouse-only); no merge
- [x] Caret movement, selection, copy, mouse selection
- [x] Find in file with match count
- [ ] Replace in file

## 4.3 Navigation
- [x] Search Everywhere
- [x] Go to File / Class / Symbol
- [x] Go to Line:Column
- [ ] Find Action
- [~] Go to Declaration / Type Declaration / Implementation / Super Method / Test — no Type Declaration or Test
- [~] Go to Implementation — project sources only, from supertypes in the index (best guess); no implementations inside libraries
- [~] Library sources — Go to Declaration opens classes from Gradle/Maven source jars and the JDK `src.zip`, read in place; no navigation inside library files, no decompiler
- [x] Find Usages / Show Usages
- [ ] Quick Definition / Quick Documentation
- [x] File Structure popup
- [~] Call Hierarchy / Type Hierarchy — callers only, no callees
- [x] Navigate Back / Forward
- [~] Recent Files, Recent Locations — per project, kept in the state dir; no Last Edit Location or Switcher
- [~] Find in Files — regex, case, whole word, file mask, directory scope, gitignore; toggles are mouse-only; no Replace in Files
- [~] Bookmarks — anonymous and mnemonic, gutter markers, list popup, kept per project in the state dir; no tool window
- [ ] Copy Reference / Path / permalink
- [ ] Navigation bar jump

## 4.3.1 Dumb and smart navigation
- [x] Scope-aware resolution
- [x] Import following (Java, Kotlin)
- [x] Candidate ranking and chooser
- [~] Member access heuristics — lambda parameter types, indexing (`a[i].b`), smart casts and scope functions stay guesses
- [x] Java ↔ Kotlin interop — property accessors, file facade classes, `@JvmName`, `INSTANCE`, companion members
- [x] Gradle version catalog — `libs.x.y` accessors jump to `libs.versions.toml`
- [x] Dumb Find Usages
- [x] Dumb implementations / hierarchy
- [~] Exact / best-guess labelling — chooser headed "best guess", usages marked "maybe"; no label on a direct jump
- [ ] Language-server lifecycle
- [x] Accuracy benchmark — 287 cases, nightly in CI

## 4.4 Version control and local review
- [~] Change markers — added/modified/deleted against HEAD, Next/Previous Change, Rollback Lines; no marker popup with the old text
- [ ] Git log tool window
- [ ] Branch popup
- [~] Diff viewer — side by side against HEAD from the Project tree, word-level highlighting, next/previous difference, ignore whitespace (mouse-only), synchronised scrolling, editable right side, any two revisions through a review, next/previous file within a review; no unified view or collapse unchanged
- [~] Local review mode — Start Review takes `base`, `base..head` or `base...head` (merge base) with branch suggestions; changed-file list on the left, diff on the right; F7/Shift+F7 continue into the next/previous file; no viewed tracking or notes yet
- [ ] File history
- [ ] Checkout / fetch

## 4.5 Remote code review
- [ ] Sign in
- [ ] PR list
- [ ] Open PR into private refs
- [ ] Inline threads
- [ ] Comments, pending review, suggestions
- [ ] Submit review
- [ ] Viewed sync
- [ ] Description, timeline, checks, commits
- [ ] Pending comments survive restart

## 4.6 Settings and keymap
- [ ] Settings UI
- [ ] Keymap editor
- [~] Built-in keymaps — macOS For All (Linux), IntelliJ macOS; not selectable
- [x] IntelliJ keymap XML loader with parent inheritance
- [x] Action registry
- [x] Generated `docs/KEYMAP.md`, checked by a test
- [ ] User keymap import

## 6.2 Actions (IntelliJ ID)
Search & go to
- [x] SearchEverywhere
- [ ] GotoAction
- [x] GotoClass
- [x] GotoFile
- [x] GotoSymbol
- [x] GotoLine
- [x] RecentFiles
- [x] RecentLocations
- [ ] Switcher

Code navigation
- [x] GotoDeclaration
- [ ] GotoTypeDeclaration
- [x] GotoImplementation
- [x] GotoSuperMethod
- [ ] GotoTest
- [x] FindUsages
- [x] ShowUsages
- [ ] HighlightUsagesInFile
- [ ] QuickImplementations
- [ ] QuickJavaDoc
- [ ] ParameterInfo
- [x] FileStructurePopup
- [x] TypeHierarchy
- [~] CallHierarchy — callers only
- [x] Back
- [x] Forward
- [ ] JumpToLastChange
- [ ] ShowNavBar
- [ ] SelectIn
- [ ] GotoNextError
- [ ] GotoPreviousError
- [ ] EditSource

Find
- [x] Find
- [ ] Replace
- [x] FindNext
- [x] FindPrevious
- [x] FindInPath
- [ ] ReplaceInPath

Bookmarks
- [x] ToggleBookmark
- [x] ToggleBookmarkWithMnemonic
- [x] ShowBookmarks
- [x] GotoBookmark0…9 — with ToggleBookmark0…9

Selection & editing
- [x] EditorSelectWord
- [x] EditorUnSelectWord
- [ ] SelectNextOccurrence
- [ ] UnselectPreviousOccurrence
- [ ] SelectAllOccurrences
- [ ] EditorCloneCaretAbove
- [ ] EditorCloneCaretBelow
- [ ] EditorToggleColumnMode
- [x] CommentByLineComment
- [ ] CommentByBlockComment
- [x] EditorDuplicate
- [x] EditorDeleteLine
- [ ] MoveStatementUp / MoveStatementDown
- [x] MoveLineUp / MoveLineDown
- [x] ExpandRegion / CollapseRegion
- [x] ExpandAllRegions / CollapseAllRegions

Windows & tabs
- [x] CloseContent
- [x] PreviousTab
- [x] NextTab
- [x] ActivateProjectToolWindow
- [~] ActivateFindToolWindow — available once Find Usages has produced results
- [ ] ActivateStructureToolWindow
- [ ] ActivateVersionControlToolWindow
- [ ] ActivateBookmarksToolWindow
- [ ] ActivateProblemsViewToolWindow
- [x] HideAllWindows
- [ ] JumpToLastWindow
- [ ] ShowSettings

VCS & diff
- [x] VcsShowNextChangeMarker
- [x] VcsShowPrevChangeMarker
- [x] Vcs.RollbackChangedLines
- [x] ShowDiff (`Compare.SameVersion`, Project tree only)
- [x] NextDiff
- [x] PreviousDiff
- [x] Diff.FocusOppositePane
- [ ] Vcs.QuickListPopupAction
- [ ] RecentChanges
- [ ] Annotate
- [ ] CopyReference

Review (DumbJ only)
- [ ] Add Review Comment
- [ ] Start / Submit Review
- [ ] Next / Previous Comment Thread
- [ ] Resolve Thread
- [ ] Toggle File Viewed
- [ ] Next Unviewed File
- [ ] Open PR in Browser
- [ ] Copy Permalink

Also bound (not in §6.2): OpenFile (Open Folder, no default shortcut), caret movement and selection (EditorLeft … EditorNextWordWithSelection), $SelectAll, $Copy, $Cut, $Paste, $Undo, $Redo, EditorBackSpace, EditorDelete, EditorEnter, EditorTab, EditorUnindentSelection, EditorEscape. See `docs/KEYMAP.md`.
