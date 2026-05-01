//! Built-in hooks shipped with the crate.

pub mod file_access;

use crate::registry::{HookRegistry, RegisteredHook};

/// All built-in hooks.
#[must_use]
pub fn builtin_hooks() -> Vec<RegisteredHook> {
    vec![file_access::file_access_hook()]
}

/// Register all built-in hooks with the given registry.
pub async fn register_builtin_hooks(registry: &HookRegistry) -> Vec<String> {
    let mut ids = Vec::new();
    for hook in builtin_hooks() {
        let id = registry.register(hook).await;
        ids.push(id);
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_hooks_not_empty() {
        let hooks = builtin_hooks();
        assert!(!hooks.is_empty());
    }

    #[tokio::test]
    async fn register_builtin_hooks_works() {
        let registry = HookRegistry::new();
        let ids = register_builtin_hooks(&registry).await;
        assert!(!ids.is_empty());
        assert_eq!(registry.len().await, ids.len());
    }
}
