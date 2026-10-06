//! Reading what `agy` printed: the last result event and its errors.

/// The last `{"event":"result","result":{"status":...,"denied_actions":[...]}}`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RunResult {
    pub(super) status: String,
    /// Actions agy refused because headless mode cannot ask, e.g. "RunCommand".
    pub(super) denied: Vec<String>,
}

impl RunResult {
    pub(super) fn find(stdout: &str) -> Option<Self> {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["event"] == "result")
            .filter_map(|event| {
                let result = &event["result"];
                let status = result["status"].as_str()?.to_string();
                let denied = result["denied_actions"]
                    .as_array()
                    .map(|actions| {
                        actions
                            .iter()
                            .map(|a| {
                                a["display_name"]
                                    .as_str()
                                    .or(a["action"].as_str())
                                    .unwrap_or("an action")
                                    .to_string()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(RunResult { status, denied })
            })
            .next_back()
    }
}

/// A failed call to the model is reported on stderr as `AGY_ERROR: {...}`.
pub(super) fn agy_error(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("AGY_ERROR:"))
        .map(|rest| rest.trim().to_string())
}

pub(super) fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
}
