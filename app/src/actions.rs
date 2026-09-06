//! User-intent actions ([`gpui::Action`]) dispatched by menus, the title bar
//! and keyboard shortcuts. They decouple "what the user asked for" from the
//! widget that observed the input.

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
        About,
        ExplorerRefresh,
        ExplorerCollapseAll,
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
        // Terminal-specific actions (Zed-style)
        NextTerminal,
        PrevTerminal,
        CloseTerminal,
        ClearTerminal,
        TerminalTab1,
        TerminalTab2,
        TerminalTab3,
        TerminalTab4,
        TerminalTab5
    ]
);

/// Select a specific tab by index (used by tab bar clicks)
#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct SwitchTab {
    pub index: usize,
}

/// Close a specific tab by index (used by tab close button clicks)
#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct CloseTabAt {
    pub index: usize,
}

/// Select a theme by index into `theme::all`. Payload action (not bound to
/// any keymap), dispatched from the View → Theme submenu.
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
pub struct ExplorerCut;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerCopy;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ExplorerPaste;

// -- Source control (Git) payload actions ------------------------------------

/// Stage one changed file (from the source-control panel).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitStageFile {
    pub path: PathBuf,
}

/// Unstage one file (from the source-control panel).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitUnstageFile {
    pub path: PathBuf,
}

/// Discard all worktree changes of one file (from the source-control panel).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitDiscardFile {
    pub path: PathBuf,
}

/// Open the diff of one changed file in an editor tab.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitOpenDiff {
    pub path: PathBuf,
}

/// Open a changed file in the editor (from the source-control panel).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct GitOpenFile {
    pub path: PathBuf,
}

/// Switch to a specific terminal tab by index (Alt+1..5 style shortcuts).
#[derive(Clone, Copy, Debug, Default, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct SwitchTerminalTab {
    pub index: usize,
}
