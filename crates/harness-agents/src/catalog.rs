//! The catalog of console agents: the ones the harness runs, and others
//! worth knowing about, and a check of which of them are on this computer.
//!
//! An agent is installed when its program is found the way a role would
//! start it ([`harness_platform::program::find`]). Its version is what
//! `<program> --version` prints; it is too old when it is below the version
//! the harness was checked with. Nothing here installs anything: the
//! install and update commands are the makers' own, shown to Lisa.

use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_core::config::AGENTS;

use crate::credentials;

/// How long `--version` may take.
const VERSION_LIMIT: Duration = Duration::from_secs(10);

/// A command for Linux and macOS, and one for Windows. `None`: the maker's
/// site says how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commands {
    pub unix: Option<&'static str>,
    pub windows: Option<&'static str>,
}

impl Commands {
    const NONE: Self = Self {
        unix: None,
        windows: None,
    };

    /// The same command everywhere, such as `npm install -g …`.
    const fn both(command: &'static str) -> Self {
        Self {
            unix: Some(command),
            windows: Some(command),
        }
    }

    /// The command for this computer's system.
    pub fn here(&self) -> Option<&'static str> {
        harness_platform::program::on_this_system(self.unix, self.windows)
    }
}

/// One agent of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The name in `harness.toml` (`agent = "codex"`), or the catalog's own.
    pub id: &'static str,
    pub name: &'static str,
    pub vendor: &'static str,
    /// The program, by name; the first one found counts.
    pub programs: &'static [&'static str],
    /// The agent is a model inside this other agent's program, as DeepSeek
    /// runs inside Codex: installing that program is all it needs.
    pub inside: Option<&'static str>,
    /// The subscription or payment it works with.
    pub plan: &'static str,
    /// The lowest version the harness was checked with.
    pub min_version: Option<&'static str>,
    pub install: Commands,
    pub update: Commands,
    pub site: &'static str,
}

impl Entry {
    /// The harness can run it: it is one of the agents of `harness.toml`.
    pub fn runs(&self) -> bool {
        AGENTS.contains(&self.id)
    }
}

const NPM_CODEX: &str = "npm install -g @openai/codex@latest";
const CLAUDE_INSTALL: Commands = Commands {
    unix: Some("curl -fsSL https://claude.ai/install.sh | bash"),
    windows: Some("irm https://claude.ai/install.ps1 | iex"),
};
const AGY_INSTALL: Commands = Commands {
    unix: Some("curl -fsSL https://antigravity.google/cli/install.sh | bash"),
    windows: Some("irm https://antigravity.google/cli/install.ps1 | iex"),
};
const CURSOR_INSTALL: Commands = Commands {
    unix: Some("curl https://cursor.com/install -fsS | bash"),
    windows: Some("irm 'https://cursor.com/install?win32=true' | iex"),
};
const GROK_INSTALL: Commands = Commands {
    unix: Some("curl -fsSL https://x.ai/cli/install.sh | bash"),
    windows: Some("irm https://x.ai/cli/install.ps1 | iex"),
};

/// Every agent the harness knows: first the ones it runs, then the others.
pub const CATALOG: &[Entry] = &[
    Entry {
        id: "claude",
        name: "Claude Code",
        vendor: "Anthropic",
        programs: &["claude"],
        inside: None,
        plan: "Claude Pro $20 / Max $100–200",
        min_version: None,
        install: CLAUDE_INSTALL,
        update: Commands::both("claude update"),
        site: "https://claude.com/product/claude-code",
    },
    Entry {
        id: "codex",
        name: "Codex CLI",
        vendor: "OpenAI",
        programs: &["codex"],
        inside: None,
        plan: "ChatGPT Plus $20 / Pro",
        // Plugins were checked with 0.158.
        min_version: Some("0.158.0"),
        install: Commands::both(NPM_CODEX),
        update: Commands::both(NPM_CODEX),
        site: "https://developers.openai.com/codex/cli",
    },
    Entry {
        id: "codex+deepseek",
        name: "Codex + DeepSeek",
        vendor: "DeepSeek",
        programs: &["codex"],
        inside: Some("Codex CLI"),
        plan: "DeepSeek API key, pay per use",
        min_version: Some("0.158.0"),
        install: Commands::both(NPM_CODEX),
        update: Commands::both(NPM_CODEX),
        site: "https://platform.deepseek.com",
    },
    Entry {
        id: "antigravity",
        name: "Antigravity CLI",
        vendor: "Google",
        programs: &["agy"],
        inside: None,
        plan: "Google AI Pro $20 / Ultra",
        min_version: None,
        install: AGY_INSTALL,
        // The installer always brings the newest version.
        update: AGY_INSTALL,
        site: "https://antigravity.google/docs/cli/install",
    },
    Entry {
        id: "claude+glm",
        name: "Claude Code + GLM",
        vendor: "Z.ai",
        programs: &["claude"],
        inside: Some("Claude Code"),
        plan: "GLM Coding Plan $18–168",
        min_version: None,
        install: CLAUDE_INSTALL,
        update: Commands::both("claude update"),
        site: "https://docs.z.ai/devpack",
    },
    Entry {
        id: "copilot",
        name: "GitHub Copilot CLI",
        vendor: "GitHub",
        programs: &["copilot"],
        inside: None,
        plan: "Copilot Pro $10 / Pro+ $39",
        min_version: None,
        install: Commands::both("npm install -g @github/copilot@latest"),
        update: Commands::both("npm install -g @github/copilot@latest"),
        site: "https://docs.github.com/en/copilot/concepts/agents/about-copilot-cli",
    },
    Entry {
        id: "cursor",
        name: "Cursor CLI",
        vendor: "Cursor",
        programs: &["cursor-agent", "agent"],
        inside: None,
        plan: "Cursor Pro $20",
        min_version: None,
        install: CURSOR_INSTALL,
        update: CURSOR_INSTALL,
        site: "https://cursor.com/docs/cli/overview",
    },
    Entry {
        id: "kimi",
        name: "Kimi Code CLI",
        vendor: "Moonshot AI",
        programs: &["kimi"],
        inside: None,
        plan: "Kimi membership $19–199",
        min_version: None,
        install: Commands::both("npm install -g @kimi-code/cli@latest"),
        update: Commands::both("npm install -g @kimi-code/cli@latest"),
        site: "https://www.kimi.com/code",
    },
    Entry {
        id: "grok",
        name: "Grok Build",
        vendor: "xAI",
        programs: &["grok"],
        inside: None,
        plan: "SuperGrok $30; without a window only with an xAI API key",
        min_version: None,
        install: GROK_INSTALL,
        update: GROK_INSTALL,
        site: "https://x.ai",
    },
    Entry {
        id: "kiro",
        name: "Kiro CLI",
        vendor: "Amazon",
        programs: &["kiro-cli"],
        inside: None,
        plan: "Kiro Pro $20 / Pro Max $100",
        min_version: None,
        install: Commands::NONE,
        update: Commands::NONE,
        site: "https://kiro.dev/docs/cli/",
    },
    Entry {
        id: "vibe",
        name: "Mistral Vibe CLI",
        vendor: "Mistral AI",
        programs: &["vibe"],
        inside: None,
        plan: "Vibe Pro $14.99",
        min_version: None,
        install: Commands::NONE,
        update: Commands::NONE,
        site: "https://mistral.ai/products/vibe",
    },
    Entry {
        id: "opencode",
        name: "OpenCode",
        vendor: "OpenCode (open source)",
        programs: &["opencode"],
        inside: None,
        plan: "OpenCode Go $10, or your own keys",
        min_version: None,
        install: Commands::both("npm install -g opencode-ai@latest"),
        update: Commands::both("npm install -g opencode-ai@latest"),
        site: "https://opencode.ai",
    },
];

/// What was found for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub entry: Entry,
    /// The program's full path; `None`: not installed.
    pub path: Option<PathBuf>,
    /// What `--version` printed, reduced to the number.
    pub version: Option<String>,
    /// Why there is no version, when the program is there.
    pub problem: Option<String>,
    /// Is a login saved? `None` for agents the harness does not run yet.
    pub login: Option<bool>,
}

impl Status {
    pub fn installed(&self) -> bool {
        self.path.is_some()
    }

    /// Installed, but older than the harness was checked with.
    pub fn old(&self) -> bool {
        match (self.version.as_deref(), self.entry.min_version) {
            (Some(version), Some(min)) => older(version, min),
            _ => false,
        }
    }
}

/// Checks every agent of the catalog on this computer, all at once.
pub fn check_all(credentials_dir: Option<&Path>) -> Vec<Status> {
    check_with(
        CATALOG,
        &harness_platform::program::find,
        &version_of,
        credentials_dir,
    )
}

/// [`check_all`] for `entries`, with how a program is found and asked for
/// its version given (tests give fake ones).
pub fn check_with(
    entries: &[Entry],
    find: &(dyn Fn(&str) -> Option<PathBuf> + Sync),
    version: &(dyn Fn(&Path) -> Result<String, String> + Sync),
    credentials_dir: Option<&Path>,
) -> Vec<Status> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = entries
            .iter()
            .map(|entry| {
                scope.spawn(move || {
                    let path = entry.programs.iter().find_map(|name| find(name));
                    let (version, problem) = match path.as_deref().map(version) {
                        Some(Ok(text)) => (parse_version(&text), None),
                        Some(Err(error)) => (None, Some(error)),
                        None => (None, None),
                    };
                    let login = entry.runs().then(|| {
                        credentials_dir.is_some_and(|dir| credentials::has_login(dir, entry.id))
                    });
                    Status {
                        entry: *entry,
                        path,
                        version,
                        problem,
                        login,
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(entries)
            .map(|(handle, entry)| {
                handle.join().unwrap_or_else(|_| Status {
                    entry: *entry,
                    path: None,
                    version: None,
                    problem: Some("the check stopped".into()),
                    login: None,
                })
            })
            .collect()
    })
}

/// What `<program> --version` prints.
fn version_of(program: &Path) -> Result<String, String> {
    let dir = std::env::temp_dir();
    let mut command = crate::models::command(program, &dir);
    command.arg("--version");
    crate::models::run_for(command, "", &[], VERSION_LIMIT)
}

/// The first word that looks like a version: `codex-cli 0.159.2` → `0.159.2`,
/// `2.1.283 (Claude Code)` → `2.1.283`, `v1.2` → `1.2`.
pub fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        let word = word.strip_prefix('v').unwrap_or(word);
        let number: String = word
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let number = number.trim_end_matches('.');
        (number.contains('.') && number.starts_with(|c: char| c.is_ascii_digit()))
            .then(|| number.to_string())
    })
}

/// Is `version` below `min`? Compared number by number: 0.99 < 0.158.
pub fn older(version: &str, min: &str) -> bool {
    let numbers = |text: &str| -> Vec<u64> {
        text.split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    numbers(version) < numbers(min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;

    #[test]
    fn every_agent_the_harness_runs_is_in_the_catalog_once() {
        for agent in AGENTS {
            let count = CATALOG.iter().filter(|e| e.id == agent).count();
            assert_eq!(count, 1, "{agent}");
        }
        let ids: HashSet<_> = CATALOG.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), CATALOG.len());
        // The ones the harness runs come first.
        let first_other = CATALOG.iter().position(|e| !e.runs()).unwrap();
        assert!(CATALOG[first_other..].iter().all(|e| !e.runs()));
        assert!(CATALOG.iter().all(|e| !e.programs.is_empty()));
    }

    #[test]
    fn a_version_is_read_from_what_the_program_prints() {
        assert_eq!(parse_version("codex-cli 0.159.2\n").unwrap(), "0.159.2");
        assert_eq!(parse_version("2.1.283 (Claude Code)").unwrap(), "2.1.283");
        assert_eq!(parse_version("agy v1.2.12.").unwrap(), "1.2.12");
        assert_eq!(parse_version("no version here 3"), None);
        assert!(older("0.99.0", "0.158.0"));
        assert!(older("0.157.1", "0.158"));
        assert!(!older("0.158.0", "0.158.0"));
        assert!(!older("1.0", "0.158.0"));
    }

    #[test]
    fn the_check_finds_programs_versions_and_logins() {
        let creds = tempfile::tempdir().unwrap();
        fs::create_dir_all(creds.path().join("codex")).unwrap();
        fs::write(creds.path().join("codex/auth.json"), "{}").unwrap();
        let installed = ["codex", "agent", "agy"];
        let find = |name: &str| {
            installed
                .contains(&name)
                .then(|| PathBuf::from(format!("/bin/{name}")))
        };
        let version = |path: &Path| match path.to_str() {
            Some("/bin/codex") => Ok("codex-cli 0.157.1".to_string()),
            Some("/bin/agy") => Err("agy did not answer in 10 s".to_string()),
            _ => Ok("1.0.0".to_string()),
        };
        let all = check_with(CATALOG, &find, &version, Some(creds.path()));
        let get = |id: &str| all.iter().find(|s| s.entry.id == id).unwrap();

        let claude = get("claude");
        assert!(!claude.installed());
        assert_eq!(claude.login, Some(false));

        let codex = get("codex");
        assert_eq!(codex.path, Some(PathBuf::from("/bin/codex")));
        assert_eq!(codex.version.as_deref(), Some("0.157.1"));
        assert!(codex.old());
        assert_eq!(codex.login, Some(true));
        // DeepSeek runs inside the same Codex.
        assert!(get("codex+deepseek").installed());

        let agy = get("antigravity");
        assert!(agy.installed() && agy.version.is_none() && !agy.old());
        assert!(agy.problem.as_deref().unwrap().contains("did not answer"));

        // Cursor's second program name counts; the catalog has no login for it.
        let cursor = get("cursor");
        assert_eq!(cursor.path, Some(PathBuf::from("/bin/agent")));
        assert_eq!(cursor.login, None);
        assert_eq!(all.len(), CATALOG.len());
    }

    #[cfg(unix)]
    #[test]
    fn the_version_comes_from_the_program_itself() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let path = bin.path().join("fake");
        fs::write(
            &path,
            "#!/bin/sh\ntest \"$1\" = --version && echo 'fake 4.5.6'\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(parse_version(&version_of(&path).unwrap()).unwrap(), "4.5.6");
    }
}
