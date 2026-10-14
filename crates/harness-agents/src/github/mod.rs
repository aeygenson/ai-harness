//! GitHub CLI (`gh`): whether it is signed in to GitHub, the commands that
//! sign it in, and (in `pull`) finding, opening and merging pull requests;
//! `publish` sends a finished task to GitHub, and `merge` merges its pull
//! request and brings the work back into the project. The harness talks to GitHub only through
//! `gh` and `git push`, under Lisa's own login: `gh` keeps the login in the
//! system's key store, and the harness never sees or saves the token.

mod connect;
mod merge;
mod publish;
mod pull;
mod ready;

pub use connect::{connect, connect_with, Connection};
pub use merge::{merge, merge_with, Folder, Merged};
pub use publish::{publish, publish_with, GitHub, PUSH_URL_START};

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The only host the harness works with (GitHub Enterprise is not supported
/// yet). Naming it also keeps `gh auth login` from asking which one.
pub const HOST: &str = "github.com";

/// How long `gh auth status` may take: it asks GitHub whether the login
/// still works, which needs the network.
const STATUS_LIMIT: Duration = Duration::from_secs(15);

/// Is `gh` (at `gh`) signed in to GitHub with a login that still works?
///
/// Runs `gh auth status`, which ends with an error when there is no login,
/// the login stopped working or GitHub could not be reached; then the answer
/// is `false`. Its output is not kept: it names the account and part of the
/// token.
pub fn signed_in(gh: &Path) -> bool {
    let mut command = quiet_command(gh);
    command.args(["auth", "status", "--hostname", HOST]);
    crate::install::models::run_for(command, "", &[], STATUS_LIMIT).is_ok()
}

/// The `gh` program, installed and signed in; otherwise an error that says
/// how to fix that and then `again` (for example "run the task again").
pub(crate) fn ready_gh(again: &str) -> Result<PathBuf, String> {
    let gh = harness_platform::program::find("gh").ok_or_else(|| {
        format!(
            "the GitHub CLI (gh) is not installed: install it on the Agents tab (Computer \
             panel, GitHub CLI), sign in, then {again}"
        )
    })?;
    if !signed_in(&gh) {
        return Err(format!(
            "the GitHub CLI is not signed in, or GitHub cannot be reached: sign in on the \
             Agents tab (Computer panel, GitHub CLI), then {again}"
        ));
    }
    Ok(gh)
}

/// The commands that sign `gh` in, to run one after another in the terminal
/// (they ask questions and open the browser):
///
/// - `gh auth login` in the browser, with HTTPS for git;
/// - `gh auth setup-git`, so a plain `git push` uses the same login.
pub fn sign_in_commands(gh: &Path) -> Vec<Command> {
    let mut login = Command::new(gh);
    login.args([
        "auth",
        "login",
        "--hostname",
        HOST,
        "--web",
        "--git-protocol",
        "https",
    ]);
    let mut setup_git = Command::new(gh);
    setup_git.args(["auth", "setup-git", "--hostname", HOST]);
    vec![login, setup_git]
}

/// `gh` started for a check: Lisa's own environment (`gh` finds its login
/// through it, for example `GH_CONFIG_DIR` or `GH_TOKEN`), but without
/// questions or the note about a newer version.
pub(crate) fn quiet_command(gh: &Path) -> Command {
    let mut command = Command::new(gh);
    command
        .current_dir(std::env::temp_dir())
        // `gh` works with github.com, where git pushes, even when Lisa's
        // environment names another host (GitHub Enterprise).
        .env("GH_HOST", HOST)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_login_is_signed_in() {
        let bin = tempfile::tempdir().unwrap();
        let args = bin.path().join("args.txt");
        let script = format!("save-args {}\nexit 0\n", args.display());
        let gh = harness_fake::install(bin.path(), "gh", &script);

        assert!(signed_in(&gh));

        let saved = std::fs::read_to_string(&args).unwrap();
        assert_eq!(
            saved.lines().collect::<Vec<_>>(),
            ["auth", "status", "--hostname", "github.com"]
        );
    }

    #[test]
    fn no_login_or_no_program_is_not_signed_in() {
        let bin = tempfile::tempdir().unwrap();
        let gh = harness_fake::install(bin.path(), "gh", "eprint You are not logged in\nexit 1\n");

        assert!(!signed_in(&gh));
        assert!(!signed_in(&bin.path().join("missing-gh")));
    }

    #[test]
    fn signing_in_uses_the_browser_and_then_sets_up_git() {
        let commands = sign_in_commands(Path::new("gh"));

        let args: Vec<Vec<String>> = commands
            .iter()
            .map(|c| {
                c.get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect()
            })
            .collect();
        assert_eq!(
            args,
            [
                vec![
                    "auth",
                    "login",
                    "--hostname",
                    "github.com",
                    "--web",
                    "--git-protocol",
                    "https"
                ],
                vec!["auth", "setup-git", "--hostname", "github.com"],
            ]
        );
    }
}
