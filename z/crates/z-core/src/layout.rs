use crate::domain::{Layout, Pane, Tab};

/// Generate a Zellij KDL layout string from a `Layout`.
///
/// Output format:
/// ```kdl
/// layout {
///     tab name="claude" {
///         pane command="claude"
///     }
///     tab name="shell" {
///         pane
///     }
/// }
/// ```
pub fn generate_layout_kdl(layout: &Layout) -> String {
    let mut out = String::from("layout {\n");
    for tab in &layout.tabs {
        out.push_str(&generate_tab_kdl(tab));
    }
    out.push_str("}\n");
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
                "        pane command=\"{}\" args={}\n",
                escape_kdl_string(cmd),
                args_str
            )
        }
    } else {
        "        pane\n".to_string()
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
                    args: vec![],
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_kdl_empty_layout() {
        let layout = Layout { tabs: vec![] };
        let kdl = generate_layout_kdl(&layout);
        assert_eq!(kdl, "layout {\n}\n");
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
        };
        let kdl = generate_layout_kdl(&layout);
        assert_eq!(
            kdl,
            "layout {\n    tab name=\"shell\" {\n        pane\n    }\n}\n"
        );
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
        };
        let kdl = generate_layout_kdl(&layout);
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
        };
        let kdl = generate_layout_kdl(&layout);
        assert!(kdl.contains("pane command=\"nvim\" args=\"--headless\""));
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
    fn generate_kdl_default_layout_kdl_output() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout);
        assert!(kdl.starts_with("layout {\n"));
        assert!(kdl.contains("tab name=\"claude\""));
        assert!(kdl.contains("pane command=\"claude\""));
        assert!(kdl.contains("tab name=\"shell\""));
        // shell tab has a plain pane (no command)
        let shell_section = kdl
            .find("tab name=\"shell\"")
            .expect("shell tab in kdl");
        assert!(kdl[shell_section..].contains("pane\n"));
    }

    #[test]
    fn generate_kdl_multiple_tabs() {
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout);
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
        };
        let kdl = generate_layout_kdl(&layout);
        assert_eq!(kdl, "layout {\n    tab name=\"empty\" {\n    }\n}\n");
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
        };
        let kdl = generate_layout_kdl(&layout);
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
        };
        let kdl = generate_layout_kdl(&layout);
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
        };
        let kdl = generate_layout_kdl(&layout);
        assert!(kdl.contains(r#"args="hello \"world\"""#));
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
        };
        let kdl = generate_layout_kdl(&layout);
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
        };
        let kdl = generate_layout_kdl(&layout);
        assert!(kdl.contains(r#"pane command="nvim" args="-u" "NONE" "file.txt""#));
    }

    #[test]
    fn escape_kdl_string_no_special_chars() {
        assert_eq!(escape_kdl_string("hello"), "hello");
    }

    #[test]
    fn escape_kdl_string_with_both_quote_and_backslash() {
        assert_eq!(escape_kdl_string(r#"a\"b"#), r#"a\\\"b"#);
    }
}
