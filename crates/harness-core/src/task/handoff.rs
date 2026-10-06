//! The handoff file (`handoff.json`) that every role writes when it finishes.

use std::fmt;
use std::str::FromStr;

use serde::de::IntoDeserializer;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::text;

/// The format version this harness reads and writes (`schema_version`).
pub const SCHEMA_VERSION: u32 = 1;

/// The longest summary or issue description, in characters. The format asks
/// for a sentence or two; this leaves room for a few paragraphs, but stops an
/// agent from pasting a whole log into the history and the next prompt.
pub const MAX_TEXT_CHARS: usize = 4000;

/// The longest file path, issue location or skill name, in characters.
pub const MAX_NAME_CHARS: usize = 500;

/// Who wrote a handoff: one of the four AI roles, or Lisa's own decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Architect,
    Developer,
    Tester,
    Security,
    Human,
}

impl Role {
    /// The role's name as written in `handoff.json` and `harness.toml`, e.g. `"developer"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Architect => "architect",
            Role::Developer => "developer",
            Role::Tester => "tester",
            Role::Security => "security",
            Role::Human => "human",
        }
    }
}

/// `{role}` in `format!` prints the same name as [`Role::as_str`].
impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write_str`) keeps widths like `{role:<10}` working.
        f.pad(self.as_str())
    }
}

/// The error when a text is not one of the role names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a role (architect, developer, tester, security or human)")]
pub struct UnknownRole(pub String);

/// Lets `"developer".parse::<Role>()` turn a name back into a role.
impl FromStr for Role {
    type Err = UnknownRole;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "architect" => Ok(Role::Architect),
            "developer" => Ok(Role::Developer),
            "tester" => Ok(Role::Tester),
            "security" => Ok(Role::Security),
            "human" => Ok(Role::Human),
            _ => Err(UnknownRole(text.to_string())),
        }
    }
}

/// A role's decision about the work it received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    Rejected,
    NeedsHuman,
}

/// Where the work should go next: to a role (including Lisa), or finished.
///
/// In JSON it is a plain string: `"developer"`, `"human"`, ... or `"done"`.
/// Because `Done` is not a `Role`, a handoff can never claim to be written by "done".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    To(Role),
    Done,
}

impl Serialize for NextStep {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // A role is written exactly as `Role` writes itself, e.g. "developer".
            NextStep::To(role) => role.serialize(serializer),
            NextStep::Done => serializer.serialize_str("done"),
        }
    }
}

impl<'de> Deserialize<'de> for NextStep {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == "done" {
            return Ok(NextStep::Done);
        }
        // Anything else must be a role name; `Role` decides what is valid.
        let as_role = IntoDeserializer::<D::Error>::into_deserializer(text);
        Role::deserialize(as_role).map(NextStep::To)
    }
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
    /// The agent's texts come back without terminal control characters.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let mut handoff: Self = serde_json::from_str(text)?;
        handoff.make_text_safe();
        Ok(handoff)
    }

    /// Removes terminal control characters from every text the agent wrote, so
    /// the handoff is safe to show (see [`crate::text::safe`]).
    pub fn make_text_safe(&mut self) {
        self.summary = text::safe(&self.summary);
        for skill in &mut self.skills_used {
            *skill = text::safe(skill);
        }
        for file in &mut self.files {
            file.path = text::safe(&file.path);
        }
        for issue in &mut self.issues {
            issue.description = text::safe(&issue.description);
            // `as_mut` gives a `&mut String` inside the `Option` without taking it out.
            if let Some(location) = issue.location.as_mut() {
                *location = text::safe(location);
            }
        }
    }

    /// The first text that is too long, as `(field name, limit)`; `None` if all fit.
    pub fn too_long_field(&self) -> Option<(&'static str, usize)> {
        let too_long = |text: &str, limit: usize| text.chars().count() > limit;
        if too_long(&self.summary, MAX_TEXT_CHARS) {
            return Some(("summary", MAX_TEXT_CHARS));
        }
        if self.skills_used.iter().any(|s| too_long(s, MAX_NAME_CHARS)) {
            return Some(("skills_used", MAX_NAME_CHARS));
        }
        if self.files.iter().any(|f| too_long(&f.path, MAX_NAME_CHARS)) {
            return Some(("files.path", MAX_NAME_CHARS));
        }
        for issue in &self.issues {
            if too_long(&issue.description, MAX_TEXT_CHARS) {
                return Some(("issues.description", MAX_TEXT_CHARS));
            }
            if issue
                .location
                .as_deref()
                .is_some_and(|l| too_long(l, MAX_NAME_CHARS))
            {
                return Some(("issues.location", MAX_NAME_CHARS));
            }
        }
        None
    }

    /// Writes the handoff as nicely indented JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_match_the_json_and_parse_back() {
        let roles = [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
            Role::Human,
        ];
        for role in roles {
            let json = serde_json::to_string(&role).unwrap();
            assert_eq!(json, format!("\"{}\"", role.as_str()));
            assert_eq!(role.to_string(), role.as_str());
            assert_eq!(format!("{role:<10}|").len(), 11);
            assert_eq!(role.as_str().parse::<Role>(), Ok(role));
        }
        assert_eq!("done".parse::<Role>(), Err(UnknownRole("done".to_string())));
    }

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
        assert_eq!(handoff.next_role, NextStep::To(Role::Developer));
        assert_eq!(handoff.issues[0].severity, Severity::High);
    }

    #[test]
    fn reads_every_example_in_the_docs() {
        let doc = include_str!("../../../../docs/handoff-format.md");
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
    fn agent_texts_lose_terminal_control_characters() {
        let tricky = TESTER_EXAMPLE
            .replace("Panics when input", "Panics\\u001b[2J when input")
            .replace("src/parser.rs:42", "src/parser.rs:42\\u202e");
        let handoff = Handoff::from_json(&tricky).unwrap();
        assert_eq!(handoff.issues[0].description, "Panics when input is empty.");
        assert_eq!(
            handoff.issues[0].location.as_deref(),
            Some("src/parser.rs:42")
        );
    }

    #[test]
    fn next_role_is_a_role_or_done() {
        let done =
            TESTER_EXAMPLE.replace("\"next_role\": \"developer\"", "\"next_role\": \"done\"");
        assert_eq!(Handoff::from_json(&done).unwrap().next_role, NextStep::Done);

        let banana =
            TESTER_EXAMPLE.replace("\"next_role\": \"developer\"", "\"next_role\": \"banana\"");
        assert!(Handoff::from_json(&banana).is_err());
    }

    #[test]
    fn done_is_not_a_role() {
        let bad = TESTER_EXAMPLE.replace("\"role\": \"tester\"", "\"role\": \"done\"");
        assert!(Handoff::from_json(&bad).is_err());
    }

    #[test]
    fn rejects_an_unknown_verdict() {
        let bad = TESTER_EXAMPLE.replace("\"rejected\"", "\"maybe\"");
        assert!(Handoff::from_json(&bad).is_err());
    }

    #[test]
    fn texts_within_the_limits_pass() {
        let handoff = Handoff::from_json(TESTER_EXAMPLE).unwrap();
        assert_eq!(handoff.too_long_field(), None);
    }

    #[test]
    fn the_first_text_over_its_limit_is_named() {
        let mut handoff = Handoff::from_json(TESTER_EXAMPLE).unwrap();
        handoff.issues[0].location = Some("x".repeat(MAX_NAME_CHARS + 1));
        assert_eq!(
            handoff.too_long_field(),
            Some(("issues.location", MAX_NAME_CHARS))
        );
        // Letters, not bytes: a Russian summary of the full length still fits.
        handoff.summary = "я".repeat(MAX_TEXT_CHARS);
        assert_eq!(
            handoff.too_long_field(),
            Some(("issues.location", MAX_NAME_CHARS))
        );
        handoff.summary.push('я');
        assert_eq!(handoff.too_long_field(), Some(("summary", MAX_TEXT_CHARS)));
    }

    #[test]
    fn survives_a_round_trip() {
        let handoff = Handoff::from_json(TESTER_EXAMPLE).unwrap();
        let again = Handoff::from_json(&handoff.to_json().unwrap()).unwrap();
        assert_eq!(handoff, again);
    }
}
