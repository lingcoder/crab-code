pub mod explore;
pub mod general_purpose;
pub mod plan;

use crate::definition::AgentDefinition;

pub fn builtin_agents() -> Vec<AgentDefinition> {
    vec![explore::agent(), plan::agent(), general_purpose::agent()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_agents_have_unique_types() {
        let agents = builtin_agents();
        let mut types: Vec<&str> = agents.iter().map(|a| a.agent_type.as_str()).collect();
        types.sort_unstable();
        types.dedup();
        assert_eq!(types.len(), agents.len());
    }

    #[test]
    fn all_builtin_agents_are_builtin_source() {
        for a in builtin_agents() {
            assert!(a.is_builtin(), "{} should be builtin", a.agent_type);
        }
    }
}
