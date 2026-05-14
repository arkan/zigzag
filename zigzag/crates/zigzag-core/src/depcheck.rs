use semver::{Version, VersionReq};

use crate::error::Result;

/// Minimum version requirements for external dependencies.
#[derive(Debug, Clone)]
pub struct DepSpec {
    pub tool: &'static str,
    /// Semver requirement string, e.g. `">=0.44.0"`.
    pub min_version: &'static str,
}

pub const REQUIRED_DEPS: &[DepSpec] = &[
    DepSpec {
        tool: "zellij",
        min_version: ">=0.44.0",
    },
    DepSpec {
        tool: "wt",
        min_version: ">=0.34.0",
    },
    DepSpec {
        tool: "gh",
        min_version: ">=2.0.0",
    },
];

/// Abstraction over external tool version probing.
/// Implement this trait to inject real or mock behavior.
pub trait DepChecker {
    /// Returns the raw `--version` output for `tool`, or `None` if the tool
    /// is not found. Must not call `std::process::Command` directly —
    /// implementations live outside `zigzag-core`.
    fn get_version_output(&self, tool: &str) -> Result<Option<String>>;
}

/// Parse a `Version` from a raw `--version` output string.
///
/// Handles common formats:
/// - `"zellij 0.44.0"`
/// - `"gh version 2.0.0 (2021-08-23)"`
/// - `"wt 0.34.0"`
pub fn parse_version(output: &str) -> Option<Version> {
    for word in output.split_whitespace() {
        // Strip surrounding punctuation so "(2.0.0)" or "v2.0.0" are handled.
        let word = word.trim_matches(|c: char| c == '(' || c == ')' || c == 'v' || c == 'V');
        if let Ok(v) = Version::parse(word) {
            return Some(v);
        }
    }
    None
}

/// Result of checking a single dependency.
#[derive(Debug)]
pub struct DepCheckResult {
    pub tool: &'static str,
    pub status: DepCheckStatus,
}

#[derive(Debug)]
pub enum DepCheckStatus {
    Ok { version: Version },
    Missing,
    VersionTooLow { found: Version, required: String },
    VersionUnparseable { output: String },
}

/// Run all dependency checks and return one result per required tool.
pub fn check_deps<C: DepChecker>(checker: &C) -> Vec<DepCheckResult> {
    REQUIRED_DEPS
        .iter()
        .map(|spec| check_one(checker, spec))
        .collect()
}

fn check_one<C: DepChecker>(checker: &C, spec: &DepSpec) -> DepCheckResult {
    let status = match checker.get_version_output(spec.tool) {
        Ok(Some(output)) => match parse_version(&output) {
            Some(version) => {
                let req = VersionReq::parse(spec.min_version)
                    .expect("REQUIRED_DEPS contains valid semver requirements");
                if req.matches(&version) {
                    DepCheckStatus::Ok { version }
                } else {
                    DepCheckStatus::VersionTooLow {
                        found: version,
                        required: spec.min_version.to_string(),
                    }
                }
            }
            None => DepCheckStatus::VersionUnparseable {
                output: output.trim().to_string(),
            },
        },
        Ok(None) => DepCheckStatus::Missing,
        Err(_) => DepCheckStatus::Missing,
    };
    DepCheckResult {
        tool: spec.tool,
        status,
    }
}

/// Format a human-readable error message for a failed dep check.
/// Returns an empty string if the check passed.
pub fn format_dep_error(result: &DepCheckResult) -> String {
    match &result.status {
        DepCheckStatus::Ok { .. } => String::new(),
        DepCheckStatus::Missing => format!(
            "error: '{}' is not installed or not in PATH.\n  Please install it before running Zigzag.",
            result.tool
        ),
        DepCheckStatus::VersionTooLow { found, required } => format!(
            "error: '{}' version {} does not satisfy {}.\n  Please upgrade before running Zigzag.",
            result.tool, found, required
        ),
        DepCheckStatus::VersionUnparseable { output } => format!(
            "error: could not parse version from '{}' output: {:?}\n  Please check that '{}' is installed correctly.",
            result.tool, output, result.tool
        ),
    }
}

/// Returns `true` if all checks passed.
pub fn all_ok(results: &[DepCheckResult]) -> bool {
    results
        .iter()
        .all(|r| matches!(r.status, DepCheckStatus::Ok { .. }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ---- version parsing --------------------------------------------------

    #[test]
    fn test_parse_version_zellij_format() {
        let v = parse_version("zellij 0.44.0").unwrap();
        assert_eq!(v, Version::new(0, 44, 0));
    }

    #[test]
    fn test_parse_version_gh_format() {
        let v = parse_version("gh version 2.45.0 (2024-01-15)").unwrap();
        assert_eq!(v, Version::new(2, 45, 0));
    }

    #[test]
    fn test_parse_version_wt_format() {
        let v = parse_version("wt 0.34.0").unwrap();
        assert_eq!(v, Version::new(0, 34, 0));
    }

    #[test]
    fn test_parse_version_with_v_prefix() {
        let v = parse_version("tool v1.2.3").unwrap();
        assert_eq!(v, Version::new(1, 2, 3));
    }

    #[test]
    fn test_parse_version_not_found_returns_none() {
        assert!(parse_version("no version here").is_none());
        assert!(parse_version("").is_none());
    }

    #[test]
    fn test_parse_version_multiline_output() {
        let v = parse_version(
            "gh version 2.45.0 (2024-01-15)\nhttps://github.com/cli/cli/releases/tag/v2.45.0",
        )
        .unwrap();
        assert_eq!(v, Version::new(2, 45, 0));
    }

    #[test]
    fn test_parse_version_with_prerelease() {
        let v = parse_version("tool 1.0.0-beta.1").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
        assert!(!v.pre.is_empty());
    }

    #[test]
    fn test_parse_version_whitespace_only() {
        assert!(parse_version("   \n\t  ").is_none());
    }

    #[test]
    fn test_parse_version_only_major_minor() {
        // "2.45" is not valid semver — should return None
        assert!(parse_version("tool 2.45").is_none());
    }

    // ---- version requirement checks --------------------------------------

    #[test]
    fn test_version_req_passes_exact() {
        let req = VersionReq::parse(">=0.44.0").unwrap();
        assert!(req.matches(&Version::new(0, 44, 0)));
    }

    #[test]
    fn test_version_req_passes_higher() {
        let req = VersionReq::parse(">=0.44.0").unwrap();
        assert!(req.matches(&Version::new(0, 45, 0)));
        assert!(req.matches(&Version::new(1, 0, 0)));
    }

    #[test]
    fn test_version_req_fails_lower() {
        let req = VersionReq::parse(">=0.44.0").unwrap();
        assert!(!req.matches(&Version::new(0, 43, 9)));
        assert!(!req.matches(&Version::new(0, 43, 0)));
    }

    #[test]
    fn test_version_req_gh_passes() {
        let req = VersionReq::parse(">=2.0.0").unwrap();
        assert!(req.matches(&Version::new(2, 0, 0)));
        assert!(req.matches(&Version::new(2, 45, 0)));
    }

    #[test]
    fn test_version_req_gh_fails() {
        let req = VersionReq::parse(">=2.0.0").unwrap();
        assert!(!req.matches(&Version::new(1, 9, 9)));
    }

    // ---- mock DepChecker -------------------------------------------------

    struct MockDepChecker {
        /// tool name → raw version output (None = not found)
        versions: HashMap<&'static str, Option<&'static str>>,
    }

    impl MockDepChecker {
        fn new(versions: HashMap<&'static str, Option<&'static str>>) -> Self {
            Self { versions }
        }
    }

    impl DepChecker for MockDepChecker {
        fn get_version_output(&self, tool: &str) -> Result<Option<String>> {
            Ok(self
                .versions
                .get(tool)
                .and_then(|v| v.map(|s| s.to_string())))
        }
    }

    fn all_deps_present() -> MockDepChecker {
        MockDepChecker::new(HashMap::from([
            ("zellij", Some("zellij 0.44.0")),
            ("wt", Some("wt 0.34.0")),
            ("gh", Some("gh version 2.45.0 (2024-01-15)")),
        ]))
    }

    // ---- dep-check logic -------------------------------------------------

    #[test]
    fn test_check_deps_all_ok() {
        let checker = all_deps_present();
        let results = check_deps(&checker);
        assert!(all_ok(&results), "all deps should be ok");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_check_deps_missing_tool() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", None),
            ("wt", Some("wt 0.34.0")),
            ("gh", Some("gh version 2.45.0 (2024-01-15)")),
        ]));
        let results = check_deps(&checker);
        assert!(!all_ok(&results));
        let zellij = results.iter().find(|r| r.tool == "zellij").unwrap();
        assert!(matches!(zellij.status, DepCheckStatus::Missing));
    }

    #[test]
    fn test_check_deps_version_too_low() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", Some("zellij 0.43.0")),
            ("wt", Some("wt 0.34.0")),
            ("gh", Some("gh version 2.45.0 (2024-01-15)")),
        ]));
        let results = check_deps(&checker);
        assert!(!all_ok(&results));
        let zellij = results.iter().find(|r| r.tool == "zellij").unwrap();
        assert!(
            matches!(&zellij.status, DepCheckStatus::VersionTooLow { found, .. } if *found == Version::new(0, 43, 0))
        );
    }

    #[test]
    fn test_check_deps_version_above_minimum_ok() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", Some("zellij 1.0.0")),
            ("wt", Some("wt 1.0.0")),
            ("gh", Some("gh version 3.0.0 (2025-01-01)")),
        ]));
        let results = check_deps(&checker);
        assert!(all_ok(&results));
    }

    #[test]
    fn test_format_dep_error_missing() {
        let result = DepCheckResult {
            tool: "zellij",
            status: DepCheckStatus::Missing,
        };
        let msg = format_dep_error(&result);
        assert!(msg.contains("zellij"));
        assert!(msg.contains("not installed"));
    }

    #[test]
    fn test_format_dep_error_version_too_low() {
        let result = DepCheckResult {
            tool: "zellij",
            status: DepCheckStatus::VersionTooLow {
                found: Version::new(0, 43, 0),
                required: ">=0.44.0".to_string(),
            },
        };
        let msg = format_dep_error(&result);
        assert!(msg.contains("zellij"));
        assert!(msg.contains("0.43.0"));
        assert!(msg.contains(">=0.44.0"));
    }

    #[test]
    fn test_format_dep_error_ok_is_empty() {
        let result = DepCheckResult {
            tool: "gh",
            status: DepCheckStatus::Ok {
                version: Version::new(2, 45, 0),
            },
        };
        assert_eq!(format_dep_error(&result), "");
    }

    #[test]
    fn test_all_ok_false_when_one_fails() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", Some("zellij 0.44.0")),
            ("wt", Some("wt 0.34.0")),
            ("gh", None), // missing
        ]));
        let results = check_deps(&checker);
        assert!(!all_ok(&results));
    }

    #[test]
    fn test_check_deps_unparseable_version_output() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", Some("zellij version unknown")),
            ("wt", Some("wt 0.34.0")),
            ("gh", Some("gh version 2.45.0 (2024-01-15)")),
        ]));
        let results = check_deps(&checker);
        assert!(!all_ok(&results));
        let zellij = results.iter().find(|r| r.tool == "zellij").unwrap();
        assert!(matches!(
            &zellij.status,
            DepCheckStatus::VersionUnparseable { .. }
        ));
    }

    #[test]
    fn test_format_dep_error_unparseable() {
        let result = DepCheckResult {
            tool: "zellij",
            status: DepCheckStatus::VersionUnparseable {
                output: "zellij version unknown".to_string(),
            },
        };
        let msg = format_dep_error(&result);
        assert!(msg.contains("could not parse version"));
        assert!(msg.contains("zellij"));
    }

    #[test]
    fn test_check_deps_empty_version_output() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", Some("")),
            ("wt", Some("wt 0.34.0")),
            ("gh", Some("gh version 2.45.0 (2024-01-15)")),
        ]));
        let results = check_deps(&checker);
        assert!(!all_ok(&results));
        let zellij = results.iter().find(|r| r.tool == "zellij").unwrap();
        assert!(matches!(
            &zellij.status,
            DepCheckStatus::VersionUnparseable { .. }
        ));
    }

    #[test]
    fn test_check_deps_checker_returns_error() {
        struct ErrorChecker;
        impl DepChecker for ErrorChecker {
            fn get_version_output(&self, _tool: &str) -> Result<Option<String>> {
                Err(crate::error::ZError::Io("connection refused".to_string()))
            }
        }
        let results = check_deps(&ErrorChecker);
        assert!(!all_ok(&results));
        assert!(results
            .iter()
            .all(|r| matches!(r.status, DepCheckStatus::Missing)));
    }

    #[test]
    fn test_check_deps_all_missing() {
        let checker = MockDepChecker::new(HashMap::from([
            ("zellij", None),
            ("wt", None),
            ("gh", None),
        ]));
        let results = check_deps(&checker);
        assert!(!all_ok(&results));
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|r| matches!(r.status, DepCheckStatus::Missing)));
    }
}
