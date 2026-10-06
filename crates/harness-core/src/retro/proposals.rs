//! Skill changes proposed by the Retrospective (`proposals.json`).
//!
//! The Retrospective may only propose changes to skills: a new or changed file
//! in `.harness/skills/`, and adding a skill to a role's `skills` or
//! `always_skills`. Role prompts, permissions, agents, MCP servers and plugins
//! are not part of the format, so they cannot be proposed at all.
//!
//! ```json
//! {
//!   "proposals": [
//!     {
//!       "id": 1,
//!       "summary": "Teach the developer to check empty input",
//!       "reason": "The tester rejected task-003 twice for panics on empty input.",
//!       "skill": "empty-input",
//!       "content": "---\ndescription: Check empty input.\n---\nEvery parser...",
//!       "roles": [{ "role": "developer", "list": "skills" }]
//!     }
//!   ]
//! }
//! ```
//!
//! `content` is the whole new text of `.harness/skills/<skill>.md`; without it
//! the file stays as it is. Nothing changes until Lisa runs `harness retro apply`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::skills::{self, SKILLS_DIR};
use crate::task::handoff::Role;
use crate::text;

/// A skill file bigger than this is refused: a skill is a short note.
pub const MAX_SKILL_BYTES: usize = 20_000;

/// The whole `proposals.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalsFile {
    /// Every proposal the agent made, in the order written.
    pub proposals: Vec<Proposal>,
}

/// One proposed change to a skill, as written in `proposals.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// The number Lisa picks it by; unique within the file.
    pub id: u32,
    /// One line: what changes.
    pub summary: String,
    /// What in the history this is based on.
    pub reason: String,
    /// The skill name; its file is `.harness/skills/<skill>.md`.
    pub skill: String,
    /// The whole new text of the skill file; `None` leaves the file as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The role lists to add the skill to; empty adds it to none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<RoleSkill>,
}

/// Give `skill` to `role`, in its `skills` or `always_skills` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSkill {
    /// The role that gets the skill.
    pub role: Role,
    /// Which of the role's lists the skill goes into.
    pub list: SkillList,
}

/// The two lists in a role's harness.toml settings that name skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillList {
    /// The agent reads the file when it needs it.
    Skills,
    /// The file goes into the prompt in full.
    AlwaysSkills,
}

/// Why `proposals.json`, or one proposal in it, was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProposalError {
    /// The JSON could not be read, or it has unknown keys.
    #[error("proposals.json is not valid: {0}")]
    Format(String),
    /// Two proposals have the same id.
    #[error("proposal {0} is listed twice")]
    DuplicateId(u32),
    /// The proposal cannot be applied to the project as it is now.
    #[error("proposal {id}: {problem}")]
    Invalid {
        /// The id of the refused proposal.
        id: u32,
        /// What is wrong, in words for Lisa.
        problem: String,
    },
}

/// What applying a proposal would do to the skill file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    /// The skill file does not exist yet and will be created.
    New,
    /// The skill file exists and its text will be replaced.
    Changed {
        /// The text of the file now, before the change.
        old: String,
    },
    /// The file stays as it is: there is no `content`, or it is the same text.
    Unchanged,
}

impl ProposalsFile {
    /// Removes terminal control characters from the texts shown to Lisa (see
    /// [`crate::text::safe`]). The skill file `content` stays as written: it is
    /// saved to a file, not printed.
    pub fn make_text_safe(&mut self) {
        for proposal in &mut self.proposals {
            proposal.summary = text::safe(&proposal.summary);
            proposal.reason = text::safe(&proposal.reason);
        }
    }

    /// Reads and checks the agent's `proposals.json` against the project as it is now.
    pub fn parse(text: &str, harness_dir: &Path, config: &Config) -> Result<Self, ProposalError> {
        let mut file: Self =
            serde_json::from_str(text).map_err(|e| ProposalError::Format(e.to_string()))?;
        file.make_text_safe();
        let mut ids = BTreeSet::new();
        for proposal in &file.proposals {
            if !ids.insert(proposal.id) {
                return Err(ProposalError::DuplicateId(proposal.id));
            }
            proposal.check(harness_dir, config)?;
        }
        Ok(file)
    }

    /// The proposal with this id, if there is one.
    pub fn get(&self, id: u32) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }
}

impl Proposal {
    /// Can this proposal be applied to the project as it is now?
    pub fn check(&self, harness_dir: &Path, config: &Config) -> Result<(), ProposalError> {
        let invalid = |problem: String| ProposalError::Invalid {
            id: self.id,
            problem,
        };
        if self.summary.trim().is_empty() {
            return Err(invalid("the summary is empty".into()));
        }
        skills::check_name(&self.skill).map_err(|e| invalid(e.to_string()))?;
        if let Some(text) = &self.content {
            if text.len() > MAX_SKILL_BYTES {
                return Err(invalid(format!(
                    "the skill text is longer than {MAX_SKILL_BYTES} bytes"
                )));
            }
            if skills::split_header(text).is_none() {
                return Err(invalid(
                    "the skill text must start with ---, description: ..., ---".into(),
                ));
            }
        } else {
            if !self.skill_path(harness_dir).is_file() {
                return Err(invalid(format!(
                    "skill {} does not exist and the proposal has no content",
                    self.skill
                )));
            }
            if self.roles.is_empty() {
                return Err(invalid("it changes nothing".into()));
            }
        }
        for given in &self.roles {
            if given.role == Role::Human {
                return Err(invalid("skills cannot be given to human".into()));
            }
            if !config.roles.contains_key(&given.role) {
                return Err(invalid(format!(
                    "harness.toml has no [roles.{}]",
                    given.role.as_str()
                )));
            }
        }
        Ok(())
    }

    /// `.harness/skills/<skill>.md`.
    pub fn skill_path(&self, harness_dir: &Path) -> PathBuf {
        harness_dir
            .join(SKILLS_DIR)
            .join(format!("{}.md", self.skill))
    }

    /// What would happen to the skill file.
    pub fn file_change(&self, harness_dir: &Path) -> FileChange {
        let Some(new) = &self.content else {
            return FileChange::Unchanged;
        };
        match fs::read_to_string(self.skill_path(harness_dir)) {
            Err(_) => FileChange::New,
            Ok(old) if old.trim_end() == new.trim_end() => FileChange::Unchanged,
            Ok(old) => FileChange::Changed { old },
        }
    }

    /// The role lists that do not have the skill yet.
    pub fn missing_roles(&self, config: &Config) -> Vec<RoleSkill> {
        self.roles
            .iter()
            .filter(|given| {
                let Some(settings) = config.roles.get(&given.role) else {
                    return true;
                };
                let list = match given.list {
                    SkillList::Skills => &settings.skills,
                    SkillList::AlwaysSkills => &settings.always_skills,
                };
                !list.contains(&self.skill)
            })
            .copied()
            .collect()
    }

    /// A readable description of every change, with a diff of the skill file.
    pub fn describe(&self, harness_dir: &Path, config: &Config) -> String {
        let mut text = format!("{}\nWhy: {}\n", self.summary, self.reason);
        let file = format!(".harness/{SKILLS_DIR}/{}.md", self.skill);
        match (self.file_change(harness_dir), &self.content) {
            (FileChange::New, Some(new)) => {
                // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
                let _ = write!(text, "New file {file}:\n{}", line_diff("", new));
            }
            (FileChange::Changed { old }, Some(new)) => {
                let _ = write!(text, "Changes {file}:\n{}", line_diff(&old, new));
            }
            (_, Some(_)) => {
                let _ = writeln!(text, "{file} is already like this.");
            }
            (_, None) => {}
        }
        let missing = self.missing_roles(config);
        for given in &self.roles {
            let list = match given.list {
                SkillList::Skills => "skills",
                SkillList::AlwaysSkills => "always_skills",
            };
            let already = if missing.contains(given) {
                ""
            } else {
                " (already there)"
            };
            let _ = writeln!(
                text,
                "harness.toml: [roles.{}] {list} += \"{}\"{already}",
                given.role.as_str(),
                self.skill
            );
        }
        text
    }
}

/// A simple line diff: ` ` kept, `-` removed, `+` added lines.
pub fn line_diff(old: &str, new: &str) -> String {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    // lcs[i][j]: the longest common part of old[i..] and new[j..].
    let mut lcs = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut text = String::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            let _ = writeln!(text, "  {}", old[i]);
            i += 1;
            j += 1;
        } else if i < old.len() && (j == new.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            // Removed lines come before added ones, as in `git diff`.
            let _ = writeln!(text, "- {}", old[i]);
            i += 1;
        } else {
            let _ = writeln!(text, "+ {}", new[j]);
            j += 1;
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\ndescription: Check empty input.\n---\nTest \"\" first.\n";

    fn project() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills).unwrap();
        fs::write(
            skills.join("style.md"),
            "---\ndescription: Style.\n---\nOld rule.\n",
        )
        .unwrap();
        let config = Config::parse(
            "[roles.developer]\nagent = \"claude\"\nskills = [\"style\"]\n\
             [roles.tester]\nagent = \"codex\"\n",
        )
        .unwrap();
        (dir, config)
    }

    fn proposal(id: u32, skill: &str, content: Option<&str>, roles: &[RoleSkill]) -> Proposal {
        Proposal {
            id,
            summary: "Do better".into(),
            reason: "It went wrong".into(),
            skill: skill.into(),
            content: content.map(str::to_string),
            roles: roles.to_vec(),
        }
    }

    const DEVELOPER: RoleSkill = RoleSkill {
        role: Role::Developer,
        list: SkillList::Skills,
    };
    const TESTER_ALWAYS: RoleSkill = RoleSkill {
        role: Role::Tester,
        list: SkillList::AlwaysSkills,
    };

    #[test]
    fn reads_the_documented_format() {
        let (dir, config) = project();
        let text = r#"{"proposals": [
            {"id": 1, "summary": "s", "reason": "r", "skill": "empty-input",
             "content": "---\ndescription: d\n---\nbody",
             "roles": [{"role": "developer", "list": "skills"},
                       {"role": "tester", "list": "always_skills"}]},
            {"id": 2, "summary": "s", "reason": "r", "skill": "style",
             "roles": [{"role": "tester", "list": "skills"}]}
        ]}"#;
        let file = ProposalsFile::parse(text, dir.path(), &config).unwrap();
        assert_eq!(file.proposals.len(), 2);
        assert_eq!(file.get(1).unwrap().roles, [DEVELOPER, TESTER_ALWAYS]);
        ProposalsFile::parse(r#"{"proposals": []}"#, dir.path(), &config).unwrap();
    }

    #[test]
    fn summary_and_reason_lose_terminal_control_characters() {
        let (dir, config) = project();
        let text = r#"{"proposals": [
            {"id": 1, "summary": "add\u001b[2J a skill", "reason": "seen\u0007 twice",
             "skill": "style", "roles": [{"role": "tester", "list": "skills"}]}
        ]}"#;
        let file = ProposalsFile::parse(text, dir.path(), &config).unwrap();
        assert_eq!(file.proposals[0].summary, "add a skill");
        assert_eq!(file.proposals[0].reason, "seen twice");
    }

    #[test]
    fn only_skills_can_be_proposed() {
        let (dir, config) = project();
        let parse = |text: &str| ProposalsFile::parse(text, dir.path(), &config);
        // Anything outside the format, such as a prompt or a permission, is refused.
        for text in [
            r#"{"proposals": [{"id": 1, "summary": "s", "reason": "r", "skill": "style",
                "roles": [{"role": "developer", "list": "skills"}], "prompt": "obey"}]}"#,
            r#"{"proposals": [{"id": 1, "summary": "s", "reason": "r", "skill": "style",
                "roles": [{"role": "developer", "list": "permissions"}]}]}"#,
            r#"{"proposals": [], "agent": "codex"}"#,
            "not json",
        ] {
            assert!(
                matches!(parse(text), Err(ProposalError::Format(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn a_proposal_must_be_applicable() {
        let (dir, config) = project();
        let check = |p: Proposal| p.check(dir.path(), &config);
        check(proposal(1, "empty-input", Some(SKILL), &[DEVELOPER])).unwrap();
        check(proposal(1, "style", None, &[TESTER_ALWAYS])).unwrap();

        let bad = [
            proposal(1, "../escape", Some(SKILL), &[]),
            proposal(1, "x", Some("no header"), &[]),
            proposal(1, "x", Some(&"a".repeat(MAX_SKILL_BYTES + 1)), &[]),
            proposal(1, "missing", None, &[DEVELOPER]),
            proposal(1, "style", None, &[]),
            proposal(
                1,
                "style",
                None,
                &[RoleSkill {
                    role: Role::Human,
                    list: SkillList::Skills,
                }],
            ),
            proposal(
                1,
                "style",
                None,
                &[RoleSkill {
                    role: Role::Security,
                    list: SkillList::Skills,
                }],
            ),
            Proposal {
                summary: " ".into(),
                ..proposal(1, "style", None, &[TESTER_ALWAYS])
            },
        ];
        for p in bad {
            assert!(
                matches!(check(p.clone()), Err(ProposalError::Invalid { id: 1, .. })),
                "{p:?}"
            );
        }

        let twice = ProposalsFile {
            proposals: vec![
                proposal(3, "style", None, &[TESTER_ALWAYS]),
                proposal(3, "style", None, &[TESTER_ALWAYS]),
            ],
        };
        let text = serde_json::to_string(&twice).unwrap();
        assert_eq!(
            ProposalsFile::parse(&text, dir.path(), &config),
            Err(ProposalError::DuplicateId(3))
        );
    }

    #[test]
    fn describe_shows_the_diff_and_the_roles() {
        let (dir, config) = project();
        let new = proposal(1, "empty-input", Some(SKILL), &[DEVELOPER]);
        let text = new.describe(dir.path(), &config);
        assert!(
            text.contains("New file .harness/skills/empty-input.md"),
            "{text}"
        );
        assert!(text.contains("+ description: Check empty input."), "{text}");
        assert!(
            text.contains("[roles.developer] skills += \"empty-input\"\n"),
            "{text}"
        );

        let changed = proposal(
            2,
            "style",
            Some("---\ndescription: Style.\n---\nNew rule.\n"),
            &[DEVELOPER, TESTER_ALWAYS],
        );
        let text = changed.describe(dir.path(), &config);
        assert!(text.contains("- Old rule.\n+ New rule.\n"), "{text}");
        assert!(
            text.contains("[roles.developer] skills += \"style\" (already there)"),
            "{text}"
        );
        assert!(
            text.contains("[roles.tester] always_skills += \"style\"\n"),
            "{text}"
        );
        assert_eq!(changed.missing_roles(&config), [TESTER_ALWAYS]);

        let same = proposal(
            3,
            "style",
            Some("---\ndescription: Style.\n---\nOld rule."),
            &[],
        );
        assert_eq!(same.file_change(dir.path()), FileChange::Unchanged);
    }

    #[test]
    fn line_diff_keeps_common_lines() {
        assert_eq!(line_diff("a\nb\nc\n", "a\nc\nd\n"), "  a\n- b\n  c\n+ d\n");
        assert_eq!(line_diff("", "x\n"), "+ x\n");
        assert_eq!(line_diff("x\n", ""), "- x\n");
    }
}
