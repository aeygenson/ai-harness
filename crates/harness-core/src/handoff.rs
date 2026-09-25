//! The handoff file (`handoff.json`) that every role writes when it finishes.

use serde::{Deserialize, Serialize};

/// Who wrote a handoff: one of the four AI roles, or Lisa's own decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Architect,
    Developer,
    Tester,
    Security,
    Human,
}

/// A role's decision about the work it received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    Rejected,
    NeedsHuman,
}

/// Where the work should go next: another role, the human, or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextStep {
    Architect,
    Developer,
    Tester,
    Security,
    Human,
    Done,
}

/// What a role did with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Created,
    Modified,
    Deleted,
    Read,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub action: FileAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/// A problem a role found, explaining why it rejected the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub severity: Severity,
    /// Where the problem is, for example `src/parser.rs:42`.
    pub location: Option<String>,
    pub description: String,
}

/// The whole `handoff.json` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub schema_version: u32,
    pub task_id: String,
    pub round: u32,
    pub role: Role,
    pub verdict: Verdict,
    pub next_role: NextStep,
    pub summary: String,
    #[serde(default)]
    pub skills_used: Vec<String>,
    #[serde(default)]
    pub files: Vec<FileChange>,
    #[serde(default)]
    pub issues: Vec<Issue>,
}

impl Handoff {
    /// Reads a handoff from JSON text, rejecting anything that does not match the format.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Writes the handoff as nicely indented JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TESTER_EXAMPLE: &str = r#"{
        "schema_version": 1,
        "task_id": "task-001",
        "round": 2,
        "role": "tester",
        "verdict": "rejected",
        "next_role": "developer",
        "summary": "2 of 5 tests fail on empty input.",
        "skills_used": ["write-unit-tests"],
        "files": [{ "path": "src/parser.rs", "action": "read" }],
        "issues": [
            { "severity": "high", "location": "src/parser.rs:42",
              "description": "Panics when input is empty." }
        ]
    }"#;

    #[test]
    fn reads_the_tester_example() {
        let handoff = Handoff::from_json(TESTER_EXAMPLE).unwrap();
        assert_eq!(handoff.role, Role::Tester);
        assert_eq!(handoff.verdict, Verdict::Rejected);
        assert_eq!(handoff.next_role, NextStep::Developer);
        assert_eq!(handoff.issues[0].severity, Severity::High);
    }

    #[test]
    fn reads_every_example_in_the_docs() {
        let doc = include_str!("../../../docs/handoff-format.md");
        let examples: Vec<&str> = doc
            .split("```json")
            .skip(1)
            .map(|block| block.split("```").next().unwrap())
            .filter(|block| block.contains("\"schema_version\""))
            .collect();
        assert!(!examples.is_empty());
        for example in examples {
            Handoff::from_json(example).unwrap();
        }
    }

    #[test]
    fn rejects_an_unknown_verdict() {
        let bad = TESTER_EXAMPLE.replace("\"rejected\"", "\"maybe\"");
        assert!(Handoff::from_json(&bad).is_err());
    }

    #[test]
    fn survives_a_round_trip() {
        let handoff = Handoff::from_json(TESTER_EXAMPLE).unwrap();
        let again = Handoff::from_json(&handoff.to_json().unwrap()).unwrap();
        assert_eq!(handoff, again);
    }
}
