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
