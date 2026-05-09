use crate::domain::{Layout, Pane, Tab};
use crate::theme::Theme;

/// Zellij's default UI chrome: tab-bar above content, status-bar below.
/// Without this block in a custom layout, Zellij renders no UI chrome.
const DEFAULT_TAB_TEMPLATE: &str = "\
    default_tab_template {\n\
        pane size=1 borderless=true {\n\
            plugin location=\"tab-bar\"\n\
        }\n\
        children\n\
        pane size=2 borderless=true {\n\
            plugin location=\"status-bar\"\n\
        }\n\
    }\n";

/// Generate the keybind block for a given binary path.
/// Binds `Alt+k` to session switcher, `Alt+l` to log viewer,
/// and `Alt+z` to action menu
/// — all running in floating panes that close on exit.
fn keybinds_block(bin_path: &str) -> String {
    let bin = escape_kdl_string(bin_path);
    format!(
        "\
    keybinds {{\n\
        shared {{\n\
            bind \"Alt k\" {{\n\
                Run \"{bin}\" \"switch\" {{\n\
                    floating true\n\
                    width \"80%\"\n\
                    height \"80%\"\n\
                    x \"10%\"\n\
                    y \"10%\"\n\
                    close_on_exit true\n\
                }}\n\
            }}\n\
            bind \"Alt l\" {{\n\
                Run \"{bin}\" \"logs-viewer\" {{\n\
                    floating true\n\
                    close_on_exit true\n\
                }}\n\
            }}\n\
            bind \"Alt z\" {{\n\
                Run \"{bin}\" \"actions\" {{\n\
                    floating true\n\
                    close_on_exit true\n\
                }}\n\
            }}\n\
        }}\n\
    }}\n",
    )
}

/// Generate a Zellij KDL layout string from a `Layout`.
///
/// Always includes a `default_tab_template` with `tab-bar` and `status-bar`
/// plugins so Zellij renders its UI chrome (tab bar and status bar) even
/// when a custom layout is provided.
///
/// Output format:
/// ```kdl
/// layout {
///     default_tab_template { ... }
///     tab name="claude" { ... }
/// }
/// keybinds {
///     shared {
///         bind "Ctrl k" { ... }
///     }
/// }
/// ```
pub fn generate_layout_kdl(layout: &Layout, bin_path: &str, theme: &Theme) -> String {
    let mut out = if let Some(ref cwd) = layout.cwd {
        format!(
            "layout cwd=\"{}\" {{\n",
            escape_kdl_string(&cwd.to_string_lossy())
        )
    } else {
        String::from("layout {\n")
    };

    out.push_str(DEFAULT_TAB_TEMPLATE);
    for tab in &layout.tabs {
        out.push_str(&generate_tab_kdl(tab));
    }
    out.push_str("}\n");

    // Emit env block at layout-file root so Zellij's config parser can
    // set environment variables on the session. Placing it inside the
    // `layout { }` block causes "Unknown layout node: 'env'" from Zellij.
    if let Some(ref session_name) = layout.session_name_env {
        out.push_str(&format!(
            "env {{\n    Z_SESSION_NAME \"{}\"\n}}\n",
            escape_kdl_string(session_name)
        ));
    }

    out.push_str(&keybinds_block(bin_path));
    out.push_str(&theme.to_zellij_kdl());
    out
}

/// Escape a string for use inside a KDL double-quoted value.
fn escape_kdl_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn generate_tab_kdl(tab: &Tab) -> String {
    let mut out = format!("    tab name=\"{}\" {{\n", escape_kdl_string(&tab.name));
    for pane in &tab.panes {
        out.push_str(&generate_pane_kdl(pane));
    }
    out.push_str("    }\n");
    out
}

fn generate_pane_kdl(pane: &Pane) -> String {
    if let Some(ref cmd) = pane.command {
        if pane.args.is_empty() {
            format!("        pane command=\"{}\"\n", escape_kdl_string(cmd))
        } else {
            let args_str = pane
                .args
                .iter()
                .map(|a| format!("\"{}\"", escape_kdl_string(a)))
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "        pane command=\"{}\" {{\n            args {}\n        }}\n",
                escape_kdl_string(cmd),
                args_str
            )
        }
    } else {
        "        pane\n".to_string()
    }
}

/// Find the first pane with `command = "claude"` and append the prompt as a
/// positional argument (Claude CLI accepts the initial prompt as a bare arg).
/// No-op if no Claude pane exists.
pub fn inject_prompt_into_layout(layout: &mut Layout, prompt: &str) {
    for tab in &mut layout.tabs {
        for pane in &mut tab.panes {
            if pane.command.as_deref() == Some("claude") {
                pane.args.push(prompt.to_string());
                return;
            }
        }
    }
}

/// Build the default layout: tab "claude" (pane command=claude) + tab "shell" (bare pane).
pub fn default_layout() -> Layout {
    Layout {
        tabs: vec![
            Tab {
                name: "claude".to_string(),
                panes: vec![Pane {
                    command: Some("claude".to_string()),
                    args: vec!["--dangerously-skip-permissions".to_string()],
                }],
            },
            Tab {
                name: "shell".to_string(),
                panes: vec![Pane {
                    command: None,
                    args: vec![],
                }],
            },
        ],
        cwd: None,
        session_name_env: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_kdl_empty_layout() {
        let layout = Layout {
            tabs: vec![],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout {\n"));
        assert!(kdl.contains("theme \"dracula\""));
        assert!(kdl.contains("default_tab_template"));
    }

    #[test]
    fn generate_kdl_with_cwd() {
        let layout = Layout {
            tabs: vec![],
            cwd: Some(std::path::PathBuf::from(
                "/home/user/projects/myapp-feat-login",
            )),
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout cwd=\"/home/user/projects/myapp-feat-login\" {\n"));
        assert!(kdl.contains("theme \"dracula\""));
        assert!(kdl.contains("default_tab_template"));
    }

    #[test]
    fn generate_kdl_cwd_with_special_chars() {
        let layout = Layout {
            tabs: vec![],
            cwd: Some(std::path::PathBuf::from(r#"/home/user/my "project""#)),
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout cwd=\"/home/user/my \\\"project\\\"\""));
    }

    #[test]
    fn generate_kdl_plain_pane() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "shell".to_string(),
                panes: vec![Pane {
                    command: None,
                    args: vec![],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout {\n"));
        assert!(kdl.contains("tab name=\"shell\""));
        assert!(kdl.contains("        pane\n"));
        assert!(kdl.contains("theme \"dracula\""));
    }

    #[test]
    fn generate_kdl_pane_with_command() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "claude".to_string(),
                panes: vec![Pane {
                    command: Some("claude".to_string()),
                    args: vec![],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("pane command=\"claude\""));
        assert!(kdl.contains("tab name=\"claude\""));
    }

    #[test]
    fn generate_kdl_pane_with_args() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "editor".to_string(),
                panes: vec![Pane {
                    command: Some("nvim".to_string()),
                    args: vec!["--headless".to_string()],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("pane command=\"nvim\" {\n            args \"--headless\"\n        }"));
    }

    #[test]
    fn generate_kdl_default_layout_structure() {
        let layout = default_layout();
        assert_eq!(layout.tabs.len(), 2);
        assert_eq!(layout.tabs[0].name, "claude");
        assert_eq!(layout.tabs[1].name, "shell");
        assert_eq!(layout.tabs[0].panes[0].command, Some("claude".to_string()));
        assert!(layout.tabs[1].panes[0].command.is_none());
    }

    #[test]
    fn default_layout_claude_pane_has_dangerously_skip_permissions() {
        let layout = default_layout();
        let claude_pane = &layout.tabs[0].panes[0];
        assert_eq!(claude_pane.args, vec!["--dangerously-skip-permissions"]);
    }

    #[test]
    fn generate_kdl_default_layout_kdl_output() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout {\n"));
        assert!(kdl.contains("tab name=\"claude\""));
        assert!(kdl.contains("pane command=\"claude\""));
        assert!(kdl.contains("tab name=\"shell\""));
        // shell tab has a plain pane (no command)
        let shell_section = kdl.find("tab name=\"shell\"").expect("shell tab in kdl");
        assert!(kdl[shell_section..].contains("pane\n"));
    }

    #[test]
    fn generate_kdl_multiple_tabs() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        let claude_pos = kdl.find("tab name=\"claude\"").unwrap();
        let shell_pos = kdl.find("tab name=\"shell\"").unwrap();
        // claude tab appears before shell tab
        assert!(claude_pos < shell_pos);
    }

    #[test]
    fn generate_kdl_tab_no_panes() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "empty".to_string(),
                panes: vec![],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout {\n"));
        assert!(kdl.contains("tab name=\"empty\""));
        assert!(kdl.contains("theme \"dracula\""));
    }

    #[test]
    fn generate_kdl_tab_name_with_quotes_is_escaped() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "my \"tab\"".to_string(),
                panes: vec![Pane {
                    command: None,
                    args: vec![],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains(r#"tab name="my \"tab\"""#));
    }

    #[test]
    fn generate_kdl_command_with_backslash_is_escaped() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "test".to_string(),
                panes: vec![Pane {
                    command: Some(r"C:\bin\tool".to_string()),
                    args: vec![],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains(r#"pane command="C:\\bin\\tool""#));
    }

    #[test]
    fn generate_kdl_args_with_quotes_are_escaped() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "editor".to_string(),
                panes: vec![Pane {
                    command: Some("echo".to_string()),
                    args: vec!["hello \"world\"".to_string()],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("args \"hello \\\"world\\\"\""));
    }

    #[test]
    fn generate_kdl_multiple_panes_in_tab() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "split".to_string(),
                panes: vec![
                    Pane {
                        command: Some("htop".to_string()),
                        args: vec![],
                    },
                    Pane {
                        command: None,
                        args: vec![],
                    },
                ],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("pane command=\"htop\""));
        assert!(kdl.contains("        pane\n"));
    }

    #[test]
    fn generate_kdl_pane_with_multiple_args() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "editor".to_string(),
                panes: vec![Pane {
                    command: Some("nvim".to_string()),
                    args: vec!["-u".to_string(), "NONE".to_string(), "file.txt".to_string()],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains(
            "pane command=\"nvim\" {\n            args \"-u\" \"NONE\" \"file.txt\"\n        }"
        ));
    }

    #[test]
    fn generate_kdl_cwd_with_tabs() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "shell".to_string(),
                panes: vec![Pane {
                    command: None,
                    args: vec![],
                }],
            }],
            cwd: Some(std::path::PathBuf::from("/home/user/myapp-feat")),
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout cwd=\"/home/user/myapp-feat\" {\n"));
        assert!(kdl.contains("tab name=\"shell\""));
        assert!(kdl.contains("theme \"dracula\""));
    }

    #[test]
    fn generate_kdl_default_layout_with_cwd() {
        let mut layout = default_layout();
        layout.cwd = Some(std::path::PathBuf::from("/worktree/path"));
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout cwd=\"/worktree/path\""));
        assert!(kdl.contains("tab name=\"claude\""));
        assert!(kdl.contains("pane command=\"claude\""));
        assert!(kdl.contains("tab name=\"shell\""));
    }

    #[test]
    fn generate_kdl_includes_default_tab_template_with_tab_bar_and_status_bar() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(
            kdl.contains("default_tab_template"),
            "layout must include default_tab_template block"
        );
        assert!(
            kdl.contains("plugin location=\"tab-bar\""),
            "layout must include tab-bar plugin"
        );
        assert!(
            kdl.contains("plugin location=\"status-bar\""),
            "layout must include status-bar plugin"
        );
        assert!(
            kdl.contains("children"),
            "default_tab_template must include children placeholder"
        );
    }

    #[test]
    fn generate_kdl_tab_template_appears_before_tabs() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        let template_pos = kdl.find("default_tab_template").unwrap();
        let first_tab_pos = kdl.find("tab name=").unwrap();
        assert!(
            template_pos < first_tab_pos,
            "default_tab_template must appear before tab definitions"
        );
    }

    #[test]
    fn escape_kdl_string_no_special_chars() {
        assert_eq!(escape_kdl_string("hello"), "hello");
    }

    #[test]
    fn escape_kdl_string_with_both_quote_and_backslash() {
        assert_eq!(escape_kdl_string(r#"a\"b"#), r#"a\\\"b"#);
    }

    #[test]
    fn generate_kdl_includes_keybinds_block() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(
            kdl.contains("keybinds {"),
            "layout must include keybinds block"
        );
        assert!(
            kdl.contains("shared {"),
            "keybinds must include shared block"
        );
        assert!(kdl.contains("bind \"Alt k\""), "keybinds must bind Alt k");
        assert!(kdl.contains("bind \"Alt l\""), "keybinds must bind Alt l");
        // Alt g removed — lazygit is now in the action menu
        assert!(kdl.contains("bind \"Alt z\""), "keybinds must bind Alt z");
        assert!(kdl.contains("\"switch\""), "binding must run z switch");
        assert!(kdl.contains("\"actions\""), "binding must run z actions");
        // lazygit removed from keybinds — now in action menu
        assert!(
            kdl.contains("floating true"),
            "binding must set floating true"
        );
        assert!(
            kdl.contains("close_on_exit true"),
            "binding must set close_on_exit true"
        );
    }

    #[test]
    fn alt_k_switcher_uses_wide_floating_pane() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        let alt_k_start = kdl.find("bind \"Alt k\"").expect("Alt k binding missing");
        let alt_k_block = &kdl[alt_k_start..];
        let alt_k_end = alt_k_block
            .find("bind \"Alt l\"")
            .unwrap_or(alt_k_block.len());
        let alt_k_block = &alt_k_block[..alt_k_end];
        assert!(
            alt_k_block.contains("width"),
            "Alt k binding must specify a width to widen the switcher; got:\n{}",
            alt_k_block
        );
        assert!(
            alt_k_block.contains("height"),
            "Alt k binding must specify a height; got:\n{}",
            alt_k_block
        );
    }

    #[test]
    fn generate_kdl_keybinds_use_provided_bin_path() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/opt/z/bin/z", &Theme::default());
        assert!(
            kdl.contains("Run \"/opt/z/bin/z\" \"switch\""),
            "switch keybind must use provided bin_path"
        );
        assert!(
            kdl.contains("Run \"/opt/z/bin/z\" \"logs-viewer\""),
            "logs-viewer keybind must use provided bin_path"
        );
        assert!(
            !kdl.contains("/Users/arkan"),
            "must not contain hardcoded dev path"
        );
    }

    #[test]
    fn generate_kdl_keybinds_present_in_empty_layout() {
        let layout = Layout {
            tabs: vec![],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("keybinds {"));
        assert!(kdl.contains("bind \"Alt k\""));
    }

    #[test]
    fn generate_kdl_keybinds_present_with_custom_tabs() {
        let layout = Layout {
            tabs: vec![Tab {
                name: "work".to_string(),
                panes: vec![Pane {
                    command: Some("vim".to_string()),
                    args: vec![],
                }],
            }],
            cwd: Some(std::path::PathBuf::from("/some/path")),
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("keybinds {"));
        assert!(kdl.contains("bind \"Alt k\""));
        assert!(kdl.contains("tab name=\"work\""));
    }

    #[test]
    fn generate_kdl_keybinds_appears_after_tab_template() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        let template_pos = kdl.find("default_tab_template").unwrap();
        let keybinds_pos = kdl.find("keybinds {").unwrap();
        assert!(
            template_pos < keybinds_pos,
            "keybinds block must appear after default_tab_template"
        );
    }

    #[test]
    fn generate_kdl_keybinds_appears_after_layout_block() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        let layout_close = kdl.find("}\n").unwrap();
        let keybinds_pos = kdl.find("keybinds {").unwrap();
        assert!(
            keybinds_pos > layout_close,
            "keybinds block must appear outside (after) the layout block"
        );
    }

    // -----------------------------------------------------------------------
    // inject_prompt_into_layout
    // -----------------------------------------------------------------------

    #[test]
    fn inject_prompt_adds_arg_to_claude_pane() {
        let mut layout = default_layout();
        inject_prompt_into_layout(&mut layout, "hello world");
        let claude_pane = &layout.tabs[0].panes[0];
        assert!(claude_pane.args.contains(&"hello world".to_string()));
    }

    #[test]
    fn inject_prompt_preserves_existing_args() {
        let mut layout = default_layout();
        inject_prompt_into_layout(&mut layout, "test");
        let claude_pane = &layout.tabs[0].panes[0];
        assert!(claude_pane
            .args
            .contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn inject_prompt_no_claude_pane_is_noop() {
        let mut layout = Layout {
            tabs: vec![Tab {
                name: "shell".to_string(),
                panes: vec![Pane {
                    command: None,
                    args: vec![],
                }],
            }],
            cwd: None,
            session_name_env: None,
        };
        inject_prompt_into_layout(&mut layout, "test");
        // No panic, no change
        assert!(layout.tabs[0].panes[0].args.is_empty());
    }

    #[test]
    fn inject_prompt_claude_in_second_tab() {
        let mut layout = Layout {
            tabs: vec![
                Tab {
                    name: "shell".to_string(),
                    panes: vec![Pane {
                        command: None,
                        args: vec![],
                    }],
                },
                Tab {
                    name: "editor".to_string(),
                    panes: vec![Pane {
                        command: Some("claude".to_string()),
                        args: vec![],
                    }],
                },
            ],
            cwd: None,
            session_name_env: None,
        };
        inject_prompt_into_layout(&mut layout, "found it");
        assert!(layout.tabs[1].panes[0]
            .args
            .contains(&"found it".to_string()));
    }

    #[test]
    fn inject_prompt_generates_correct_kdl() {
        let mut layout = default_layout();
        inject_prompt_into_layout(&mut layout, "/grill-me issue #42");
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("\"/grill-me issue #42\""));
    }

    // -----------------------------------------------------------------------
    // session_name_env (env block)
    // -----------------------------------------------------------------------

    #[test]
    fn generate_kdl_no_env_block_when_not_set() {
        let layout = Layout {
            tabs: vec![],
            cwd: None,
            session_name_env: None,
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(
            !kdl.contains("env {"),
            "should not contain env block when session_name_env is None"
        );
    }

    #[test]
    fn generate_kdl_env_block_with_session_name() {
        let layout = Layout {
            tabs: vec![],
            cwd: None,
            session_name_env: Some("myapp:feat-login".to_string()),
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.contains("env {"), "should contain env block");
        assert!(
            kdl.contains(r#"Z_SESSION_NAME "myapp:feat-login""#),
            "should set Z_SESSION_NAME"
        );
    }

    #[test]
    fn generate_kdl_env_block_appears_outside_layout_block() {
        let layout = Layout {
            tabs: vec![],
            cwd: None,
            session_name_env: Some("myapp:main".to_string()),
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        let layout_close = kdl.find("layout {").expect("layout block opening");
        // Find the "}\n" that closes the layout block (first `}\n` after layout {)
        let layout_block_end = kdl[layout_close..].find("}\n").expect("layout block end");
        let layout_block_end_abs = layout_close + layout_block_end;
        let env_pos = kdl.find("env {").expect("env block");
        let keybinds_pos = kdl.find("keybinds {").expect("keybinds block");
        assert!(
            env_pos > layout_block_end_abs,
            "env block must appear after the layout block closes"
        );
        assert!(
            env_pos < keybinds_pos,
            "env block must appear before keybinds block"
        );
    }

    #[test]
    fn generate_kdl_env_block_with_cwd_and_session() {
        let layout = Layout {
            tabs: vec![],
            cwd: Some(std::path::PathBuf::from("/work/path")),
            session_name_env: Some("proj:branch".to_string()),
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(kdl.starts_with("layout cwd=\"/work/path\" {"));
        assert!(kdl.contains(r#"Z_SESSION_NAME "proj:branch""#));
    }

    #[test]
    fn generate_kdl_env_block_escapes_session_name() {
        let layout = Layout {
            tabs: vec![],
            cwd: None,
            session_name_env: Some(r#"my"app:feat"#.to_string()),
        };
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &Theme::default());
        assert!(
            kdl.contains(r#"Z_SESSION_NAME "my\"app:feat""#),
            "should escape quotes in session name"
        );
    }
}
