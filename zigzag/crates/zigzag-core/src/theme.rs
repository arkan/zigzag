//! Theme system for Zigzag TUI.
//!
//! Themes are embedded in the binary as constants. The user selects a theme
//! by name in `~/.config/zigzag/config.kdl` via `theme "dracula"`.

/// RGB color triple, framework-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A style combining optional foreground, background, bold and dim flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeStyle {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub dim: bool,
}

/// Available built-in theme names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeName {
    #[default]
    Dracula,
}

/// Complete theme definition for the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub background: Rgb,
    pub foreground: Rgb,

    // Panels
    pub border_focused: ThemeStyle,
    pub border_unfocused: ThemeStyle,
    pub title: ThemeStyle,

    // Lists
    pub item_selected_focused: ThemeStyle,
    pub item_selected_unfocused: ThemeStyle,
    pub item_normal: ThemeStyle,
    pub highlight_symbol: String,

    // Preview
    pub preview_text: ThemeStyle,
    pub preview_label: ThemeStyle,

    // Status bar
    pub status_text: ThemeStyle,
    pub status_key: ThemeStyle,

    // Indicators
    pub indicator_active: ThemeStyle,
    pub indicator_error: ThemeStyle,
    pub indicator_warning: ThemeStyle,
    pub indicator_info: ThemeStyle,

    // Modals
    pub modal_border: ThemeStyle,
    pub modal_background: Rgb,
    pub modal_title: ThemeStyle,

    // Logs
    pub log_error: ThemeStyle,
    pub log_warning: ThemeStyle,
    pub log_default: ThemeStyle,

    // Text
    pub text_dim: ThemeStyle,
    pub text_highlight: ThemeStyle,

    // Terminal ANSI palette (used by Zellij theme)
    pub terminal_black: Rgb,
    pub terminal_red: Rgb,
    pub terminal_green: Rgb,
    pub terminal_yellow: Rgb,
    pub terminal_blue: Rgb,
    pub terminal_magenta: Rgb,
    pub terminal_cyan: Rgb,
    pub terminal_white: Rgb,
    pub terminal_orange: Rgb,
}

impl ThemeName {
    /// Backwards-compatible alias for callers that used the pre-Rust-1.95 helper.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<ThemeName> {
        Self::parse_str(s)
    }

    /// Parse a theme name from a string.
    pub fn parse_str(s: &str) -> Option<ThemeName> {
        match s {
            "dracula" => Some(ThemeName::Dracula),
            _ => None,
        }
    }
}

impl Theme {
    /// Build a complete theme from a built-in name.
    pub fn from_name(name: ThemeName) -> Theme {
        match name {
            ThemeName::Dracula => Self::dracula(),
        }
    }

    fn dracula() -> Theme {
        // Dracula palette
        let bg = Rgb(40, 42, 54); // #282a36
        let fg = Rgb(248, 248, 242); // #f8f8f2
        let current = Rgb(68, 71, 90); // #44475a
        let comment = Rgb(98, 114, 164); // #6272a4
        let cyan = Rgb(139, 233, 253); // #8be9fd
        let green = Rgb(80, 250, 123); // #50fa7b
        let orange = Rgb(255, 184, 108); // #ffb86c
        let pink = Rgb(255, 121, 198); // #ff79c6
        let purple = Rgb(189, 147, 249); // #bd93f9
        let red = Rgb(255, 85, 85); // #ff5555
        let yellow = Rgb(241, 250, 140); // #f1fa8c

        let s = |fg_c: Option<Rgb>, bg_c: Option<Rgb>, bold: bool, dim: bool| ThemeStyle {
            fg: fg_c,
            bg: bg_c,
            bold,
            dim,
        };

        Theme {
            name: "dracula".to_string(),
            background: bg,
            foreground: fg,

            border_focused: s(Some(purple), None, true, false),
            border_unfocused: s(Some(comment), None, false, false),
            title: s(Some(pink), None, true, false),

            item_selected_focused: s(Some(purple), Some(current), true, false),
            item_selected_unfocused: s(Some(comment), Some(current), false, false),
            item_normal: s(Some(fg), None, false, false),
            highlight_symbol: "▸ ".to_string(),

            preview_text: s(Some(fg), None, false, false),
            preview_label: s(Some(cyan), None, true, false),

            status_text: s(Some(comment), None, false, false),
            status_key: s(Some(fg), None, true, false),

            indicator_active: s(Some(green), None, false, false),
            indicator_error: s(Some(red), None, false, false),
            indicator_warning: s(Some(yellow), None, false, false),
            indicator_info: s(Some(cyan), None, false, false),

            modal_border: s(Some(purple), None, true, false),
            modal_background: bg,
            modal_title: s(Some(pink), None, true, false),

            log_error: s(Some(red), None, false, false),
            log_warning: s(Some(yellow), None, false, false),
            log_default: s(Some(comment), None, false, false),

            text_dim: s(Some(comment), None, false, true),
            text_highlight: s(Some(orange), None, true, false),

            terminal_black: Rgb(33, 34, 44), // #21222c
            terminal_red: red,
            terminal_green: green,
            terminal_yellow: yellow,
            terminal_blue: purple, // Dracula uses purple for "blue" slot
            terminal_magenta: pink,
            terminal_cyan: cyan,
            terminal_white: fg,
            terminal_orange: orange,
        }
    }
}

impl Theme {
    /// Generate a Zellij KDL `themes {}` block and `theme "name"` directive
    /// using the modern structured format (Zellij 0.41+).
    pub fn to_zellij_kdl(&self) -> String {
        let name = &self.name;
        match ThemeName::parse_str(name) {
            Some(ThemeName::Dracula) => Self::dracula_zellij_kdl(),
            None => Self::dracula_zellij_kdl(), // fallback
        }
    }

    /// Official Dracula theme for Zellij in the modern structured format.
    fn dracula_zellij_kdl() -> String {
        // Colors: orange=255,184,108 cyan=139,233,253 green=80,250,123
        //         pink=255,121,198 red=255,85,85 yellow=241,250,140
        //         comment=98,114,164 fg=248,248,242 bg=40,42,54
        "\
themes {\n\
    dracula {\n\
        text_unselected {\n\
            base 255 255 255\n\
            background 0 0 0\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        text_selected {\n\
            base 255 255 255\n\
            background 40 42 54\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        ribbon_selected {\n\
            base 0 0 0\n\
            background 80 250 123\n\
            emphasis_0 255 85 85\n\
            emphasis_1 255 184 108\n\
            emphasis_2 255 121 198\n\
            emphasis_3 98 114 164\n\
        }\n\
        ribbon_unselected {\n\
            base 0 0 0\n\
            background 248 248 242\n\
            emphasis_0 255 85 85\n\
            emphasis_1 255 255 255\n\
            emphasis_2 98 114 164\n\
            emphasis_3 255 121 198\n\
        }\n\
        table_title {\n\
            base 80 250 123\n\
            background 0\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        table_cell_selected {\n\
            base 255 255 255\n\
            background 40 42 54\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        table_cell_unselected {\n\
            base 255 255 255\n\
            background 0 0 0\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        list_selected {\n\
            base 255 255 255\n\
            background 40 42 54\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        list_unselected {\n\
            base 255 255 255\n\
            background 0 0 0\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 80 250 123\n\
            emphasis_3 255 121 198\n\
        }\n\
        frame_selected {\n\
            base 80 250 123\n\
            background 0\n\
            emphasis_0 255 184 108\n\
            emphasis_1 139 233 253\n\
            emphasis_2 255 121 198\n\
            emphasis_3 0\n\
        }\n\
        frame_highlight {\n\
            base 255 184 108\n\
            background 0\n\
            emphasis_0 255 121 198\n\
            emphasis_1 255 184 108\n\
            emphasis_2 255 184 108\n\
            emphasis_3 255 184 108\n\
        }\n\
        exit_code_success {\n\
            base 80 250 123\n\
            background 0\n\
            emphasis_0 139 233 253\n\
            emphasis_1 0 0 0\n\
            emphasis_2 255 121 198\n\
            emphasis_3 98 114 164\n\
        }\n\
        exit_code_error {\n\
            base 255 85 85\n\
            background 0\n\
            emphasis_0 241 250 140\n\
            emphasis_1 0\n\
            emphasis_2 0\n\
            emphasis_3 0\n\
        }\n\
        multiplayer_user_colors {\n\
            player_1 255 121 198\n\
            player_2 98 114 164\n\
            player_3 0\n\
            player_4 241 250 140\n\
            player_5 139 233 253\n\
            player_6 0\n\
            player_7 255 85 85\n\
            player_8 0\n\
            player_9 0\n\
            player_10 0\n\
        }\n\
    }\n\
}\n\
theme \"dracula\"\n"
            .to_string()
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from_name(ThemeName::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dracula_has_correct_background() {
        let theme = Theme::from_name(ThemeName::Dracula);
        assert_eq!(theme.background, Rgb(40, 42, 54));
    }

    #[test]
    fn theme_name_from_str_dracula() {
        assert_eq!(ThemeName::parse_str("dracula"), Some(ThemeName::Dracula));
    }

    #[test]
    fn theme_name_from_str_unknown() {
        assert_eq!(ThemeName::parse_str("nord"), None);
    }

    #[test]
    fn theme_name_from_str_empty() {
        assert_eq!(ThemeName::parse_str(""), None);
    }

    #[test]
    fn dracula_has_all_dracula_colors() {
        let t = Theme::from_name(ThemeName::Dracula);

        assert_eq!(t.foreground, Rgb(248, 248, 242));
        assert_eq!(t.name, "dracula");

        // Selected focused uses purple fg + current-line bg
        assert_eq!(t.item_selected_focused.fg, Some(Rgb(189, 147, 249)));
        assert_eq!(t.item_selected_focused.bg, Some(Rgb(68, 71, 90)));
        assert!(t.item_selected_focused.bold);

        // Borders: focused = purple, unfocused = comment
        assert_eq!(t.border_focused.fg, Some(Rgb(189, 147, 249)));
        assert_eq!(t.border_unfocused.fg, Some(Rgb(98, 114, 164)));

        // Titles = pink
        assert_eq!(t.title.fg, Some(Rgb(255, 121, 198)));

        // Indicators
        assert_eq!(t.indicator_active.fg, Some(Rgb(80, 250, 123)));
        assert_eq!(t.indicator_error.fg, Some(Rgb(255, 85, 85)));
        assert_eq!(t.indicator_warning.fg, Some(Rgb(241, 250, 140)));
    }

    #[test]
    fn default_theme_is_dracula() {
        let t = Theme::default();
        assert_eq!(t.name, "dracula");
        assert_eq!(t.background, Rgb(40, 42, 54));
    }

    #[test]
    fn dracula_zellij_theme_kdl_contains_theme_block() {
        let t = Theme::from_name(ThemeName::Dracula);
        let kdl = t.to_zellij_kdl();
        assert!(kdl.contains("themes {"), "should contain themes block");
        assert!(
            kdl.contains("dracula {"),
            "should contain dracula sub-block"
        );
        assert!(kdl.contains("theme \"dracula\""), "should set active theme");
    }

    #[test]
    fn dracula_zellij_theme_kdl_uses_modern_format() {
        let t = Theme::from_name(ThemeName::Dracula);
        let kdl = t.to_zellij_kdl();
        // Modern Zellij theme format uses structured blocks
        assert!(
            kdl.contains("ribbon_selected {"),
            "should have ribbon_selected"
        );
        assert!(
            kdl.contains("ribbon_unselected {"),
            "should have ribbon_unselected"
        );
        assert!(kdl.contains("text_selected {"), "should have text_selected");
        assert!(
            kdl.contains("text_unselected {"),
            "should have text_unselected"
        );
        assert!(
            kdl.contains("frame_selected {"),
            "should have frame_selected"
        );
        assert!(
            kdl.contains("exit_code_success {"),
            "should have exit_code_success"
        );
        // Uses RGB values, not hex strings
        assert!(kdl.contains("base 80 250 123"), "should use RGB for green");
        assert!(
            kdl.contains("background 40 42 54"),
            "should use Dracula bg RGB"
        );
    }
}
