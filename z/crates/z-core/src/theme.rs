/// Theme system for z TUI.
///
/// Themes are embedded in the binary as constants. The user selects a theme
/// by name in `~/.config/z/config.kdl` via `theme "dracula"`.

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
}

impl ThemeName {
    /// Parse a theme name from a string.
    pub fn from_str(s: &str) -> Option<ThemeName> {
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
        let bg = Rgb(40, 42, 54);         // #282a36
        let fg = Rgb(248, 248, 242);      // #f8f8f2
        let current = Rgb(68, 71, 90);    // #44475a
        let comment = Rgb(98, 114, 164);  // #6272a4
        let cyan = Rgb(139, 233, 253);    // #8be9fd
        let green = Rgb(80, 250, 123);    // #50fa7b
        let orange = Rgb(255, 184, 108);  // #ffb86c
        let pink = Rgb(255, 121, 198);    // #ff79c6
        let purple = Rgb(189, 147, 249);  // #bd93f9
        let red = Rgb(255, 85, 85);       // #ff5555
        let yellow = Rgb(241, 250, 140);  // #f1fa8c

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
        }
    }
}

impl Rgb {
    fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

impl Theme {
    /// Generate a Zellij KDL `themes {}` block and `theme "name"` directive
    /// so Zellij sessions inherit the same color scheme as the z TUI.
    pub fn to_zellij_kdl(&self) -> String {
        let name = &self.name;
        // Map theme fields to Zellij's named color slots
        let fg = self.foreground.to_hex();
        let bg = self.background.to_hex();
        let black = self.background.to_hex(); // close enough for terminal "black"
        let red = self.indicator_error.fg.unwrap_or(self.foreground).to_hex();
        let green = self.indicator_active.fg.unwrap_or(self.foreground).to_hex();
        let yellow = self.indicator_warning.fg.unwrap_or(self.foreground).to_hex();
        let blue = self.border_focused.fg.unwrap_or(self.foreground).to_hex();
        let magenta = self.title.fg.unwrap_or(self.foreground).to_hex();
        let cyan = self.indicator_info.fg.unwrap_or(self.foreground).to_hex();
        let white = self.foreground.to_hex();
        let orange = self.text_highlight.fg.unwrap_or(self.foreground).to_hex();

        format!(
            "\
themes {{\n\
    {name} {{\n\
        fg \"{fg}\"\n\
        bg \"{bg}\"\n\
        black \"{black}\"\n\
        red \"{red}\"\n\
        green \"{green}\"\n\
        yellow \"{yellow}\"\n\
        blue \"{blue}\"\n\
        magenta \"{magenta}\"\n\
        cyan \"{cyan}\"\n\
        white \"{white}\"\n\
        orange \"{orange}\"\n\
    }}\n\
}}\n\
theme \"{name}\"\n"
        )
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
        assert_eq!(ThemeName::from_str("dracula"), Some(ThemeName::Dracula));
    }

    #[test]
    fn theme_name_from_str_unknown() {
        assert_eq!(ThemeName::from_str("nord"), None);
    }

    #[test]
    fn theme_name_from_str_empty() {
        assert_eq!(ThemeName::from_str(""), None);
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
        assert!(kdl.contains("dracula {"), "should contain dracula sub-block");
        assert!(kdl.contains("theme \"dracula\""), "should set active theme");
    }

    #[test]
    fn dracula_zellij_theme_kdl_contains_all_colors() {
        let t = Theme::from_name(ThemeName::Dracula);
        let kdl = t.to_zellij_kdl();
        assert!(kdl.contains("fg \"#f8f8f2\""), "should have fg");
        assert!(kdl.contains("bg \"#282a36\""), "should have bg");
        assert!(kdl.contains("red \"#ff5555\""), "should have red");
        assert!(kdl.contains("green \"#50fa7b\""), "should have green");
        assert!(kdl.contains("yellow \"#f1fa8c\""), "should have yellow");
        assert!(kdl.contains("blue \"#bd93f9\""), "should have blue");
        assert!(kdl.contains("magenta \"#ff79c6\""), "should have magenta");
        assert!(kdl.contains("cyan \"#8be9fd\""), "should have cyan");
        assert!(kdl.contains("orange \"#ffb86c\""), "should have orange");
    }
}
