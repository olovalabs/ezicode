use std::path::Path;

use gpui::{
    div, img, px, rgba, Context, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled,
};

use crate::theme::Colors;
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreadcrumbKind {
    Folder,
    File,
    Symbol,
}

#[derive(Clone, Debug)]
pub struct BreadcrumbItem {
    pub label: String,
    pub kind: BreadcrumbKind,
    pub icon: Option<String>,
    pub line: Option<usize>, // 1-based line number for jump navigation
}

/// Parses hierarchical Markdown headings (#, ##, ###) and finds the chain
/// enclosing the given cursor_line (1-based).
pub fn extract_markdown_headings(text: &str, cursor_line: usize) -> Vec<BreadcrumbItem> {
    let mut heading_stack: Vec<(usize, usize, String)> = Vec::new(); // (level, line, title)

    for (idx, line) in text.lines().enumerate() {
        let line_num = idx + 1;
        if line_num > cursor_line {
            break;
        }

        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|&c| c == '#').count();
            if hashes >= 1 && hashes <= 6 {
                let rest = trimmed[hashes..].trim();
                if !rest.is_empty() {
                    let heading_title = trimmed.to_string();

                    // Pop headings of same or deeper level
                    while let Some(&(prev_level, _, _)) = heading_stack.last() {
                        if prev_level >= hashes {
                            heading_stack.pop();
                        } else {
                            break;
                        }
                    }

                    heading_stack.push((hashes, line_num, heading_title));
                }
            }
        }
    }

    heading_stack
        .into_iter()
        .map(|(_, line_num, title)| BreadcrumbItem {
            label: title,
            kind: BreadcrumbKind::Symbol,
            icon: None,
            line: Some(line_num),
        })
        .collect()
}

/// Parses code symbols (functions, structs, classes, impl blocks) enclosing cursor_line.
pub fn extract_code_symbols(text: &str, cursor_line: usize, ext: &str) -> Vec<BreadcrumbItem> {
    let mut symbols: Vec<(usize, String)> = Vec::new(); // (line, title)

    for (idx, line) in text.lines().enumerate() {
        let line_num = idx + 1;
        if line_num > cursor_line {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }

        let matched_symbol = match ext {
            "rs" => {
                if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") || trimmed.starts_with("pub(crate) fn ") || trimmed.starts_with("async fn ") || trimmed.starts_with("pub async fn ") {
                    extract_name(trimmed, "fn ")
                } else if trimmed.starts_with("impl ") || trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") || trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") || trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
                    Some(clean_declaration(trimmed))
                } else if trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") {
                    Some(clean_declaration(trimmed))
                } else {
                    None
                }
            }
            "ts" | "tsx" | "js" | "jsx" => {
                if trimmed.starts_with("function ") || trimmed.starts_with("export function ") || trimmed.starts_with("async function ") || trimmed.starts_with("export async function ") {
                    extract_name(trimmed, "function ")
                } else if trimmed.starts_with("class ") || trimmed.starts_with("export class ") || trimmed.starts_with("interface ") || trimmed.starts_with("export interface ") || trimmed.starts_with("type ") || trimmed.starts_with("export type ") {
                    Some(clean_declaration(trimmed))
                } else if trimmed.starts_with("const ") && trimmed.contains(" = ") && (trimmed.contains("=>") || trimmed.contains("function")) {
                    extract_name(trimmed, "const ")
                } else {
                    None
                }
            }
            "py" => {
                if trimmed.starts_with("def ") {
                    extract_name(trimmed, "def ")
                } else if trimmed.starts_with("class ") {
                    extract_name(trimmed, "class ")
                } else {
                    None
                }
            }
            "go" => {
                if trimmed.starts_with("func ") {
                    extract_name(trimmed, "func ")
                } else if trimmed.starts_with("type ") {
                    Some(clean_declaration(trimmed))
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(sym) = matched_symbol {
            symbols.push((line_num, sym));
        }
    }

    if symbols.is_empty() {
        return Vec::new();
    }

    let keep_count = symbols.len().min(2);
    let slice = &symbols[symbols.len() - keep_count..];

    slice
        .iter()
        .map(|(line_num, title)| BreadcrumbItem {
            label: title.clone(),
            kind: BreadcrumbKind::Symbol,
            icon: None,
            line: Some(*line_num),
        })
        .collect()
}

fn extract_name(line: &str, keyword: &str) -> Option<String> {
    let after = line.split(keyword).nth(1)?.trim();
    let name = after
        .split(|c: char| c == '(' || c == ':' || c == '{' || c == '<' || c == ' ')
        .next()?
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(format!("{}{}", keyword.trim(), if keyword.ends_with(' ') { " " } else { "" }) + name)
    }
}

fn clean_declaration(line: &str) -> String {
    let cleaned = line
        .split('{')
        .next()
        .unwrap_or(line)
        .trim_end_matches(';')
        .trim();
    if cleaned.len() > 40 {
        format!("{}…", &cleaned[..38])
    } else {
        cleaned.to_string()
    }
}

/// Builds the full list of breadcrumb items for a given file and cursor line.
pub fn extract_breadcrumbs(
    path: &Path,
    text: &str,
    cursor_line: usize,
    root: Option<&Path>,
) -> Vec<BreadcrumbItem> {
    let mut items = Vec::new();

    // 1. Path folders
    let relative = if let Some(r) = root {
        path.strip_prefix(r).unwrap_or(path)
    } else {
        path
    };

    if let Some(parent) = relative.parent() {
        for component in parent.iter() {
            let name = component.to_string_lossy().to_string();
            if !name.is_empty() && name != "." {
                items.push(BreadcrumbItem {
                    label: name,
                    kind: BreadcrumbKind::Folder,
                    icon: None,
                    line: None,
                });
            }
        }
    }

    // 2. File itself with official icon
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string());
    let icon_path = crate::file_icons::icon_for(path).to_string();

    items.push(BreadcrumbItem {
        label: file_name,
        kind: BreadcrumbKind::File,
        icon: Some(icon_path),
        line: Some(1),
    });

    // 3. Document outline / symbols
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "md" || ext == "markdown" {
        items.extend(extract_markdown_headings(text, cursor_line));
    } else if !ext.is_empty() {
        items.extend(extract_code_symbols(text, cursor_line, &ext));
    }

    items
}

/// Renders the Breadcrumbs bar directly below the tab bar.
pub fn render_breadcrumbs(
    items: &[BreadcrumbItem],
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let mut bar = div()
        .id("breadcrumbs-bar")
        .w_full()
        .h(px(24.0))
        .px(px(10.0))
        .flex()
        .flex_row()
        .items_center()
        .bg(rgba(t.editor_bg))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .overflow_x_hidden();

    for (ix, item) in items.iter().enumerate() {
        let is_last = ix == items.len() - 1;
        let target_line = item.line;
        let label = item.label.clone();
        let icon = item.icon.clone();
        let kind = item.kind;

        let text_color = if is_last {
            rgba(0xf0f6fcff)
        } else {
            rgba(0x8b949eff)
        };

        let mut item_el = div()
            .id(SharedString::from(format!("breadcrumb-{ix}")))
            .h(px(20.0))
            .px(px(4.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded(px(3.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(t.element_hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(line) = target_line {
                    this.jump_to_line(line, window, cx);
                } else {
                    this.toggle_file_finder(window, cx);
                }
            }));

        // Render Icon / Symbol badge
        if let Some(ic) = icon {
            item_el = item_el.child(
                div()
                    .size(px(14.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(img(SharedString::from(ic)).size(px(14.0))),
            );
        } else if kind == BreadcrumbKind::Symbol {
            // [abc] style symbol badge matching the VS Code reference screenshot
            item_el = item_el.child(
                div()
                    .px(px(3.0))
                    .py(px(0.5))
                    .rounded(px(2.5))
                    .border_1()
                    .border_color(rgba(0x388bfd66))
                    .bg(rgba(0x388bfd18))
                    .text_size(px(9.5))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgba(0x58a6ffff))
                    .flex_none()
                    .child("abc"),
            );
        }

        item_el = item_el.child(
            div()
                .text_size(px(12.0))
                .text_color(text_color)
                .whitespace_nowrap()
                .child(label),
        );

        bar = bar.child(item_el);

        // Separator '>'
        if !is_last {
            bar = bar.child(
                div()
                    .px(px(3.0))
                    .text_size(px(11.0))
                    .text_color(rgba(0x6e7681ff))
                    .flex_none()
                    .child("›"),
            );
        }
    }

    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_heading_extraction() {
        let md = r#"# Zed Workspace Packages

Total packages: **252**

## Tooling (3)

Notes on tooling.

### xtask

xtask details.
"#;
        // Line 1 is the H1
        let h_line1 = extract_markdown_headings(md, 1);
        assert_eq!(h_line1.len(), 1);
        assert_eq!(h_line1[0].label, "# Zed Workspace Packages");

        // Line 5 is inside H2
        let h_line5 = extract_markdown_headings(md, 6);
        assert_eq!(h_line5.len(), 2);
        assert_eq!(h_line5[0].label, "# Zed Workspace Packages");
        assert_eq!(h_line5[1].label, "## Tooling (3)");

        // Line 10 is inside H3
        let h_line10 = extract_markdown_headings(md, 11);
        assert_eq!(h_line10.len(), 3);
        assert_eq!(h_line10[2].label, "### xtask");
    }

    #[test]
    fn test_code_symbol_extraction() {
        let rust_code = r#"
pub struct PickerState {
    pub kind: PickerKind,
}

impl PickerState {
    pub fn new() -> Self {
        Self { kind: PickerKind::FileFinder }
    }
}
"#;
        let syms = extract_code_symbols(rust_code, 8, "rs");
        assert!(!syms.is_empty());
        assert!(syms.iter().any(|s| s.label.contains("impl PickerState") || s.label.contains("fn new")));
    }
}
