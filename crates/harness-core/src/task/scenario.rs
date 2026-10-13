//! What kind of project this is (the scenario) and which path a task takes
//! through its roles (the workflow).
//!
//! Both are enums on purpose: a new scenario or workflow is a new variant, and
//! the compiler then points at every `match` that must learn about it.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::task::handoff::Role;

/// What kind of project this is: it decides which roles work and which workflows exist.
///
/// Written as `scenario = "software"` in `harness.toml`; a project without the
/// line is `Software`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scenario {
    /// Writing software: architect, developer, tester and security.
    #[default]
    Software,
}

impl Scenario {
    /// The name as written in `harness.toml`, e.g. `"software"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Scenario::Software => "software",
        }
    }

    /// The AI roles of this scenario, in the order of the flow.
    ///
    /// Lisa (`Role::Human`) takes part in every scenario and is not listed.
    pub fn roles(self) -> &'static [Role] {
        match self {
            Scenario::Software => &[
                Role::Architect,
                Role::Developer,
                Role::Tester,
                Role::Security,
            ],
        }
    }

    /// The workflows a task of this scenario can follow.
    pub fn workflows(self) -> &'static [Workflow] {
        match self {
            Scenario::Software => &[Workflow::Task],
        }
    }

    /// The workflow a new task gets when nobody chooses one.
    pub fn default_workflow(self) -> Workflow {
        match self {
            Scenario::Software => Workflow::Task,
        }
    }
}

/// `{scenario}` in `format!` prints the same name as [`Scenario::as_str`].
impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// Which path a task takes through the roles. Saved in `state.json`.
///
/// A task that was saved before workflows existed reads as `Task`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Workflow {
    /// Software: architect, then Lisa approves the design, then developer,
    /// tester and security.
    #[default]
    Task,
}

impl Workflow {
    /// The role a new task of this workflow starts with.
    pub fn start(self) -> Role {
        match self {
            Workflow::Task => Role::Architect,
        }
    }

    /// Who gets the work when Lisa approves what waits for her approval and
    /// does not say who continues.
    pub fn after_approval(self) -> Role {
        match self {
            Workflow::Task => Role::Developer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scenario. The `match` stops the build when a new one is added
    /// and not listed here, so the tests below cannot miss it.
    fn all() -> Vec<Scenario> {
        let all = vec![Scenario::Software];
        for scenario in &all {
            match scenario {
                Scenario::Software => {}
            }
        }
        all
    }

    #[test]
    fn every_scenario_has_a_workflow_that_starts_with_one_of_its_roles() {
        for scenario in all() {
            assert!(!scenario.roles().is_empty(), "{scenario}");
            assert!(scenario.workflows().contains(&scenario.default_workflow()));
            for workflow in scenario.workflows() {
                assert!(scenario.roles().contains(&workflow.start()));
                assert!(scenario.roles().contains(&workflow.after_approval()));
            }
        }
    }

    #[test]
    fn software_keeps_the_four_roles_and_the_task_workflow() {
        assert_eq!(
            Scenario::Software.roles(),
            [
                Role::Architect,
                Role::Developer,
                Role::Tester,
                Role::Security
            ]
        );
        assert_eq!(Scenario::Software.default_workflow(), Workflow::Task);
        assert_eq!(Workflow::Task.start(), Role::Architect);
        assert_eq!(Workflow::Task.after_approval(), Role::Developer);
    }

    #[test]
    fn the_names_in_files_are_the_ones_lisa_writes() {
        assert_eq!(Scenario::default(), Scenario::Software);
        assert_eq!(Scenario::Software.to_string(), "software");
        assert_eq!(serde_json::to_string(&Workflow::Task).unwrap(), "\"task\"");
    }
}
