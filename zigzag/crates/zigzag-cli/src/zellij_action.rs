use std::process::{Command, ExitStatus};

use zigzag_core::action::PaneType;

/// Request to launch an Action command inside Zellij.
#[derive(Debug, Clone, PartialEq)]
pub struct ZellijActionRequest {
    pub session: Option<String>,
    pub tab_name: Option<String>,
    pub pane_type: PaneType,
    pub command: String,
}

/// Build the `zellij` arguments for an Action command.
pub fn action_args(request: &ZellijActionRequest) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(session) = &request.session {
        args.extend(["-s".to_string(), session.clone()]);
    }

    match request.pane_type {
        PaneType::Tab => {
            args.extend(["action".to_string(), "new-tab".to_string()]);
            if let Some(tab_name) = &request.tab_name {
                args.extend(["-n".to_string(), tab_name.clone()]);
            }
            args.extend([
                "--".to_string(),
                "sh".to_string(),
                "-c".to_string(),
                request.command.clone(),
            ]);
        }
        PaneType::Float | PaneType::FloatFullscreen | PaneType::Split => {
            args.push("run".to_string());
            match request.pane_type {
                PaneType::Float => args.extend(["--floating".to_string(), "-c".to_string()]),
                PaneType::FloatFullscreen => args.extend([
                    "--floating".to_string(),
                    "-c".to_string(),
                    "--width".to_string(),
                    "100%".to_string(),
                    "--height".to_string(),
                    "100%".to_string(),
                ]),
                PaneType::Split => args.push("-c".to_string()),
                PaneType::Tab => unreachable!(),
            }
            args.extend([
                "--".to_string(),
                "sh".to_string(),
                "-c".to_string(),
                request.command.clone(),
            ]);
        }
    }
    args
}

/// Run an Action command through the concrete Zellij Adapter.
pub fn run_action(request: &ZellijActionRequest) -> std::io::Result<ExitStatus> {
    Command::new("zellij").args(action_args(request)).status()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(pane_type: PaneType) -> ZellijActionRequest {
        ZellijActionRequest {
            session: Some("myapp:main".to_string()),
            tab_name: Some("test".to_string()),
            pane_type,
            command: "cargo test".to_string(),
        }
    }

    #[test]
    fn builds_tui_tab_action_args_with_session() {
        let mut req = request(PaneType::Tab);
        req.tab_name = None;

        assert_eq!(
            action_args(&req),
            vec![
                "-s",
                "myapp:main",
                "action",
                "new-tab",
                "--",
                "sh",
                "-c",
                "cargo test"
            ]
        );
    }

    #[test]
    fn builds_cli_tab_action_args_with_name() {
        let mut req = request(PaneType::Tab);
        req.session = None;

        assert_eq!(
            action_args(&req),
            vec![
                "action",
                "new-tab",
                "-n",
                "test",
                "--",
                "sh",
                "-c",
                "cargo test"
            ]
        );
    }

    #[test]
    fn builds_float_action_args() {
        assert_eq!(
            action_args(&request(PaneType::Float)),
            vec![
                "-s",
                "myapp:main",
                "run",
                "--floating",
                "-c",
                "--",
                "sh",
                "-c",
                "cargo test"
            ]
        );
    }

    #[test]
    fn builds_fullscreen_float_action_args() {
        assert_eq!(
            action_args(&request(PaneType::FloatFullscreen)),
            vec![
                "-s",
                "myapp:main",
                "run",
                "--floating",
                "-c",
                "--width",
                "100%",
                "--height",
                "100%",
                "--",
                "sh",
                "-c",
                "cargo test"
            ]
        );
    }

    #[test]
    fn builds_split_action_args() {
        assert_eq!(
            action_args(&request(PaneType::Split)),
            vec![
                "-s",
                "myapp:main",
                "run",
                "-c",
                "--",
                "sh",
                "-c",
                "cargo test"
            ]
        );
    }
}
