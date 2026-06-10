use crab_core::permission::PermissionMode;

/// Resolve the effective permission mode from CLI flags and settings.
///
/// Priority:
/// 1. `--permission-mode` (explicit string)
/// 2. `--dangerously-skip-permissions`
/// 3. `--auto`
/// 4. `--trust-project`
/// 5. Settings file `permission_mode`
/// 6. Default
pub fn resolve_permission_mode(
    cli_mode: Option<&str>,
    dangerously: bool,
    auto: bool,
    trust_project: bool,
    settings_mode: Option<&str>,
) -> Result<PermissionMode, String> {
    if let Some(mode_str) = cli_mode {
        mode_str.parse::<PermissionMode>()
    } else if dangerously {
        Ok(PermissionMode::Dangerously)
    } else if auto {
        Ok(PermissionMode::Auto)
    } else if trust_project {
        Ok(PermissionMode::TrustProject)
    } else {
        Ok(settings_mode
            .and_then(|s| s.parse::<PermissionMode>().ok())
            .unwrap_or(PermissionMode::Default))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_cli_mode_takes_priority() {
        assert_eq!(
            resolve_permission_mode(Some("plan"), false, false, false, None).unwrap(),
            PermissionMode::Plan
        );
    }

    #[test]
    fn dangerously_flag() {
        assert_eq!(
            resolve_permission_mode(None, true, false, false, None).unwrap(),
            PermissionMode::Dangerously
        );
    }

    #[test]
    fn auto_flag() {
        assert_eq!(
            resolve_permission_mode(None, false, true, false, None).unwrap(),
            PermissionMode::Auto
        );
    }

    #[test]
    fn trust_project_flag() {
        assert_eq!(
            resolve_permission_mode(None, false, false, true, None).unwrap(),
            PermissionMode::TrustProject
        );
    }

    #[test]
    fn settings_fallback() {
        assert_eq!(
            resolve_permission_mode(None, false, false, false, Some("acceptEdits")).unwrap(),
            PermissionMode::AcceptEdits
        );
    }

    #[test]
    fn default_when_nothing_set() {
        assert_eq!(
            resolve_permission_mode(None, false, false, false, None).unwrap(),
            PermissionMode::Default
        );
    }

    #[test]
    fn cli_mode_overrides_flags() {
        // Even if dangerously is set, explicit cli_mode wins.
        assert_eq!(
            resolve_permission_mode(Some("plan"), true, false, false, None).unwrap(),
            PermissionMode::Plan
        );
    }

    #[test]
    fn invalid_cli_mode_errors() {
        assert!(resolve_permission_mode(Some("bogus"), false, false, false, None).is_err());
    }

    #[test]
    fn invalid_settings_ignored() {
        // Invalid settings mode falls back to Default.
        assert_eq!(
            resolve_permission_mode(None, false, false, false, Some("bogus")).unwrap(),
            PermissionMode::Default
        );
    }
}
