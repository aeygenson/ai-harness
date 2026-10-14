//! The programs the harness needs besides the agents (Git, Node.js, curl,
//! Zed, GitHub CLI, ...) and a check of which of them are on this computer:
//! the first half of `harness doctor` and of the «Computer» panel on the
//! Agents tab.
//!
//! The agents themselves are checked by [`super::catalog`]. Everything
//! missing here except the GitHub CLI is installed by the harness's
//! installer ([`harness_platform::program::installer_command`]).

use std::path::{Path, PathBuf};

use super::catalog::{older, parse_version, version_of};

mod commands;

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
    /// GitHub CLI (`gh`): pushes a project's task branches and opens pull
    /// requests (see [`crate::github`]).
    GitHubCli,
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
        tools.push(Tool::GitHubCli);
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
            Tool::GitHubCli => "GitHub CLI",
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
            Tool::GitHubCli => "gh",
        }
    }

    /// How much the harness needs it.
    pub fn need(self) -> Need {
        match self {
            Tool::Git | Tool::Node | Tool::Npx => Need::Required,
            // Only projects on GitHub need it.
            Tool::Curl | Tool::Bubblewrap | Tool::Zed | Tool::GitHubCli => Need::Recommended,
        }
    }

    /// The oldest version the harness was checked with, if it matters.
    pub fn min_version(self) -> Option<&'static str> {
        match self {
            Tool::Node => Some(NODE_MIN),
            Tool::Git | Tool::Npx | Tool::Curl | Tool::Bubblewrap | Tool::Zed | Tool::GitHubCli => {
                None
            }
        }
    }

    /// The maker's page that says how to install it.
    pub fn site(self) -> &'static str {
        match self {
            Tool::Git => "https://git-scm.com/downloads",
            // npx comes with Node.js (and npm).
            Tool::Node | Tool::Npx => "https://nodejs.org/en/download",
            Tool::Curl => "https://curl.se/download.html",
            Tool::Bubblewrap => "https://github.com/containers/bubblewrap",
            Tool::Zed => "https://zed.dev/download",
            Tool::GitHubCli => "https://cli.github.com",
        }
    }

    /// Does the harness's installer bring it? Only the GitHub CLI it does
    /// not: most projects do not need it.
    pub fn from_installer(self) -> bool {
        match self {
            Tool::Git | Tool::Node | Tool::Npx | Tool::Curl | Tool::Bubblewrap | Tool::Zed => true,
            Tool::GitHubCli => false,
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
    /// For a program with a login (the GitHub CLI): is it signed in? `None`
    /// for the others, and when the program is not found.
    pub signed_in: Option<bool>,
}

/// How a checked tool looks: fine, worth a look, or missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Found and new enough.
    Good,
    /// Too old, not signed in, or a recommended tool is missing.
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

    /// Is it missing or too old, so that installing it would help?
    pub fn needs_install(&self) -> bool {
        self.path.is_none() || self.old()
    }

    /// Fine, worth a look, or missing (see [`Health`]).
    pub fn health(&self) -> Health {
        match (&self.path, self.tool.need()) {
            (None, Need::Required) => Health::Missing,
            (None, Need::Recommended) => Health::Warning,
            (Some(_), _) if self.old() || self.signed_in == Some(false) => Health::Warning,
            (Some(_), _) => Health::Good,
        }
    }
}

/// Checks every tool this system needs, all at once.
pub fn check() -> Vec<ToolStatus> {
    check_with(&Tool::here(), &find, &version_of, &signed_in)
}

/// [`check`] for `tools`, with how a program is found, asked for its
/// version and asked whether it is signed in given (tests give fake ones).
pub fn check_with(
    tools: &[Tool],
    find: &(dyn Fn(Tool) -> Option<PathBuf> + Sync),
    version: &(dyn Fn(&Path) -> Result<String, String> + Sync),
    signed_in: &(dyn Fn(Tool, &Path) -> Option<bool> + Sync),
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
                    let signed_in = path.as_deref().and_then(|p| signed_in(tool, p));
                    ToolStatus {
                        tool,
                        path,
                        version,
                        signed_in,
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
                    signed_in: None,
                })
            })
            .collect()
    })
}

/// Where `tool` is on this computer: Zed also in its own install folders.
fn find(tool: Tool) -> Option<PathBuf> {
    match tool {
        Tool::Zed => harness_platform::editor::zed(),
        Tool::Git | Tool::Node | Tool::Npx | Tool::Curl | Tool::Bubblewrap | Tool::GitHubCli => {
            harness_platform::program::find(tool.program())
        }
    }
}

/// Whether `tool` at `path` is signed in, for the tools that have a login.
fn signed_in(tool: Tool, path: &Path) -> Option<bool> {
    match tool {
        Tool::GitHubCli => Some(crate::github::signed_in(path)),
        Tool::Git | Tool::Node | Tool::Npx | Tool::Curl | Tool::Bubblewrap | Tool::Zed => None,
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
            Tool::GitHubCli => Some(PathBuf::from("/bin/gh")),
            Tool::Npx | Tool::Bubblewrap | Tool::Zed => None,
        }
    }

    /// `gh` is found but not signed in.
    fn fake_signed_in(tool: Tool, _: &Path) -> Option<bool> {
        (tool == Tool::GitHubCli).then_some(false)
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
        let tools = [
            Tool::Git,
            Tool::Node,
            Tool::Npx,
            Tool::Curl,
            Tool::Zed,
            Tool::GitHubCli,
        ];

        let found = check_with(&tools, &fake_find, &fake_version, &fake_signed_in);

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
                // Found, but not signed in.
                (Tool::GitHubCli, Health::Warning),
            ]
        );
        assert_eq!(found[0].version.as_deref(), Some("2.43.0"));
        assert_eq!(found[3].version, None);
        assert_eq!(found[0].signed_in, None);
        assert_eq!(found[5].signed_in, Some(false));
        // Signing in fixes it, installing does not.
        assert!(!found[5].needs_install());
        assert!(found[4].needs_install());
    }

    #[test]
    fn a_signed_in_github_cli_is_good() {
        let found = check_with(&[Tool::GitHubCli], &fake_find, &fake_version, &|_, _| {
            Some(true)
        });

        assert_eq!(found[0].health(), Health::Good);
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
