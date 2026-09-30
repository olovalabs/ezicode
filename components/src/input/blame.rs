use gpui::SharedString;

/// A single line's blame annotation, pre-formatted by the application layer so
/// this widget stays completely independent of any git implementation. Mirrors
/// what Zed renders as inline blame ghost text.
#[derive(Clone, Debug, Default)]
pub struct BlameLine {
    /// The ghost text, e.g. `"Ada Lovelace, 3 days ago"` — optionally with the
    /// commit summary appended (`" - Fixed the thing"`), matching Zed.
    pub text: SharedString,
    /// Abbreviated commit SHA (empty for not-yet-committed lines). Used as the
    /// hover/tooltip key.
    pub sha: SharedString,
    /// Remote avatar URL for the line's author (Zed's inline gutter
    /// avatar). Empty when the repo has no avatar-supporting remote.
    pub avatar_url: SharedString,
}

/// Full commit detail for the blame hover popover. Like [`BlameLine`], this is
/// produced by the application layer (lazily, on hover) so the widget stays
/// git-agnostic; it is pushed in via
/// [`crate::input::InputState::set_blame_detail`].
#[derive(Clone, Debug, Default)]
pub struct BlameDetail {
    /// Full 40-char SHA (the source of truth for copy / open actions).
    pub sha: SharedString,
    /// Abbreviated SHA shown in the popover.
    pub short_sha: SharedString,
    pub author: SharedString,
    pub author_email: SharedString,
    /// Remote avatar URL for the author (Zed's blame popover). Empty
    /// when the repo has no avatar-supporting remote or the author
    /// has no resolvable avatar.
    pub avatar_url: SharedString,
    /// Pre-formatted absolute commit date, e.g. `"Mon, 3 Feb 2025 14:02"`.
    pub date: SharedString,
    /// Full commit message (subject + body).
    pub message: SharedString,
}

/// Per-buffer inline git blame state, mirroring Zed's inline blame plus the
/// gutter-blame toggle. The application computes the annotations off-thread and
/// pushes them in via [`crate::input::InputState::set_inline_blame`].
#[derive(Clone, Debug, Default)]
pub struct InlineBlame {
    /// Annotation per row (index = 0-based line). `None` = no annotation for
    /// that line (e.g. freshly typed, uncommitted-and-hidden, or unknown).
    pub lines: Vec<Option<BlameLine>>,
    /// Master switch (`git.inline_blame.enabled`): show the annotation on the
    /// cursor's line.
    pub enabled: bool,
    /// `git.inline_blame.min_column` — never start the annotation before this
    /// column, so short lines don't get the hint jammed against the text.
    pub min_column: u32,
    /// `git.inline_blame.padding` — columns between the end of the
    /// line and the annotation.
    pub padding: u32,
    /// When true (the "Toggle Git Blame" command), show the annotation on every
    /// visible line, not only the cursor's line.
    pub show_all: bool,
    /// Asset path of the small git icon drawn before the annotation. Empty to
    /// draw no icon. Kept here so the widget never hard-codes an app asset.
    pub icon: SharedString,
    /// Row whose annotation the mouse is currently hovering (drives the commit
    /// detail popover). `None` when the mouse is not over any annotation.
    pub hovered_row: Option<usize>,
    /// Lazily-provided full commit detail for the hovered annotation. Set by the
    /// app after it fetches (and caches) the commit; cleared when the hover ends.
    pub detail: Option<BlameDetail>,
}

impl InlineBlame {
    /// The annotation for `row` (0-based), if any.
    pub fn line(&self, row: usize) -> Option<&BlameLine> {
        self.lines.get(row).and_then(|l| l.as_ref())
    }

    /// Whether the current-cursor-line annotation should be drawn.
    pub fn shows_current_line(&self) -> bool {
        self.enabled && !self.lines.is_empty()
    }

    /// Whether every visible line's annotation should be drawn (gutter toggle).
    pub fn shows_all_lines(&self) -> bool {
        self.show_all && !self.lines.is_empty()
    }
}
