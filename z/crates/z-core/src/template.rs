use std::collections::HashMap;

/// Default prompt template for sessions created from a GitHub issue.
pub const DEFAULT_ISSUE_TEMPLATE: &str = "/grill-me We are going to work on issue #{number}: {title}. Fetch full context and all comments with gh issue view {number} --comments";

/// Default prompt template for sessions created from a GitHub PR.
pub const DEFAULT_PR_TEMPLATE: &str = "/grill-me We are going to review PR #{number}: {title}. Fetch full context, diff, and all comments with gh pr view {number} --comments";

/// Replace `{key}` placeholders in `template` with values from `vars`.
/// Unknown placeholders are left as-is.
pub fn resolve_template(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{}}}", key), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_placeholder() {
        let mut vars = HashMap::new();
        vars.insert("number", "42");
        assert_eq!(resolve_template("issue #{number}", &vars), "issue #42");
    }

    #[test]
    fn multiple_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("number", "42");
        vars.insert("title", "fix login");
        assert_eq!(
            resolve_template("#{number}: {title}", &vars),
            "#42: fix login"
        );
    }

    #[test]
    fn repeated_placeholder() {
        let mut vars = HashMap::new();
        vars.insert("number", "7");
        assert_eq!(resolve_template("{number} and {number}", &vars), "7 and 7");
    }

    #[test]
    fn missing_key_left_as_is() {
        let vars = HashMap::new();
        assert_eq!(resolve_template("{unknown}", &vars), "{unknown}");
    }

    #[test]
    fn empty_template() {
        let vars = HashMap::new();
        assert_eq!(resolve_template("", &vars), "");
    }

    #[test]
    fn special_chars_in_values() {
        let mut vars = HashMap::new();
        vars.insert("title", "fix: \"quoted\" & <tag>");
        assert_eq!(
            resolve_template("{title}", &vars),
            "fix: \"quoted\" & <tag>"
        );
    }

    #[test]
    fn default_issue_template_resolves() {
        let mut vars = HashMap::new();
        vars.insert("number", "42");
        vars.insert("title", "add auth");
        let result = resolve_template(DEFAULT_ISSUE_TEMPLATE, &vars);
        assert!(result.contains("issue #42"));
        assert!(result.contains("add auth"));
        assert!(result.contains("gh issue view 42 --comments"));
    }

    #[test]
    fn default_pr_template_resolves() {
        let mut vars = HashMap::new();
        vars.insert("number", "99");
        vars.insert("title", "refactor API");
        let result = resolve_template(DEFAULT_PR_TEMPLATE, &vars);
        assert!(result.contains("PR #99"));
        assert!(result.contains("refactor API"));
        assert!(result.contains("gh pr view 99 --comments"));
    }
}
