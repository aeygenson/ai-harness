//! How many tokens a role used and what it cost, read from its `agent.log`.
//!
//! The agents print this themselves in their JSON output, which the harness
//! saves as `agent.log`. Two shapes are known:
//!
//! - Claude Code (`stream-json`) ends with one line
//!   `{"type":"result","total_cost_usd":0.12,"usage":{"input_tokens":...}}`
//!   that sums the whole run. With a subscription the cost is what the same
//!   work would cost through the API, not money that was paid.
//! - Codex (`--json`) prints `{"type":"turn.completed","usage":{...}}` after
//!   each turn, without a cost; the turns are added up.
//!
//! Other agents print no usage the harness knows yet, so their steps are
//! not counted. Reading the logs (not a new file) also counts tasks that ran
//! before this was written.

use serde::Serialize;
use serde_json::Value;

/// Tokens and cost of one step, or of several added up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Usage {
    /// Tokens the model read, cached ones included.
    pub input_tokens: u64,
    /// Tokens the model wrote.
    pub output_tokens: u64,
    /// The cost in US dollars; `None` when the agent does not report it.
    pub cost_usd: Option<f64>,
}

impl Usage {
    /// Adds `other` to this usage. A cost stays known if either side knows it.
    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
    }
}

/// The usage an agent printed in `log`; `None` if it printed none.
pub fn from_log(log: &str) -> Option<Usage> {
    let mut total: Option<Usage> = None;
    // Lines that are not JSON (stderr, plain text) are skipped.
    for event in log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let found = match event["type"].as_str() {
            Some("result") => claude_result(&event),
            Some("turn.completed") => Some(tokens(&event["usage"], &["input_tokens"])),
            _ => None,
        };
        if let Some(found) = found {
            total.get_or_insert_with(Usage::default).add(found);
        }
    }
    total
}

/// The usage in Claude Code's final `result` line; `None` if it has none.
fn claude_result(event: &Value) -> Option<Usage> {
    let usage = event.get("usage")?;
    // Claude Code counts the cached input apart from the rest.
    let input = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ];
    Some(Usage {
        cost_usd: event["total_cost_usd"].as_f64(),
        ..tokens(usage, &input)
    })
}

/// The input tokens (the sum of `input_fields`) and the output tokens in
/// `usage`; a missing number counts as 0.
fn tokens(usage: &Value, input_fields: &[&str]) -> Usage {
    let number = |field: &str| usage[field].as_u64().unwrap_or(0);
    Usage {
        input_tokens: input_fields.iter().map(|field| number(field)).sum(),
        output_tokens: number("output_tokens"),
        cost_usd: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_codes_result_line_gives_tokens_and_cost() {
        let log = "{\"type\":\"system\",\"subtype\":\"init\"}\n\
                   {\"type\":\"assistant\",\"message\":{}}\n\
                   {\"type\":\"result\",\"is_error\":false,\"total_cost_usd\":0.25,\
                   \"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":200,\
                   \"cache_read_input_tokens\":3000,\"output_tokens\":400}}\n";

        let usage = from_log(log);

        assert_eq!(
            usage,
            Some(Usage {
                input_tokens: 3210,
                output_tokens: 400,
                cost_usd: Some(0.25),
            })
        );
    }

    #[test]
    fn codex_turns_are_added_up_without_a_cost() {
        let log = "{\"type\":\"thread.started\"}\n\
                   {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":100,\
                   \"cached_input_tokens\":60,\"output_tokens\":7}}\n\
                   not json\n\
                   {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":50,\
                   \"output_tokens\":3}}\n\
                   --- stderr ---\nwarning\n";

        let usage = from_log(log);

        assert_eq!(
            usage,
            Some(Usage {
                input_tokens: 150,
                output_tokens: 10,
                cost_usd: None,
            })
        );
    }

    #[test]
    fn a_log_without_usage_gives_none() {
        let log = "{\"type\":\"result\",\"is_error\":false}\nplain text\n";

        assert_eq!(from_log(log), None);
        assert_eq!(from_log(""), None);
    }

    #[test]
    fn a_known_cost_survives_adding_an_unknown_one() {
        let mut usage = Usage {
            input_tokens: 1,
            output_tokens: 2,
            cost_usd: Some(0.5),
        };

        usage.add(Usage {
            input_tokens: 10,
            output_tokens: 20,
            cost_usd: None,
        });

        assert_eq!(
            usage,
            Usage {
                input_tokens: 11,
                output_tokens: 22,
                cost_usd: Some(0.5),
            }
        );
    }
}
