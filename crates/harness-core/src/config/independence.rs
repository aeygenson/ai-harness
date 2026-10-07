//! Is the role that reviews the code independent of the role that wrote it?
//!
//! The same agent with the same model tends to have the same blind spots: a
//! mistake it made while writing the code is a mistake it will easily miss
//! while checking it. The harness only warns about this (on the Roles tab and
//! in `harness doctor`); it is not forbidden, since not everyone has several
//! agents.

use std::collections::BTreeMap;

use super::RoleConfig;
use crate::task::handoff::Role;

/// Does the Security role use the same agent and model as the Developer?
///
/// A missing model means "the agent's default": two defaults of one agent
/// are the same model. When one role names a model and the other uses the
/// default, nobody can tell, so that is not counted as the same.
pub fn security_same_as_developer(roles: &BTreeMap<Role, RoleConfig>) -> bool {
    let (Some(developer), Some(security)) =
        (roles.get(&Role::Developer), roles.get(&Role::Security))
    else {
        return false;
    };
    developer.agent == security.agent && developer.model == security.model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentKind;

    fn role(agent: AgentKind, model: Option<&str>) -> RoleConfig {
        RoleConfig {
            agent,
            model: model.map(String::from),
            effort: None,
            skills: vec![],
            always_skills: vec![],
            mcp: vec![],
            plugins: vec![],
        }
    }

    fn roles(developer: RoleConfig, security: RoleConfig) -> BTreeMap<Role, RoleConfig> {
        BTreeMap::from([(Role::Developer, developer), (Role::Security, security)])
    }

    #[test]
    fn the_same_agent_and_model_is_not_independent() {
        let same = roles(
            role(AgentKind::Claude, Some("opus")),
            role(AgentKind::Claude, Some("opus")),
        );
        let both_default = roles(role(AgentKind::Codex, None), role(AgentKind::Codex, None));

        assert!(security_same_as_developer(&same));
        assert!(security_same_as_developer(&both_default));
    }

    #[test]
    fn another_agent_or_model_is_independent() {
        let other_agent = roles(role(AgentKind::Claude, None), role(AgentKind::Codex, None));
        let other_model = roles(
            role(AgentKind::Claude, Some("opus")),
            role(AgentKind::Claude, Some("sonnet")),
        );
        let unknown = roles(
            role(AgentKind::Claude, Some("opus")),
            role(AgentKind::Claude, None),
        );

        assert!(!security_same_as_developer(&other_agent));
        assert!(!security_same_as_developer(&other_model));
        assert!(!security_same_as_developer(&unknown));
        assert!(!security_same_as_developer(&BTreeMap::new()));
    }
}
