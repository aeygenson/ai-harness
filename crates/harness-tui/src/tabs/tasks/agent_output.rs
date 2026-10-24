//! Turns what an agent prints into short lines for the live log.

use harness_core::text;

/// A line of the agent's output as the live log shows it. The agents print
/// JSON events; the texts in them are enough to follow the work. Empty events
/// (`None`) are left out.
pub fn readable(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Some(shorten(line));
    };
    let kind = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let mut texts = Vec::new();
    collect_texts(&value, &mut texts);
    if texts.is_empty() {
        return None;
    }
    Some(shorten(&format!("{kind:<14} {}", texts.join(" · "))))
}

/// Keys whose string values say what the agent does.
const TEXT_KEYS: [&str; 7] = [
    "text",
    "command",
    "name",
    "tool_name",
    "result",
    "content",
    "message",
];

fn collect_texts(value: &serde_json::Value, out: &mut Vec<String>) {
    if out.len() >= 3 {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            // The object's own texts first (a tool's name), then what is
            // inside it (the tool's command).
            for (key, value) in map {
                if let serde_json::Value::String(text) = value {
                    if TEXT_KEYS.contains(&key.as_str()) && !text.trim().is_empty() {
                        out.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
                    }
                }
            }
            for value in map.values() {
                if value.is_object() || value.is_array() {
                    collect_texts(value, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_texts(item, out);
            }
        }
        _ => {}
    }
}

/// At most 300 characters (the panel cuts at its width anyway), without
/// terminal control characters, which agents may print.
fn shorten(text: &str) -> String {
    let text = text::safe(text);
    let mut short: String = text.chars().take(300).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_events_become_short_readable_lines() {
        let claude = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        assert_eq!(
            readable(claude).unwrap(),
            "assistant      Bash · cargo test"
        );
        let codex = r#"{"type":"item.completed","item":{"type":"agent_message","text":"All\n tests pass"}}"#;
        assert_eq!(readable(codex).unwrap(), "item.completed All tests pass");
        // Events without text are left out; plain text stays.
        assert_eq!(readable(r#"{"type":"turn.started"}"#), None);
        assert_eq!(readable("  "), None);
        assert_eq!(readable("warning: slow").unwrap(), "warning: slow");
        // Colours and screen commands from the agent do not reach the panel.
        assert_eq!(readable("\u{1b}[31mred\u{1b}[0m").unwrap(), "red");
        let event = r#"{"type":"assistant","text":"done\u001b[2J"}"#;
        assert_eq!(readable(event).unwrap(), "assistant      done");
        let long = "x".repeat(400);
        assert_eq!(readable(&long).unwrap().chars().count(), 301);
    }
}
