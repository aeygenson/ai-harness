//! The catalog of console agents: the ones the harness runs, and others
//! worth knowing about, and a check of which of them are on this computer.
//!
//! An agent is installed when its program is found the way a role would
//! start it ([`harness_platform::program::find`]). Its version is what
//! `<program> --version` prints; it is too old when it is below the version
//! the harness was checked with. The install and update commands are the
//! makers' own; [`run_command`] runs one after Lisa has seen and confirmed it.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
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

    /// The command for this computer's system. On Linux `npm install -g`
    /// installs into `~/.local`, which needs no `sudo`.
    pub fn here(&self) -> Option<String> {
        let command = harness_platform::program::on_this_system(self.unix, self.windows)?;
        Some(for_npm(
            command,
            harness_platform::program::npm_user_prefix(),
        ))
    }
}

/// `npm install -g …` with `--prefix "<prefix>"` when there is one.
fn for_npm(command: &str, prefix: Option<&str>) -> String {
    match (command.strip_prefix("npm install -g "), prefix) {
        (Some(rest), Some(prefix)) => format!("npm install -g --prefix \"{prefix}\" {rest}"),
        _ => command.to_string(),
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
    /// The maker's own way to take it off, when deleting the program file
    /// is not enough (Claude Code keeps its versions next to it).
    pub remove: Commands,
    pub site: &'static str,
}

impl Entry {
    /// The harness can run it: it is one of the agents of `harness.toml`.
    pub fn runs(&self) -> bool {
        AGENTS.contains(&self.id)
    }

    /// The npm package it is installed from (`npm install -g <package>@latest`).
    pub fn npm_package(&self) -> Option<&'static str> {
        let rest = self.install.unix?.strip_prefix("npm install -g ")?;
        Some(rest.strip_suffix("@latest").unwrap_or(rest))
    }
}

const NPM_CODEX: &str = "npm install -g @openai/codex@latest";
const NPM_DSH: &str = "npm install -g @deepseek-ai/dsh@latest";
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
        // Its native installer's files: the command and all its versions.
        remove: Commands {
            unix: Some("rm -f \"$HOME/.local/bin/claude\" && rm -rf \"$HOME/.local/share/claude\""),
            windows: Some("Remove-Item -Force \"$env:USERPROFILE\\.local\\bin\\claude.exe\"; Remove-Item -Recurse -Force \"$env:USERPROFILE\\.local\\share\\claude\""),
        },
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
        site: "https://antigravity.google/docs/cli/install",
    },
    Entry {
        id: "dsh",
        name: "DeepSeek Harness",
        vendor: "DeepSeek",
        programs: &["dsh"],
        inside: None,
        plan: "DeepSeek API key, pay per use",
        // Checked with 0.2.0-rc.2; it needs Node.js 22.19 or newer.
        min_version: None,
        install: Commands::both(NPM_DSH),
        update: Commands::both(NPM_DSH),
        remove: Commands::NONE,
        site: "https://github.com/deepseek-ai/deepseek-harness",
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
        // Its install command is on the site; not checked yet.
        install: Commands::NONE,
        update: Commands::NONE,
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
        remove: Commands::NONE,
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
    /// Its program is here. An agent that is a model inside another agent's
    /// program counts only when the harness runs it: Claude Code alone is
    /// not GLM, which also needs its plan's key and an adapter.
    pub fn installed(&self) -> bool {
        self.path.is_some() && (self.entry.inside.is_none() || self.entry.runs())
    }

    /// The program this agent lives inside is here, but the agent itself is
    /// not usable yet (see [`Status::installed`]).
    pub fn host_only(&self) -> bool {
        self.path.is_some() && !self.installed()
    }

    /// What can be done now: update an installed agent, install a missing
    /// one, nothing for a [`host_only`](Status::host_only) one. With the command for
    /// this computer, if the maker gives one.
    pub fn action(&self) -> Option<(Action, Option<String>)> {
        if self.host_only() {
            None
        } else if self.installed() {
            let command = match (self.entry.npm_package(), self.npm_place()) {
                (Some(_), NpmPlace::System) => None,
                (Some(package), NpmPlace::User(prefix)) => {
                    Some(npm_command("install", prefix, &format!("{package}@latest")))
                }
                (None, _) => self.entry.update.here(),
            };
            Some((Action::Update, command))
        } else {
            Some((Action::Install, self.entry.install.here()))
        }
    }

    /// For an agent npm installed into the system's own folders (on Linux,
    /// with `sudo`): the command to run in a terminal for `action`. The
    /// harness cannot run it, it would ask for a password; installing a
    /// second copy into `~/.local` instead would leave the old one in use
    /// by everything else.
    pub fn needs_sudo(&self, action: Action) -> Option<String> {
        let package = self.entry.npm_package()?;
        if !self.installed() || self.npm_place() != NpmPlace::System {
            return None;
        }
        match action {
            Action::Install => None,
            Action::Update => Some(format!("sudo npm install -g {package}@latest")),
            Action::Remove => Some(format!("sudo npm uninstall -g {package}")),
        }
    }

    /// Where npm keeps this installed agent, judged by its program's path.
    fn npm_place(&self) -> NpmPlace {
        let Some(prefix) = harness_platform::program::npm_user_prefix() else {
            // macOS (Homebrew) and Windows: npm's own folder is the user's.
            return NpmPlace::User(None);
        };
        let home = harness_platform::home::home_dir();
        let path = self.path.as_deref().unwrap_or(Path::new(""));
        match home {
            Some(home) if path.starts_with(home.join(".local")) => NpmPlace::User(Some(prefix)),
            // nvm and other npm folders of the user's own.
            Some(home) if path.starts_with(&home) => NpmPlace::User(None),
            _ => NpmPlace::System,
        }
    }

    /// How to take the agent's program off this computer: `npm uninstall`
    /// for what npm installed, otherwise deleting the program file that was
    /// found (Claude Code, Antigravity and Cursor install one command there).
    /// Its settings, saved logins and data stay. `None` when it is not
    /// installed, or lives inside another agent's program: removing Codex
    /// for «Codex + DeepSeek» would surprise.
    pub fn removal(&self) -> Option<String> {
        if !self.installed() || self.entry.inside.is_some() {
            return None;
        }
        let path = self.path.as_ref()?;
        if let Some(package) = self.entry.npm_package() {
            return match self.npm_place() {
                NpmPlace::User(prefix) => Some(npm_command("uninstall", prefix, package)),
                NpmPlace::System => None,
            };
        }
        // The maker's own way, for an install in the usual place.
        let local = harness_platform::home::home_dir()
            .is_some_and(|home| path.starts_with(home.join(".local")));
        if let (true, Some(command)) = (local, self.entry.remove.here()) {
            return Some(command);
        }
        let path = path.to_string_lossy();
        Some(harness_platform::program::on_this_system(
            format!("rm -f '{}'", path.replace('\'', "'\\''")),
            format!("Remove-Item -LiteralPath '{}'", path.replace('\'', "''")),
        ))
    }

    /// Installed, but older than the harness was checked with.
    pub fn old(&self) -> bool {
        match (self.version.as_deref(), self.entry.min_version) {
            (Some(version), Some(min)) => older(version, min),
            _ => false,
        }
    }
}

/// Where npm keeps an installed agent: the user's own folders (with the
/// `--prefix` it was installed with, if any) or the system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NpmPlace {
    User(Option<&'static str>),
    System,
}

/// `npm <verb> -g [--prefix "<prefix>"] <what>`.
fn npm_command(verb: &str, prefix: Option<&str>, what: &str) -> String {
    match prefix {
        Some(prefix) => format!("npm {verb} -g --prefix \"{prefix}\" {what}"),
        None => format!("npm {verb} -g {what}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Update,
    Remove,
}

/// Runs a maker's install or update `command` in the system's shell, with
/// the user's own environment (installers need the network settings and
/// `PATH`), nothing on standard input. Each line it prints goes to
/// `on_line`, cleaned of colours and progress bars. `Err` says how it ended.
pub fn run_command(command: &str, mut on_line: impl FnMut(String)) -> Result<(), String> {
    let (shell, args) = harness_platform::program::shell();
    let mut child = Command::new(harness_platform::program::resolve(shell));
    child
        .args(args)
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Started from PowerShell 7, Windows PowerShell 5.1 inherits a module
    // path it cannot load its own modules from, and installers fail with
    // «'Get-FileHash' is not recognized». Without it, it uses its default.
    child.env_remove("PSModulePath");
    // Programs installed into ~/.local/bin are found by the next steps.
    if let Some(path) = with_local_bin() {
        child.env("PATH", path);
    }
    let mut child =
        crate::process::spawn(&mut child).map_err(|e| format!("cannot start {shell}: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let pipes: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    let readers: Vec<_> = pipes
        .into_iter()
        .map(|pipe| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                    let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
                }
            })
        })
        .collect();
    drop(tx);
    for line in rx {
        let line = clean_line(&line);
        if !line.is_empty() {
            on_line(line);
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the command ended with {status}"))
    }
}

/// `PATH` with `~/.local/bin` in front, if it is not there already.
fn with_local_bin() -> Option<std::ffi::OsString> {
    let local = harness_platform::home::home_dir()?
        .join(".local")
        .join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&path).any(|dir| dir == local) {
        return None;
    }
    std::env::join_paths(std::iter::once(local).chain(std::env::split_paths(&path))).ok()
}

/// A line as a terminal would end up showing it: after the last carriage
/// return (progress bars redraw with `\r`), without colour codes.
pub fn clean_line(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    let line = line.rsplit('\r').next().unwrap_or(line);
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ … letter, or ESC and one more character.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out.trim_end().to_string()
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

    #[test]
    fn npm_installs_go_to_the_users_folder_where_npm_needs_it() {
        assert_eq!(
            for_npm("npm install -g opencode-ai@latest", Some("$HOME/.local")),
            "npm install -g --prefix \"$HOME/.local\" opencode-ai@latest"
        );
        assert_eq!(
            for_npm("npm install -g opencode-ai@latest", None),
            "npm install -g opencode-ai@latest"
        );
        assert_eq!(
            for_npm("claude update", Some("$HOME/.local")),
            "claude update"
        );
    }

    #[test]
    fn claude_alone_is_not_glm() {
        let glm = CATALOG.iter().find(|e| e.id == "claude+glm").unwrap();
        let found = Status {
            entry: *glm,
            path: Some(PathBuf::from("/bin/claude")),
            version: Some("2.1.0".into()),
            problem: None,
            login: None,
        };
        assert!(!found.installed() && found.host_only());
        assert_eq!(found.action(), None);
        let missing = Status {
            path: None,
            ..found.clone()
        };
        assert!(!missing.host_only());
        assert_eq!(missing.action().unwrap().0, Action::Install);
        // DeepSeek is run by the harness, so Codex is enough for it.
        let deepseek = CATALOG.iter().find(|e| e.id == "codex+deepseek").unwrap();
        let found = Status {
            entry: *deepseek,
            ..found
        };
        assert!(found.installed());
        assert_eq!(found.action().unwrap().0, Action::Update);
    }

    #[test]
    fn removing_takes_away_only_the_program() {
        let entry = |id: &str| *CATALOG.iter().find(|e| e.id == id).unwrap();
        let status = |id: &str, path: &str| Status {
            entry: entry(id),
            path: Some(PathBuf::from(path)),
            version: None,
            problem: None,
            login: Some(true),
        };
        assert_eq!(entry("opencode").npm_package(), Some("opencode-ai"));
        assert_eq!(entry("claude").npm_package(), None);
        // A program file is deleted, quoted for the shell.
        let claude = status("claude", "/opt/x/it's/claude").removal().unwrap();
        if cfg!(windows) {
            assert_eq!(claude, "Remove-Item -LiteralPath '/opt/x/it''s/claude'");
        } else {
            assert_eq!(claude, "rm -f '/opt/x/it'\\''s/claude'");
        }
        // Claude Code's own installer: its versions go too.
        let home = harness_platform::home::home_dir().unwrap();
        let native = home.join(".local/bin/claude");
        let native = status("claude", native.to_str().unwrap())
            .removal()
            .unwrap();
        assert!(
            native.contains(".local") && native.contains("share"),
            "{native}"
        );

        // What npm installed, npm takes away, from where it lives.
        let local = home.join(".local/bin/opencode");
        let local = status("opencode", local.to_str().unwrap());
        let system = status("opencode", "/usr/bin/opencode");
        if harness_platform::program::npm_user_prefix().is_some() {
            assert_eq!(
                local.removal().unwrap(),
                "npm uninstall -g --prefix \"$HOME/.local\" opencode-ai"
            );
            assert_eq!(
                local.action().unwrap().1.unwrap(),
                "npm install -g --prefix \"$HOME/.local\" opencode-ai@latest"
            );
            // In the system's folders only sudo can: the harness says how
            // instead of putting a second copy into ~/.local.
            assert_eq!(system.removal(), None);
            assert_eq!(system.action().unwrap(), (Action::Update, None));
            assert_eq!(
                system.needs_sudo(Action::Update).unwrap(),
                "sudo npm install -g opencode-ai@latest"
            );
            assert_eq!(
                system.needs_sudo(Action::Remove).unwrap(),
                "sudo npm uninstall -g opencode-ai"
            );
            assert_eq!(local.needs_sudo(Action::Update), None);
        } else {
            assert_eq!(system.removal().unwrap(), "npm uninstall -g opencode-ai");
            assert_eq!(system.needs_sudo(Action::Update), None);
        }
        // Not for an agent inside another one, nor for one not installed.
        assert_eq!(status("codex+deepseek", "/usr/bin/codex").removal(), None);
        let missing = Status {
            path: None,
            ..status("claude", "/x")
        };
        assert_eq!(missing.removal(), None);
    }

    #[test]
    fn output_lines_lose_colours_and_progress_bars() {
        assert_eq!(clean_line("\u{1b}[32m✓ done\u{1b}[0m\r\n"), "✓ done");
        assert_eq!(clean_line("10%\r50%\r100% installed"), "100% installed");
        assert_eq!(clean_line("   \r"), "");
    }

    #[cfg(unix)]
    #[test]
    fn a_command_runs_in_the_shell_and_its_lines_come_back() {
        let mut lines = Vec::new();
        run_command("echo one; echo two >&2", |l| lines.push(l)).unwrap();
        lines.sort();
        assert_eq!(lines, ["one", "two"]);
        let error = run_command("echo bad; exit 3", |_| {}).unwrap_err();
        assert!(error.contains('3'), "{error}");
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
