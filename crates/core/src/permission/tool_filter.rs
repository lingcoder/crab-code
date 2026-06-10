/// Resolve `--allowed-tools`, `--disallowed-tools`, and `--tools` into effective
/// allowed/denied lists.
///
/// `--tools ""` disables all tools (denied = `["*"]`).
/// `--tools "default"` allows all (no filtering).
/// `--tools "read,write"` restricts to named tools only (allowed = those names).
pub fn resolve_tool_filters(
    allowed: &[String],
    disallowed: &[String],
    tools: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut effective_allowed = allowed.to_vec();
    let mut effective_denied = disallowed.to_vec();

    if let Some(tools_arg) = tools {
        let trimmed = tools_arg.trim();
        if trimmed.is_empty() {
            // Empty string: disable all tools
            effective_denied = vec!["*".to_string()];
        } else if trimmed == "default" {
            // "default": no filtering
        } else {
            // Explicit list: only these tools are allowed
            let names: Vec<String> = trimmed
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            effective_allowed = names;
        }
    }

    (effective_allowed, effective_denied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_tool_filters_empty_inputs() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], None);
        assert!(allowed.is_empty());
        assert!(denied.is_empty());
    }

    #[test]
    fn resolve_tool_filters_passthrough_lists() {
        let (allowed, denied) =
            resolve_tool_filters(&["Read".into(), "Write".into()], &["Bash".into()], None);
        assert_eq!(allowed, vec!["Read", "Write"]);
        assert_eq!(denied, vec!["Bash"]);
    }

    #[test]
    fn resolve_tool_filters_tools_empty_disables_all() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], Some(""));
        assert!(allowed.is_empty());
        assert_eq!(denied, vec!["*"]);
    }

    #[test]
    fn resolve_tool_filters_tools_default_no_change() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], Some("default"));
        assert!(allowed.is_empty());
        assert!(denied.is_empty());
    }

    #[test]
    fn resolve_tool_filters_tools_explicit_list() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], Some("Read,Write,Edit"));
        assert_eq!(allowed, vec!["Read", "Write", "Edit"]);
        assert!(denied.is_empty());
    }
}
