//! The catalog of console agents: the ones the harness runs, and others
//! worth knowing about, and a check of which of them are on this computer.
//!
//! An agent is installed when its program is found the way a role would
//! start it ([`harness_platform::program::find`]). Its version is what
//! `<program> --version` prints; it is too old when it is below the version
//! the harness was checked with. The install and update commands are the
//! makers' own; [`run_command`] runs one after Lisa has seen and confirmed it.

mod entries;
mod install;
mod status;

use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_core::config::AgentKind;

use crate::install::credentials;
pub use entries::{Commands, Entry, CATALOG};
pub use install::{clean_line, run_command};
pub use status::{Action, Status};

/// How long `--version` may take.
const VERSION_LIMIT: Duration = Duration::from_secs(10);

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
                    let login = entry.id.parse::<AgentKind>().ok().map(|agent| {
                        credentials_dir.is_some_and(|dir| credentials::has_login(dir, agent))
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
pub(crate) fn version_of(program: &Path) -> Result<String, String> {
    let dir = std::env::temp_dir();
    let mut command = crate::process::base_command(program, &dir);
    command.arg("--version");
    crate::install::models::run_for(command, "", &[], VERSION_LIMIT)
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
        // A preview keeps its mark: `0.2.0-rc.2`.
        let pre: String = word[number.len()..]
            .strip_prefix('-')
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
                    .collect::<String>()
            })
            .map(|rest| rest.trim_end_matches('.').to_string())
            .filter(|rest| rest.starts_with(|c: char| c.is_ascii_alphabetic()))
            .map(|rest| format!("-{rest}"))
            .unwrap_or_default();
        (number.contains('.') && number.starts_with(|c: char| c.is_ascii_digit()))
            .then(|| format!("{number}{pre}"))
    })
}

/// Is `version` below `min`? Compared number by number: 0.99 < 0.158. A
/// preview mark (`-rc.2`) is left out.
pub fn older(version: &str, min: &str) -> bool {
    let numbers = |text: &str| -> Vec<u64> {
        let text = text.split('-').next().unwrap_or(text);
        text.split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    numbers(version) < numbers(min)
}

#[cfg(test)]
mod tests {
    use super::entries::for_npm;
    use super::*;
    use std::collections::HashSet;
    use std::fs;

    #[test]
    fn every_agent_the_harness_runs_is_in_the_catalog_once() {
        for agent in AgentKind::ALL {
            let count = CATALOG.iter().filter(|e| e.id == agent.as_str()).count();
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
        assert_eq!(parse_version("0.2.0-rc.2\n").unwrap(), "0.2.0-rc.2");
        assert_eq!(parse_version("tool 1.2.3-1").unwrap(), "1.2.3");
        assert!(!older("0.2.0-rc.2", "0.2.0"));
        assert!(older("0.1.9-beta", "0.2.0"));
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
        assert_eq!(status("claude+glm", "/usr/bin/claude").removal(), None);
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

    #[test]
    fn a_command_runs_in_the_shell_and_its_lines_come_back() {
        // `sh` and PowerShell write to standard error differently.
        let both = if cfg!(windows) {
            "echo one; [Console]::Error.WriteLine('two')"
        } else {
            "echo one; echo two >&2"
        };
        let mut lines = Vec::new();
        run_command(both, |l| lines.push(l)).unwrap();
        lines.sort();
        assert_eq!(lines, ["one", "two"]);
        let error = run_command("echo bad; exit 3", |_| {}).unwrap_err();
        assert!(error.contains('3'), "{error}");
    }

    #[test]
    fn the_version_comes_from_the_program_itself() {
        let bin = tempfile::tempdir().unwrap();
        let path = harness_fake::install(
            bin.path(),
            "fake",
            "when-arg 1 --version print fake 4.5.6\n",
        );
        assert_eq!(parse_version(&version_of(&path).unwrap()).unwrap(), "4.5.6");
    }
}
