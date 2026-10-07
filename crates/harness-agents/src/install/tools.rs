//! The programs the harness needs besides the agents (Git, Node.js, curl,
//! Zed, ...) and a check of which of them are on this computer: the first
//! half of `harness doctor` and of the «Computer» panel on the Agents tab.
//!
//! The agents themselves are checked by [`super::catalog`]. Everything
//! missing here is installed by the harness's installer
//! ([`harness_platform::program::installer_command`]).

use std::path::{Path, PathBuf};

use super::catalog::{older, parse_version, version_of};

/// A program the harness uses that is not an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Every role's work is committed with git.
    Git,
    /// Runs Codex CLI, `DeepSeek` Harness and MCP servers.
    Node,
    /// Starts most MCP servers (`npx -y <package>`).
    Npx,
    /// Asks the MCP registry and the model lists, and talks to web MCP servers.
    Curl,
    /// Linux only: lets Claude Code hide its login from the commands it runs.
    Bubblewrap,
    /// The editor the harness opens files in.
    Zed,
}

/// How much the harness needs a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Without it tasks do not run.
    Required,
    /// The harness works without it, but something is missing or less safe.
    Recommended,
}

/// Node.js this old or newer: `DeepSeek` Harness needs 22.19.
const NODE_MIN: &str = "22.19";

impl Tool {
    /// The tools this system needs, in the order they are shown.
    pub fn here() -> Vec<Tool> {
        let mut tools = vec![Tool::Git, Tool::Node, Tool::Npx, Tool::Curl];
        if harness_platform::program::sandbox_program().is_some() {
            tools.push(Tool::Bubblewrap);
        }
        tools.push(Tool::Zed);
        tools
    }

    /// The name shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Git => "Git",
            Tool::Node => "Node.js",
            Tool::Npx => "npx",
            Tool::Curl => "curl",
            Tool::Bubblewrap => "bubblewrap",
            Tool::Zed => "Zed",
        }
    }

    /// The program looked for in `PATH` (Zed is also looked for in its own folders).
    pub fn program(self) -> &'static str {
        match self {
            Tool::Git => "git",
            Tool::Node => "node",
            Tool::Npx => "npx",
            Tool::Curl => "curl",
            Tool::Bubblewrap => harness_platform::program::sandbox_program().unwrap_or("bwrap"),
            Tool::Zed => "zed",
        }
    }

    /// How much the harness needs it.
    pub fn need(self) -> Need {
        match self {
            Tool::Git | Tool::Node | Tool::Npx => Need::Required,
            Tool::Curl | Tool::Bubblewrap | Tool::Zed => Need::Recommended,
        }
    }

    /// The oldest version the harness was checked with, if it matters.
    pub fn min_version(self) -> Option<&'static str> {
        match self {
            Tool::Node => Some(NODE_MIN),
            Tool::Git | Tool::Npx | Tool::Curl | Tool::Bubblewrap | Tool::Zed => None,
        }
    }
}

/// What was found for one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    /// The tool this status is about.
    pub tool: Tool,
    /// The program's full path; `None`: not found.
    pub path: Option<PathBuf>,
    /// What `--version` printed, reduced to the number.
    pub version: Option<String>,
}

/// How a checked tool looks: fine, worth a look, or missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Found and new enough.
    Good,
    /// Too old, or a recommended tool is missing.
    Warning,
    /// A required tool is missing.
    Missing,
}

impl ToolStatus {
    /// Is the version below the one the harness was checked with?
    pub fn old(&self) -> bool {
        match (self.version.as_deref(), self.tool.min_version()) {
            (Some(version), Some(min)) => older(version, min),
            _ => false,
        }
    }

    /// Fine, worth a look, or missing (see [`Health`]).
    pub fn health(&self) -> Health {
        match (&self.path, self.tool.need()) {
            (None, Need::Required) => Health::Missing,
            (None, Need::Recommended) => Health::Warning,
            (Some(_), _) if self.old() => Health::Warning,
            (Some(_), _) => Health::Good,
        }
    }
}

/// Checks every tool this system needs, all at once.
pub fn check() -> Vec<ToolStatus> {
    check_with(&Tool::here(), &find, &version_of)
}

/// [`check`] for `tools`, with how a program is found and asked for its
/// version given (tests give fake ones).
pub fn check_with(
    tools: &[Tool],
    find: &(dyn Fn(Tool) -> Option<PathBuf> + Sync),
    version: &(dyn Fn(&Path) -> Result<String, String> + Sync),
) -> Vec<ToolStatus> {
    // `--version` can take a moment (Node starts slowly): ask all at once.
    std::thread::scope(|scope| {
        let handles: Vec<_> = tools
            .iter()
            .map(|&tool| {
                scope.spawn(move || {
                    let path = find(tool);
                    let version = path
                        .as_deref()
                        .and_then(|p| version(p).ok())
                        .and_then(|text| parse_version(&text));
                    ToolStatus {
                        tool,
                        path,
                        version,
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(tools)
            .map(|(handle, &tool)| {
                handle.join().unwrap_or(ToolStatus {
                    tool,
                    path: None,
                    version: None,
                })
            })
            .collect()
    })
}

/// Where `tool` is on this computer: Zed also in its own install folders.
fn find(tool: Tool) -> Option<PathBuf> {
    match tool {
        Tool::Zed => harness_platform::editor::zed(),
        Tool::Git | Tool::Node | Tool::Npx | Tool::Curl | Tool::Bubblewrap => {
            harness_platform::program::find(tool.program())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_find(tool: Tool) -> Option<PathBuf> {
        match tool {
            Tool::Git => Some(PathBuf::from("/bin/git")),
            Tool::Node => Some(PathBuf::from("/bin/node")),
            Tool::Curl => Some(PathBuf::from("/bin/curl")),
            Tool::Npx | Tool::Bubblewrap | Tool::Zed => None,
        }
    }

    fn fake_version(path: &Path) -> Result<String, String> {
        Ok(if path.ends_with("git") {
            "git version 2.43.0".to_string()
        } else if path.ends_with("node") {
            "v20.11.1".to_string()
        } else {
            return Err("no version".to_string());
        })
    }

    #[test]
    fn each_tool_is_good_old_or_missing_as_needed() {
        let tools = [Tool::Git, Tool::Node, Tool::Npx, Tool::Curl, Tool::Zed];

        let found = check_with(&tools, &fake_find, &fake_version);

        let health: Vec<(Tool, Health)> = found.iter().map(|s| (s.tool, s.health())).collect();
        assert_eq!(
            health,
            [
                (Tool::Git, Health::Good),
                // Below 22.19, which DeepSeek Harness needs.
                (Tool::Node, Health::Warning),
                (Tool::Npx, Health::Missing),
                // Found, even when it gives no version.
                (Tool::Curl, Health::Good),
                (Tool::Zed, Health::Warning),
            ]
        );
        assert_eq!(found[0].version.as_deref(), Some("2.43.0"));
        assert_eq!(found[3].version, None);
    }

    #[test]
    fn only_linux_lists_bubblewrap() {
        let here = Tool::here();
        assert_eq!(
            here.contains(&Tool::Bubblewrap),
            harness_platform::program::sandbox_program().is_some()
        );
        assert_eq!(here.first(), Some(&Tool::Git));
    }
}
