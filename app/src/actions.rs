use std::path::PathBuf;

use gpui::actions;

actions!(
    editor,
    [
        Save,
        Quit,
        NewFile,
        OpenFile,
        OpenFolder,
        OpenSettings,
        ShowExplorer,
        ShowSearch,
        ShowGit,
        ShowExtensions,
        ToggleSidebar,
        ToggleTerminal,
        NewTerminal,
        /// Toggle the Zed-style terminal panel docked to the right edge.
        /// Fully independent of the bottom panel: separate PTY sessions.
        ToggleTerminalRight,
        About,
        ExplorerRefresh,
        ExplorerCollapseAll,
        ExplorerToggleStickyScroll,
        CloseTab,
        NextTab,
        PrevTab,
        CloseActiveTab,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        CopyDiagnostic,
        FormatDocument,
        GitRefresh,
        GitStageAll,
        GitUnstageAll,
        GitDiscardAll,
        GitCommit,
        GitCommitAll,
        GitCommitAmend,
        GitFetch,
        GitPull,
        GitPush,
        GitForcePush,
        GitStashPush,
        GitStashPop,
        GitInit,
        GitBranchPicker,
        /// Toggle the Zed-style per-line git blame in the gutter of the active
        /// editor ("Toggle Git Blame").
        ToggleGitBlame,
        /// Switch the Source Control panel to the commit "History" graph.
        GitShowHistory,
        /// Switch the Source Control panel back to the changes list.
        GitShowChanges,
        /// Load the next page of commits into the history graph.
        GitHistoryLoadMore,
        NextTerminal,
        PrevTerminal,
        CloseTerminal,
        /// Close every terminal tab in the focused dock except the active
        /// one (pinned tabs survive, matching Zed's `pane::CloseOtherItems`).
        CloseOtherTerminals,
        /// Close the terminal tabs positioned before the active one.
        CloseTerminalsLeft,
        /// Close the terminal tabs positioned after the active one.
        CloseTerminalsRight,
        /// Close terminals whose child process has already exited
        /// (Zed's `pane::CloseCleanItems` applied to terminals).
        CloseCleanTerminals,
        /// Close every terminal tab in the focused dock (pinned tabs survive).
        CloseAllTerminals,
        /// Pin or unpin the active terminal tab (Zed's `pane::TogglePinTab`).
        ToggleTerminalPin,
        /// Rename the active terminal tab inline.
        RenameTerminal,
        /// Toggle the active terminal's read-only state.
        ToggleTerminalReadOnly,
        ClearTerminal,
        /// Copy the active terminal selection.
        TerminalCopy,
        /// Paste the current clipboard payload into the active terminal.
        TerminalPaste,
        /// Paste only the clipboard's text representation.
        TerminalPasteText,
        /// Select the active terminal's complete scrollback.
        TerminalSelectAll,
        TerminalTab1,
        TerminalTab2,
        TerminalTab3,
        TerminalTab4,
        TerminalTab5,
        ToggleFileFinder,
        ToggleCommandPalette,
        ToggleGoToLine,
        ToggleLanguageSelector,
        CloseModal
    ]
);

#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct SwitchTab {
    pub index: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct CloseTabAt {
    pub index: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct SelectTheme {
    pub ix: usize,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerNewFile {
    pub parent: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerNewFolder {
    pub parent: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerRevealInFinder {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerCopyPath {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerCopyRelativePath {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerRename {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerDelete {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerDuplicate {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerFindInFolder {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerOpenInTerminal {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerCut;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerCopy;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerPaste;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitStageFile {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitUnstageFile {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitDiscardFile {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitOpenDiff {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitOpenFile {
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct SwitchTerminalTab {
    pub index: usize,
}

/// Open the details (message + changed files + diff) of a commit in the
/// history graph.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitSelectCommit {
    pub sha: String,
}

/// Copy a commit's full SHA to the clipboard (history right-click menu).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitCopySha {
    pub sha: String,
}

/// Copy a commit's subject line to the clipboard (history right-click menu).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitCopyCommitMessage {
    pub sha: String,
}

/// Check out a commit (detached HEAD) from the history right-click menu.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitCheckoutCommit {
    pub sha: String,
}

/// Open a commit's full diff in a read-only diff tab (history right-click).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitViewCommitDiff {
    pub sha: String,
}
