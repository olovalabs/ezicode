//! Native Markdown preview parsing and presentation.
//!
//! The parser is deliberately independent of GPUI. `Workspace` owns the live
//! preview state and hands the immutable result to this module's thin view
//! adapter. This mirrors Zed's split between its `markdown` parser/model and
//! `markdown_preview` workspace item without importing Zed's internal crates.

use std::{ops::Range, path::Path, sync::Arc};

use gpui::{div, prelude::*, px, rgba, svg, Context, SharedString, Window};
use gpui_component::text::{TextView, TextViewStyle};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

use crate::{theme::Colors, workspace::Workspace};

pub const REPARSE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(75);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    CodeBlock(Option<String>),
    BlockQuote,
    List { start: Option<u64> },
    ListItem { checked: Option<bool> },
    Table,
    TableHead,
    TableRow,
    TableCell,
    FootnoteDefinition(String),
    Rule,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: Option<String>,
    pub image: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRun {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownBlock {
    pub kind: BlockKind,
    pub source: Range<usize>,
    pub source_line: usize,
    pub depth: usize,
    pub runs: Vec<InlineRun>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkdownDocument {
    pub blocks: Vec<MarkdownBlock>,
}

impl MarkdownDocument {
    /// Finds the rendered root block nearest to a zero-based source line.
    /// This source map is the same primitive Zed uses for editor/preview sync;
    /// Easy Code keeps it in the pure model so a richer scroll hook can use it.
    #[allow(dead_code)] // retained for editor/preview scroll synchronization
    pub fn block_for_source_line(&self, line: usize) -> Option<usize> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.depth == 0 && block.source_line <= line)
            .map(|(ix, _)| ix)
            .next_back()
            .or_else(|| (!self.blocks.is_empty()).then_some(0))
    }
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn source_line(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset.min(source.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

/// Parse GitHub-flavoured Markdown into an owned, render-ready model.
///
/// All strings are owned so the result can safely cross from GPUI's background
/// executor back to the UI thread.
pub fn parse(source: &str) -> MarkdownDocument {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES;
    let mut document = MarkdownDocument::default();
    let mut block_stack: Vec<usize> = Vec::new();
    let mut style_stack = vec![InlineStyle::default()];

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let block_kind = match &tag {
                    Tag::Paragraph => Some(BlockKind::Paragraph),
                    Tag::Heading { level, .. } => Some(BlockKind::Heading(heading_level(*level))),
                    Tag::CodeBlock(kind) => Some(BlockKind::CodeBlock(match kind {
                        CodeBlockKind::Indented => None,
                        CodeBlockKind::Fenced(language) => {
                            let language = language.split_whitespace().next().unwrap_or_default();
                            (!language.is_empty()).then(|| language.to_string())
                        }
                    })),
                    Tag::BlockQuote(_) => Some(BlockKind::BlockQuote),
                    Tag::List(start) => Some(BlockKind::List { start: *start }),
                    Tag::Item => Some(BlockKind::ListItem { checked: None }),
                    Tag::Table(_) => Some(BlockKind::Table),
                    Tag::TableHead => Some(BlockKind::TableHead),
                    Tag::TableRow => Some(BlockKind::TableRow),
                    Tag::TableCell => Some(BlockKind::TableCell),
                    Tag::FootnoteDefinition(label) => {
                        Some(BlockKind::FootnoteDefinition(label.to_string()))
                    }
                    _ => None,
                };

                if let Some(kind) = block_kind {
                    let index = document.blocks.len();
                    document.blocks.push(MarkdownBlock {
                        kind,
                        source: range.clone(),
                        source_line: source_line(source, range.start),
                        depth: block_stack.len(),
                        runs: Vec::new(),
                    });
                    block_stack.push(index);
                }

                let mut style = style_stack.last().cloned().unwrap_or_default();
                let changed = match tag {
                    Tag::Strong => {
                        style.strong = true;
                        true
                    }
                    Tag::Emphasis => {
                        style.emphasis = true;
                        true
                    }
                    Tag::Strikethrough => {
                        style.strikethrough = true;
                        true
                    }
                    Tag::Link { dest_url, .. } => {
                        style.link = Some(dest_url.to_string());
                        true
                    }
                    Tag::Image { dest_url, .. } => {
                        style.image = Some(dest_url.to_string());
                        true
                    }
                    _ => false,
                };
                if changed {
                    style_stack.push(style);
                }
            }
            Event::End(end) => {
                use pulldown_cmark::TagEnd;
                if matches!(
                    end,
                    TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_)
                        | TagEnd::List(_)
                        | TagEnd::Item
                        | TagEnd::Table
                        | TagEnd::TableHead
                        | TagEnd::TableRow
                        | TagEnd::TableCell
                        | TagEnd::FootnoteDefinition
                ) {
                    if let Some(index) = block_stack.pop() {
                        document.blocks[index].source.end = range.end;
                    }
                }
                if matches!(
                    end,
                    TagEnd::Strong
                        | TagEnd::Emphasis
                        | TagEnd::Strikethrough
                        | TagEnd::Link
                        | TagEnd::Image
                ) && style_stack.len() > 1
                {
                    style_stack.pop();
                }
            }
            Event::Text(text) => push_run(
                &mut document,
                &block_stack,
                text.as_ref(),
                style_stack.last().cloned().unwrap_or_default(),
            ),
            Event::Code(code) => {
                let mut style = style_stack.last().cloned().unwrap_or_default();
                style.code = true;
                push_run(&mut document, &block_stack, code.as_ref(), style);
            }
            Event::SoftBreak | Event::HardBreak => push_run(
                &mut document,
                &block_stack,
                "\n",
                style_stack.last().cloned().unwrap_or_default(),
            ),
            Event::TaskListMarker(checked) => {
                if let Some(index) = block_stack.iter().rev().find(|index| {
                    matches!(document.blocks[**index].kind, BlockKind::ListItem { .. })
                }) {
                    document.blocks[*index].kind = BlockKind::ListItem {
                        checked: Some(checked),
                    };
                }
            }
            Event::Rule => document.blocks.push(MarkdownBlock {
                kind: BlockKind::Rule,
                source: range.clone(),
                source_line: source_line(source, range.start),
                depth: block_stack.len(),
                runs: Vec::new(),
            }),
            Event::FootnoteReference(label) => push_run(
                &mut document,
                &block_stack,
                &format!("[^{label}]"),
                style_stack.last().cloned().unwrap_or_default(),
            ),
            Event::Html(html) | Event::InlineHtml(html) => push_run(
                &mut document,
                &block_stack,
                html.as_ref(),
                style_stack.last().cloned().unwrap_or_default(),
            ),
            _ => {}
        }
    }

    document
}

fn push_run(
    document: &mut MarkdownDocument,
    block_stack: &[usize],
    text: &str,
    style: InlineStyle,
) {
    let Some(index) = block_stack.last().copied() else {
        return;
    };
    let runs = &mut document.blocks[index].runs;
    if let Some(last) = runs.last_mut().filter(|last| last.style == style) {
        last.text.push_str(text);
    } else {
        runs.push(InlineRun {
            text: text.to_string(),
            style,
        });
    }
}

#[derive(Clone)]
pub(crate) struct MarkdownPreviewState {
    pub(crate) path: std::path::PathBuf,
    pub(crate) source: Arc<str>,
    pub(crate) document: Arc<MarkdownDocument>,
    pub(crate) generation: u64,
}

pub(crate) fn render_panel(
    preview: &MarkdownPreviewState,
    t: &Colors,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> gpui::AnyElement {
    let title = preview
        .path
        .file_name()
        .map(|name| format!("Preview: {}", name.to_string_lossy()))
        .unwrap_or_else(|| "Markdown Preview".to_string());
    let style = TextViewStyle {
        heading_base_font_size: px(15.0),
        highlight_theme: gpui_component::Theme::global(cx).highlight_theme.clone(),
        is_dark: gpui_component::Theme::global(cx).mode.is_dark(),
        ..Default::default()
    };

    div()
        .id("markdown-preview-panel")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(240.0))
        .h_full()
        .bg(rgba(t.editor_bg))
        .border_l_1()
        .border_color(rgba(t.border_variant))
        .child(
            div()
                .h(px(35.0))
                .flex_none()
                .px(px(12.0))
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(rgba(t.border_variant))
                .text_size(px(12.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(title))
                .child(
                    div()
                        .id("close-markdown-preview")
                        .size(px(22.0))
                        .rounded(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|button| button.bg(rgba(t.element_hover)))
                        .text_color(rgba(t.icon))
                        .on_click(cx.listener(|workspace, _, _, cx| {
                            workspace.close_markdown_preview(cx);
                        }))
                        .child("×"),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .p(px(20.0))
                .overflow_hidden()
                .child(
                    TextView::markdown(
                        SharedString::from(format!(
                            "live-markdown-preview:{}",
                            preview.path.display()
                        )),
                        SharedString::from(preview.source.to_string()),
                        window,
                        cx,
                    )
                    .style(style)
                    .selectable(true)
                    .scrollable(true),
                ),
        )
        .into_any_element()
}

pub(crate) fn toolbar_icon(t: &Colors) -> impl IntoElement {
    svg()
        .path("ui_icons/markdown-preview.svg")
        .size(px(18.0))
        .text_color(rgba(t.icon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enables_gfm_blocks_and_inline_styles() {
        let source = "# Title\n\n- [x] **done** and ~~old~~\n\n| A | B |\n| - | - |\n| `x` | y |\n";
        let document = parse(source);
        assert!(document
            .blocks
            .iter()
            .any(|b| b.kind == BlockKind::Heading(1)));
        assert!(document.blocks.iter().any(|b| b.kind
            == BlockKind::ListItem {
                checked: Some(true)
            }));
        assert!(document.blocks.iter().any(|b| b.kind == BlockKind::Table));
        assert!(document
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .any(|r| r.style.strong && r.text == "done"));
        assert!(document
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .any(|r| r.style.strikethrough && r.text == "old"));
        assert!(document
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .any(|r| r.style.code && r.text == "x"));
    }

    #[test]
    fn preserves_nested_lists_and_source_lines() {
        let document = parse("intro\n\n- outer\n  1. inner\n\nend\n");
        let lists: Vec<_> = document
            .blocks
            .iter()
            .filter(|block| matches!(block.kind, BlockKind::List { .. }))
            .collect();
        assert_eq!(lists.len(), 2);
        assert!(lists[1].depth > lists[0].depth);
        let mapped = document.block_for_source_line(3).unwrap();
        assert!(document.blocks[mapped].source_line <= 3);
    }

    #[test]
    fn captures_code_links_images_and_footnotes() {
        let source = "[link](https://example.com) ![alt](image.png)\n\n```rust\nfn main() {}\n```\n\nref[^a]\n\n[^a]: note";
        let document = parse(source);
        assert!(document
            .blocks
            .iter()
            .any(|b| b.kind == BlockKind::CodeBlock(Some("rust".into()))));
        assert!(document
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .any(|r| r.style.link.as_deref() == Some("https://example.com")));
        assert!(document
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .any(|r| r.style.image.as_deref() == Some("image.png")));
        assert!(document
            .blocks
            .iter()
            .any(|b| matches!(b.kind, BlockKind::FootnoteDefinition(_))));
    }

    #[test]
    fn recognizes_markdown_extensions_case_insensitively() {
        assert!(is_markdown_path(Path::new("README.MD")));
        assert!(is_markdown_path(Path::new("notes.markdown")));
        assert!(!is_markdown_path(Path::new("notes.txt")));
    }
}
