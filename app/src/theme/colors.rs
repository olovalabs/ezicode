#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub background: u32,
    pub surface: u32,
    pub elevated_surface: u32,
    pub element_bg: u32,
    pub element_hover: u32,
    pub element_active: u32,
    pub element_selected: u32,
    pub ghost_hover: u32,
    pub ghost_active: u32,
    pub border: u32,
    pub border_variant: u32,
    pub border_focused: u32,
    pub text: u32,
    pub text_muted: u32,
    pub text_accent: u32,
    pub icon: u32,
    pub icon_muted: u32,
    pub icon_accent: u32,
    pub title_bar: u32,
    pub status_bar: u32,
    pub panel: u32,
    pub toolbar: u32,
    pub tab_bar: u32,
    pub tab_active_bg: u32,
    pub tab_inactive_bg: u32,
    pub tab_active_fg: u32,
    pub tab_inactive_fg: u32,
    pub editor_bg: u32,
    pub editor_fg: u32,
    pub terminal_bg: u32,
    pub vc_added: u32,
    pub vc_modified: u32,
    pub vc_deleted: u32,
}

impl Colors {

    pub(crate) const MISSING: u32 = 0xFF00FF;

    pub(crate) fn all_missing() -> Self {
        Self {
            background: Self::MISSING,
            surface: Self::MISSING,
            elevated_surface: Self::MISSING,
            element_bg: Self::MISSING,
            element_hover: Self::MISSING,
            element_active: Self::MISSING,
            element_selected: Self::MISSING,
            ghost_hover: Self::MISSING,
            ghost_active: Self::MISSING,
            border: Self::MISSING,
            border_variant: Self::MISSING,
            border_focused: Self::MISSING,
            text: Self::MISSING,
            text_muted: Self::MISSING,
            text_accent: Self::MISSING,
            icon: Self::MISSING,
            icon_muted: Self::MISSING,
            icon_accent: Self::MISSING,
            title_bar: Self::MISSING,
            status_bar: Self::MISSING,
            panel: Self::MISSING,
            toolbar: Self::MISSING,
            tab_bar: Self::MISSING,
            tab_active_bg: Self::MISSING,
            tab_inactive_bg: Self::MISSING,
            tab_active_fg: Self::MISSING,
            tab_inactive_fg: Self::MISSING,
            editor_bg: Self::MISSING,
            editor_fg: Self::MISSING,
            terminal_bg: Self::MISSING,
            vc_added: Self::MISSING,
            vc_modified: Self::MISSING,
            vc_deleted: Self::MISSING,
        }
    }

    #[cfg(test)]
    fn values(&self) -> [u32; 33] {
        [
            self.background,
            self.surface,
            self.elevated_surface,
            self.element_bg,
            self.element_hover,
            self.element_active,
            self.element_selected,
            self.ghost_hover,
            self.ghost_active,
            self.border,
            self.border_variant,
            self.border_focused,
            self.text,
            self.text_muted,
            self.text_accent,
            self.icon,
            self.icon_muted,
            self.icon_accent,
            self.title_bar,
            self.status_bar,
            self.panel,
            self.toolbar,
            self.tab_bar,
            self.tab_active_bg,
            self.tab_inactive_bg,
            self.tab_active_fg,
            self.tab_inactive_fg,
            self.editor_bg,
            self.editor_fg,
            self.terminal_bg,
            self.vc_added,
            self.vc_modified,
            self.vc_deleted,
        ]
    }

    pub(crate) fn get_mut(&mut self, field: &str) -> Option<&mut u32> {
        match field {
            "background" => Some(&mut self.background),
            "surface" => Some(&mut self.surface),
            "elevated_surface" => Some(&mut self.elevated_surface),
            "element_bg" => Some(&mut self.element_bg),
            "element_hover" => Some(&mut self.element_hover),
            "element_active" => Some(&mut self.element_active),
            "element_selected" => Some(&mut self.element_selected),
            "ghost_hover" => Some(&mut self.ghost_hover),
            "ghost_active" => Some(&mut self.ghost_active),
            "border" => Some(&mut self.border),
            "border_variant" => Some(&mut self.border_variant),
            "border_focused" => Some(&mut self.border_focused),
            "text" => Some(&mut self.text),
            "text_muted" => Some(&mut self.text_muted),
            "text_accent" => Some(&mut self.text_accent),
            "icon" => Some(&mut self.icon),
            "icon_muted" => Some(&mut self.icon_muted),
            "icon_accent" => Some(&mut self.icon_accent),
            "title_bar" => Some(&mut self.title_bar),
            "status_bar" => Some(&mut self.status_bar),
            "panel" => Some(&mut self.panel),
            "toolbar" => Some(&mut self.toolbar),
            "tab_bar" => Some(&mut self.tab_bar),
            "tab_active_bg" => Some(&mut self.tab_active_bg),
            "tab_inactive_bg" => Some(&mut self.tab_inactive_bg),
            "tab_active_fg" => Some(&mut self.tab_active_fg),
            "tab_inactive_fg" => Some(&mut self.tab_inactive_fg),
            "editor_bg" => Some(&mut self.editor_bg),
            "editor_fg" => Some(&mut self.editor_fg),
            "terminal_bg" => Some(&mut self.terminal_bg),
            "vc_added" => Some(&mut self.vc_added),
            "vc_modified" => Some(&mut self.vc_modified),
            "vc_deleted" => Some(&mut self.vc_deleted),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_complete(&self) -> bool {
        !self.values().contains(&Self::MISSING)
    }
}

pub(crate) const KEY_MAP: &[(&str, &str)] = &[
    ("background", "background"),
    ("surface", "surface.background"),
    ("elevated_surface", "elevated_surface.background"),
    ("element_bg", "element.background"),
    ("element_hover", "element.hover"),
    ("element_active", "element.active"),
    ("element_selected", "element.selected"),
    ("ghost_hover", "ghost_element.hover"),
    ("ghost_active", "ghost_element.active"),
    ("border", "border"),
    ("border_variant", "border.variant"),
    ("border_focused", "border.focused"),
    ("text", "text"),
    ("text_muted", "text.muted"),
    ("text_accent", "text.accent"),
    ("icon", "icon"),
    ("icon_muted", "icon.muted"),
    ("icon_accent", "icon.accent"),
    ("title_bar", "title_bar.background"),
    ("status_bar", "status_bar.background"),
    ("panel", "panel.background"),
    ("toolbar", "toolbar.background"),
    ("tab_bar", "tab_bar.background"),
    ("tab_active_bg", "tab.active_background"),
    ("tab_inactive_bg", "tab.inactive_background"),
    ("tab_active_fg", "tab.active_foreground"),
    ("tab_inactive_fg", "tab.inactive_foreground"),
    ("editor_bg", "editor.background"),
    ("editor_fg", "editor.foreground"),
    ("terminal_bg", "terminal.background"),
    ("vc_added", "version_control.added"),
    ("vc_modified", "version_control.modified"),
    ("vc_deleted", "version_control.deleted"),
];

pub(crate) const FALLBACKS: &[(&str, u32)] = &[
    ("vc_added", 0x27a657ff),
    ("vc_modified", 0xd3b020ff),
    ("vc_deleted", 0xe06c76ff),
];

pub(crate) fn parse_hex(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#')?;
    if hex.len() == 6 {
        u32::from_str_radix(&format!("{hex}ff"), 16).ok()
    } else {
        u32::from_str_radix(hex, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_alpha() {
        assert_eq!(parse_hex("#282c34"), Some(0x282c34ff));
        assert_eq!(parse_hex("#83899480"), Some(0x83899480));
        assert_eq!(parse_hex("nope"), None);
    }
}
