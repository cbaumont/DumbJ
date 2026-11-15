# DumbJ features

`[x]` done · `[~]` partial · `[ ]` not started. Sections follow `docs/BRIEF.md`.

## 4.1 Workspace and window layout
- [~] Open a folder as a project — CLI path, Open Folder; no recent projects or saved state
- [~] Editor tabs — open, close, next/previous; no splits or drag
- [~] Tool windows with stripe — Project only, left side
- [ ] Navigation bar
- [~] Status bar — breadcrumb, line:col, language
- [~] Project tool window — tree; no git status colours
- [ ] Structure tool window
- [~] Find tool window — Find Usages results only
- [ ] Git tool window
- [ ] Review tool window
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
- [~] Tree-sitter highlighting — Java, Kotlin, YAML
- [ ] Scope-aware highlighting (locals)
- [ ] Semantic tokens (LSP)
- [~] Gutter — line numbers, implements/overrides icons
- [ ] Inlay hints
- [ ] Inline blame / Annotate
- [ ] Sticky lines
- [ ] Indent guides, bracket matching, whitespace, right margin, soft wrap
- [ ] Code folding
- [ ] Highlight usages at caret
- [~] Scrollbar — draggable thumb; no marks
- [ ] Hover popups
- [ ] Editing — editor is read-only (see ARCHITECTURE decision)
- [x] Caret movement, selection, copy, mouse selection
- [x] Find in file with match count
- [ ] Replace in file

## 4.3 Navigation
- [x] Search Everywhere
- [x] Go to File / Class / Symbol
- [x] Go to Line:Column
- [ ] Find Action
- [~] Go to Declaration / Type Declaration / Implementation / Super Method / Test — no Type Declaration or Test
- [x] Find Usages / Show Usages
- [ ] Quick Definition / Quick Documentation
- [x] File Structure popup
- [~] Call Hierarchy / Type Hierarchy — callers only, no callees
- [x] Navigate Back / Forward
- [ ] Recent Files / Recent Locations / Last Edit Location / Switcher
- [ ] Find in Files / Replace in Files
- [ ] Bookmarks
- [ ] Copy Reference / Path / permalink
- [ ] Navigation bar jump

## 4.3.1 Dumb and smart navigation
- [x] Scope-aware resolution
- [x] Import following (Java, Kotlin)
- [x] Candidate ranking and chooser
- [~] Member access heuristics — lambda parameter types, indexing (`a[i].b`), smart casts and scope functions stay guesses
- [x] Dumb Find Usages
- [x] Dumb implementations / hierarchy
- [~] Exact / best-guess labelling — chooser headed "best guess", usages marked "maybe"; no label on a direct jump
- [ ] Language-server lifecycle
- [x] Accuracy benchmark — 264 cases, nightly in CI

## 4.4 Version control and local review
- [ ] Git log tool window
- [ ] Branch popup
- [ ] Diff viewer
- [ ] Local review mode
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
- [ ] RecentFiles
- [ ] RecentLocations
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
- [ ] FindInPath
- [ ] ReplaceInPath

Bookmarks
- [ ] ToggleBookmark
- [ ] ToggleBookmarkWithMnemonic
- [ ] ShowBookmarks
- [ ] GotoBookmark0…9

Selection & editing
- [~] EditorSelectWord — word level only
- [ ] EditorUnSelectWord
- [ ] SelectNextOccurrence
- [ ] UnselectPreviousOccurrence
- [ ] SelectAllOccurrences
- [ ] EditorCloneCaretAbove
- [ ] EditorCloneCaretBelow
- [ ] EditorToggleColumnMode
- [ ] CommentByLineComment
- [ ] CommentByBlockComment
- [ ] EditorDuplicate
- [ ] EditorDeleteLine
- [ ] MoveStatementUp / MoveStatementDown
- [ ] MoveLineUp / MoveLineDown
- [ ] ExpandRegion / CollapseRegion
- [ ] ExpandAllRegions / CollapseAllRegions

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
- [ ] VcsShowNextChangeMarker
- [ ] VcsShowPrevChangeMarker
- [ ] Vcs.RollbackChangedLines
- [ ] ShowDiff
- [ ] NextDiff
- [ ] PreviousDiff
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

Also bound (not in §6.2): OpenFile (Open Folder, no default shortcut), caret movement and selection (EditorLeft … EditorNextWordWithSelection), $SelectAll, $Copy, $Cut, $Paste, EditorBackSpace, EditorDelete, EditorEnter, EditorEscape. See `docs/KEYMAP.md`.
