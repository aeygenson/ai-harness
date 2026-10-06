//! Builds the prompt an agent receives for one role.
//!
//! The role's own `prompt.md` comes in a later stage.
//!
//! The agent works in *another* project, so the prompt must contain everything it
//! needs, including the exact `handoff.json` format: a first live run showed that
//! an agent told to "see docs/handoff-format.md" invents its own fields.

use std::path::Path;

use crate::skills::RoleSkills;
use crate::task::handoff::{FileAction, FileChange, Handoff, NextStep, Role, Verdict};
use crate::task::permissions::{self, WriteRule};
use crate::task::routes;
use crate::task::TaskState;

/// Builds the whole prompt an agent receives for `role` in this task.
/// It holds the task, the previous handoff, what the role may change, its skills
/// and the exact `handoff.json` format, to be written in `output_dir`.
pub fn build(
    role: Role,
    task_description: &str,
    state: &TaskState,
    previous: Option<&Handoff>,
    output_dir: &Path,
    skills: &RoleSkills,
) -> String {
    let mut prompt = format!(
        "You are the {role:?} in a team of AI roles.\n\
         Task {task} (round {round}):\n{task_description}\n\n",
        task = state.task_id,
        round = state.round,
    );
    if let Some(previous) = previous {
        let json = serde_json::to_string_pretty(previous).unwrap_or_default();
        prompt.push_str(&format!(
            "The previous step was done by the {:?}. Its handoff:\n{json}\n\n",
            previous.role
        ));
    }
    if let Some(task_dir) = output_dir.parent() {
        prompt.push_str(&format!(
            "Notes and handoffs of earlier steps are in {}/round-XX/NN-role/ \
             (notes.md, handoff.json); read them if you need more context.\n\n",
            task_dir.display()
        ));
    }
    prompt.push_str(&match permissions::rule_for(role) {
        WriteRule::Anything => {
            "You may change any project file except .harness/ and agent settings.\n".to_string()
        }
        WriteRule::Only { folders, files: [] } => format!(
            "You may change only files inside folders named {}.\n",
            folders.join(" or ")
        ),
        WriteRule::Only { folders, files } => format!(
            "You may change only files inside folders named {}, and test files \
             anywhere named like {}.\n",
            folders.join(" or "),
            files.join(", ")
        ),
        WriteRule::Nothing => "Do not change any project files; only read them.\n".to_string(),
    });
    prompt.push_str(&skills_text(skills));
    prompt.push_str(&format!(
        "\nWhen you finish, write two files into {dir}:\n\
         - notes.md: short notes for Lisa.\n\
         - handoff.json: your result, exactly in the format below.\n\
         Do not commit to git; the harness does that.\n\n",
        dir = output_dir.display(),
    ));
    prompt.push_str(&handoff_format(role, state));
    prompt
}

/// The role's skills: its base and the always-on ones in full, the others as
/// a list of files.
fn skills_text(skills: &RoleSkills) -> String {
    let mut text = String::new();
    for skill in &skills.base {
        text.push_str(&format!("\n{}\n", skill.body));
    }
    for skill in &skills.always {
        text.push_str(&format!(
            "\nAlways follow the skill \"{}\":\n{}\n",
            skill.name, skill.body
        ));
    }
    if !skills.on_demand.is_empty() {
        text.push_str("\nSkills you can use: read a skill's file when it fits your work.\n");
        for skill in &skills.on_demand {
            text.push_str(&format!(
                "- {}: {} (file {})\n",
                skill.name,
                skill.description,
                skill.path.display()
            ));
        }
    }
    if !skills.is_empty() {
        text.push_str("List the names of the skills you used in skills_used.\n");
    }
    text
}

/// The `handoff.json` rules for this role, with a ready example.
fn handoff_format(role: Role, state: &TaskState) -> String {
    let mut text = String::from(
        "handoff.json format (JSON, these fields only, no others):\n\
         - schema_version: 1\n\
         - task_id, round, role: exactly as in the example\n\
         - verdict: \"approved\" (your work is done / the work you checked is good), \
         \"rejected\" (the work you received has problems) or \
         \"needs_human\" (you need Lisa to decide or answer)\n\
         - next_role: who works next (see the list below)\n\
         - summary: one short sentence\n\
         - skills_used: list of skill names you used, or []\n\
         - files: list of {\"path\": \"...\", \"action\": \"created\" | \"modified\" | \"deleted\" | \"read\"}\n\
         - issues: list of {\"severity\": \"low\" | \"medium\" | \"high\" | \"critical\", \
         \"location\": \"file:line\" or null, \"description\": \"...\"}; \
         must not be empty when the verdict is \"rejected\"\n\n\
         Allowed next_role for you:\n",
    );
    for verdict in [Verdict::Approved, Verdict::Rejected, Verdict::NeedsHuman] {
        let next = routes::allowed_next(role, verdict);
        let names = if next.is_empty() {
            "(not allowed for your role)".to_string()
        } else {
            next.iter()
                .map(|n| quoted(*n))
                .collect::<Vec<_>>()
                .join(" or ")
        };
        text.push_str(&format!("- verdict {}: {names}\n", quoted_verdict(verdict)));
    }
    let json = serde_json::to_string_pretty(&example(role, state)).unwrap_or_default();
    text.push_str(&format!("\nExample:\n{json}\n"));
    text
}

/// A valid approved handoff for this role and round.
fn example(role: Role, state: &TaskState) -> Handoff {
    let next = routes::allowed_next(role, Verdict::Approved)
        .first()
        .copied()
        .unwrap_or(NextStep::Done);
    Handoff {
        schema_version: 1,
        task_id: state.task_id.clone(),
        round: state.round,
        role,
        verdict: Verdict::Approved,
        next_role: next,
        summary: "One sentence about what you did.".to_string(),
        skills_used: vec![],
        files: vec![FileChange {
            path: "docs/design.md".to_string(),
            action: FileAction::Created,
        }],
        issues: vec![],
    }
}

/// How a value is written in JSON, e.g. `"developer"`.
fn quoted(next: NextStep) -> String {
    serde_json::to_string(&next).unwrap_or_default()
}

fn quoted_verdict(verdict: Verdict) -> String {
    serde_json::to_string(&verdict).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Stage, DEFAULT_MAX_ROUNDS};

    fn prompt_for(role: Role) -> String {
        let mut state = TaskState::new("task-007", DEFAULT_MAX_ROUNDS);
        state.round = 2;
        state.stage = Stage::Working(role);
        build(
            role,
            "Build a parser",
            &state,
            None,
            Path::new("/p/.harness/runs/task-007/inbox"),
            &RoleSkills::default(),
        )
    }

    /// The JSON after "Example:" in the prompt.
    fn example_json(prompt: &str) -> &str {
        prompt.split("Example:\n").nth(1).unwrap().trim()
    }

    #[test]
    fn the_example_in_the_prompt_is_a_valid_handoff_for_this_step() {
        for role in [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
        ] {
            let prompt = prompt_for(role);
            let handoff = Handoff::from_json(example_json(&prompt)).unwrap();
            assert_eq!(handoff.task_id, "task-007");
            assert_eq!(handoff.round, 2);
            assert_eq!(handoff.role, role);
            // The harness itself would accept it.
            let mut state = TaskState::new("task-007", DEFAULT_MAX_ROUNDS);
            state.round = 2;
            state.stage = Stage::Working(role);
            state.apply(&handoff).unwrap();
        }
    }

    #[test]
    fn the_prompt_does_not_point_to_files_the_project_may_not_have() {
        assert!(!prompt_for(Role::Architect).contains("handoff-format.md"));
    }

    #[test]
    fn the_prompt_says_where_earlier_notes_are() {
        assert!(prompt_for(Role::Security)
            .contains("earlier steps are in /p/.harness/runs/task-007/round-XX/NN-role/"));
    }

    #[test]
    fn the_prompt_lists_skills_and_includes_always_skills_in_full() {
        use crate::skills::{Skill, Source};
        use std::path::PathBuf;

        let skill = |name: &str, body: &str| Skill {
            name: name.to_string(),
            description: format!("About {name}."),
            path: PathBuf::from(format!("/p/.harness/skills/{name}.md")),
            body: body.to_string(),
            source: Source::Own,
        };
        let skills = RoleSkills {
            base: vec![skill("developer", "# Developer\nImplement the design.")],
            on_demand: vec![skill("rust-errors", "Use thiserror.")],
            always: vec![skill("style", "Run clippy.")],
        };
        let state = TaskState::new("task-007", DEFAULT_MAX_ROUNDS);
        let prompt = build(
            Role::Developer,
            "Build a parser",
            &state,
            None,
            Path::new("/p/.harness/runs/task-007/inbox"),
            &skills,
        );
        assert!(prompt.contains(
            "- rust-errors: About rust-errors. (file /p/.harness/skills/rust-errors.md)"
        ));
        // Only the list: the agent reads the file itself.
        assert!(!prompt.contains("Use thiserror."));
        assert!(prompt.contains("Always follow the skill \"style\":\nRun clippy."));
        assert!(prompt.contains("in skills_used"));
        // The role's base is in full, without a heading of its own.
        assert!(prompt.contains("\n# Developer\nImplement the design.\n"));
    }

    #[test]
    fn a_role_without_skills_gets_no_skill_text() {
        assert!(!prompt_for(Role::Tester).contains("skill \""));
        assert!(!prompt_for(Role::Tester).contains("Skills you can use"));
    }

    #[test]
    fn the_prompt_lists_the_allowed_next_roles() {
        let architect = prompt_for(Role::Architect);
        assert!(
            architect.contains("verdict \"approved\": \"human\""),
            "{architect}"
        );
        let tester = prompt_for(Role::Tester);
        assert!(tester.contains("verdict \"rejected\": \"developer\" or \"architect\""));
    }
}
