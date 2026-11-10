//! What the roles wanted to do but were refused, read from their logs.
//!
//! The roles run without a person to ask, so an action outside a role's
//! rights is refused at once. Three shapes in the logs are known:
//!
//! - Claude Code (`stream-json`) ends with a `result` line whose
//!   `permission_denials` list the refused tool calls; a shell call carries
//!   its command line in `tool_input.command`.
//! - Antigravity's `result` event lists `denied_actions`.
//! - The ACP adapter writes `{"type":"harness_refused","action":"..."}`
//!   for every permission request the harness turned down.
//!
//! Like `usage`, this reads the saved logs (`agent.log` and the failed
//! attempts' logs), so tasks that ran before this was written count too.
//! The logs come from the agents, so every action is cleaned with
//! [`text::safe_line`] before it is kept. It is only shown: nothing here is
//! ever allowed from it, because the agent chose what it tried. The user
//! decides with `harness allow-command`. An agent can also print a line in
//! one of these shapes itself, so a row is a hint, not a proof.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::task::handoff::Role;
use crate::text;

/// The longest action text kept: enough for a command line, short enough
/// for one table row.
const MAX_ACTION_CHARS: usize = 200;

/// One action refused to one role, summed over the counted tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusalCount {
    /// The role that was refused.
    pub role: Role,
    /// What it wanted to do: the command line for a shell call, otherwise
    /// the tool and its target.
    pub action: String,
    /// How many times it was refused.
    pub count: usize,
    /// The tasks it happened in.
    pub tasks: Vec<String>,
}

/// Every refused action in `log`, in the order found.
pub fn from_log(log: &str) -> Vec<String> {
    let mut found = Vec::new();
    // Lines that are not JSON (stderr, plain text) are skipped.
    for event in log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if event["type"] == "result" {
            claude_denials(&event, &mut found);
        } else if event["type"] == "harness_refused" {
            if let Some(action) = event["action"].as_str() {
                found.push(clean(action));
            }
        } else if event["event"] == "result" {
            for action in list(&event["result"]["denied_actions"]) {
                let name = action["display_name"]
                    .as_str()
                    .or(action["action"].as_str());
                found.push(clean(name.unwrap_or("an action")));
            }
        }
    }
    found
}

/// Sums the refusals of every task (the task id with its (role, action)
/// pairs), most frequent first.
pub fn count(tasks: &[(String, Vec<(Role, String)>)]) -> Vec<RefusalCount> {
    let mut rows: BTreeMap<(Role, String), RefusalCount> = BTreeMap::new();
    for (task_id, refusals) in tasks {
        for (role, action) in refusals {
            let row = rows
                .entry((*role, action.clone()))
                .or_insert_with(|| RefusalCount {
                    role: *role,
                    action: action.clone(),
                    count: 0,
                    tasks: Vec::new(),
                });
            row.count += 1;
            super::collect::push_new(&mut row.tasks, task_id.clone());
        }
    }
    let mut rows: Vec<RefusalCount> = rows.into_values().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.count));
    rows
}

/// Claude Code's `permission_denials` in its final `result` line.
fn claude_denials(event: &Value, found: &mut Vec<String>) {
    for denial in list(&event["permission_denials"]) {
        let tool = denial["tool_name"].as_str().unwrap_or("a tool");
        let input = &denial["tool_input"];
        if let Some(command) = input["command"].as_str().filter(|_| tool == "Bash") {
            found.push(clean(command));
        } else if let Some(path) = input["file_path"].as_str() {
            found.push(clean(&format!("{tool} {path}")));
        } else {
            found.push(clean(tool));
        }
    }
}

/// The items of a JSON list; nothing when `value` is not a list.
fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// The agent's text, safe to show on one line.
fn clean(action: &str) -> String {
    text::safe_line(action, MAX_ACTION_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r#"{"type":"assistant","message":{}}
{"type":"result","is_error":false,"permission_denials":[{"tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"trivy fs --quiet ."}},{"tool_name":"Edit","tool_use_id":"t2","tool_input":{"file_path":"src/main.rs"}}]}"#;

    #[test]
    fn claude_denials_give_the_command_line_or_the_tool_and_file() {
        assert_eq!(from_log(CLAUDE), ["trivy fs --quiet .", "Edit src/main.rs"]);
    }

    #[test]
    fn antigravity_and_acp_refusals_are_read_and_other_lines_skipped() {
        let log = "agent: antigravity\nnot json\n\
            {\"event\":\"result\",\"result\":{\"status\":\"SUCCESS\",\"denied_actions\":[{\"display_name\":\"RunCommand\"}]}}\n\
            harness refused Run trivy: not allowed\n\
            {\"action\":\"trivy fs\",\"type\":\"harness_refused\"}\n";
        assert_eq!(from_log(log), ["RunCommand", "trivy fs"]);
    }

    #[test]
    fn control_characters_from_the_agent_are_removed() {
        let log = r#"{"type":"harness_refused","action":"evil\u001b[2Jx\nnext"}"#;
        assert_eq!(from_log(log), ["evilx next"]);
    }

    #[test]
    fn counts_join_tasks_most_frequent_first() {
        let tasks = vec![
            (
                "task-001".to_string(),
                vec![
                    (Role::Security, "ls".to_string()),
                    (Role::Security, "trivy fs .".to_string()),
                ],
            ),
            (
                "task-002".to_string(),
                vec![(Role::Security, "trivy fs .".to_string())],
            ),
        ];
        let rows = count(&tasks);
        assert_eq!(rows[0].action, "trivy fs .");
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].tasks, ["task-001", "task-002"]);
        assert_eq!(rows[1].action, "ls");
        assert_eq!(rows.len(), 2);
    }
}
