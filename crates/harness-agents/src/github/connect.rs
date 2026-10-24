//! Putting a project on GitHub: make the repository there (or use the one
//! that is already there), write `[github]` into `harness.toml` and push the
//! main branch. Every step can be run again: a try that stopped half way is
//! finished by the next one.

use std::fs;
use std::path::Path;
use std::time::Duration;

use harness_core::config::{self, Config, GitHubConfig, CONFIG_FILE};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::task::branch::BRANCH_PREFIX;

use super::{quiet_command, PUSH_URL_START};
use crate::install::credentials;
use crate::install::models::run_for;

/// How long one `gh` call may take: it talks to GitHub.
const GH_LIMIT: Duration = Duration::from_mins(1);

/// What `gh repo view` says when there is no such repository (or the user may
/// not see it); any other failure is a real error, not "make a new one".
const NOT_FOUND: &str = "Could not resolve to a Repository";

/// What to connect the project to.
#[derive(Debug, Clone)]
pub struct Connection<'a> {
    /// The `gh` program, signed in.
    pub gh: &'a Path,
    /// The repository: `name` (in the user's own account) or `owner/name`.
    pub name: &'a str,
    /// The repository must be private (only the user sees it): a new one is made
    /// private, and a public one that is already there is refused.
    pub private: bool,
    /// The start of the address git pushes to; the repository and `.git`
    /// are added. [`PUSH_URL_START`] (GitHub), a local folder in tests.
    pub push_start: &'a str,
    /// Saved secrets, hidden in the errors `gh` prints.
    pub secrets: &'a [&'a str],
}

/// Puts the project in `root` on GitHub as the repository `name` (see
/// [`Connection::name`]); needs the GitHub CLI, signed in. Returns the
/// repository as `owner/name`.
pub fn connect(root: &Path, name: &str, private: bool) -> Result<String, String> {
    let gh = super::ready_gh("try again")?;
    let secrets = credentials::default_dir()
        .map(|dir| credentials::saved_secrets(&dir))
        .unwrap_or_default();
    let secrets: Vec<&str> = secrets.iter().map(credentials::Secret::expose).collect();
    let connection = Connection {
        gh: &gh,
        name,
        private,
        push_start: PUSH_URL_START,
        secrets: &secrets,
    };
    connect_with(root, &connection)
}

/// [`connect`] with everything given: tests use a fake `gh` and a local
/// folder as GitHub.
///
/// # Errors
///
/// Nothing is pushed when the project is already on another repository,
/// the main branch is not checked out, the repository is public but should
/// be private, or it has commits the project does not have (someone else's
/// work, or pull requests merged there that were not brought in yet).
pub fn connect_with(root: &Path, connection: &Connection) -> Result<String, String> {
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    let config = Config::load(&root.join(HARNESS_DIR)).map_err(|e| e.to_string())?;
    let mut full = full_name(connection)?;
    if let Some(known) = &config.github {
        // GitHub does not care about capitals in names.
        if !known.repo.eq_ignore_ascii_case(&full) {
            return Err(format!(
                "this project is already on GitHub as {}; change [github] in harness.toml \
                 to move it",
                known.repo
            ));
        }
        full.clone_from(&known.repo);
    }
    let base = main_branch(&repo, config.github.as_ref())?;
    let github = GitHubConfig {
        repo: full.clone(),
        base: base.clone(),
    };
    // Checked before GitHub is asked, and again before the commit.
    settings_to_commit(&repo, &github)?;
    match visibility(connection, &full)? {
        None => create(connection, &full)?,
        Some(seen) if connection.private && seen != "PRIVATE" => {
            return Err(format!(
                "{full} is already on GitHub and is not private; nothing was pushed. Make it \
                 private on GitHub, or untick «Private» (harness github --public)"
            ))
        }
        Some(_) => {}
    }
    let url = format!("{}{full}.git", connection.push_start);
    check_remote(&repo, &url, &full)?;
    // `base` goes to GitHub before `[github]` is committed: a project that
    // says it is on GitHub always has its main branch there for pull requests.
    let push = || {
        repo.push_branch(&url, &base)
            .map_err(|e| format!("cannot push {base} to GitHub: {e}"))
    };
    push()?;
    write_github(&repo, &github)?;
    push()?;
    repo.ensure_origin(&url).map_err(|e| e.to_string())?;
    Ok(full)
}

/// `owner/name`: a plain `name` goes into the account `gh` is signed in to.
fn full_name(connection: &Connection) -> Result<String, String> {
    let name = connection.name.trim();
    let full = if name.contains('/') {
        name.to_string()
    } else {
        let mut command = quiet_command(connection.gh);
        command.args(["api", "user", "--jq=.login"]);
        let owner = run_for(command, "", connection.secrets, GH_LIMIT)?;
        format!("{}/{name}", owner.trim())
    };
    if config::is_repo(&full) {
        Ok(full)
    } else {
        Err(format!(
            "{:?} is not a name GitHub takes: use letters, digits, '-', '_' and '.', for \
             example vgs-prototype",
            harness_core::text::safe_line(&full, 100)
        ))
    }
}

/// The branch pull requests go into: `[github]`'s `base`, or for a new
/// connection the branch checked out now. It must be checked out (the
/// commit goes there, and its history is compared with GitHub), and it
/// must not be a task's branch.
fn main_branch(repo: &Repo, known: Option<&GitHubConfig>) -> Result<String, String> {
    let current = repo
        .current_branch()
        .map_err(|e| e.to_string())?
        .ok_or("no branch is checked out; check out the main branch first")?;
    let base = known.map_or(current.as_str(), |known| known.base.as_str());
    // Capitals too: on macOS and Windows git may take `Harness/` for `harness/`.
    if base.to_ascii_lowercase().starts_with(BRANCH_PREFIX) {
        return Err(format!(
            "{base} is a task's branch; check out the main branch (for example main) first"
        ));
    }
    if base != current {
        return Err(format!(
            "check out {base} first: it is the project's main branch"
        ));
    }
    Ok(base.to_string())
}

/// `PRIVATE`, `PUBLIC` or `INTERNAL` when the repository is there; `None`
/// when GitHub says there is no such repository.
fn visibility(connection: &Connection, full: &str) -> Result<Option<String>, String> {
    let mut command = quiet_command(connection.gh);
    command.args([
        "repo",
        "view",
        full,
        "--json=visibility",
        "--jq=.visibility",
    ]);
    match run_for(command, "", connection.secrets, GH_LIMIT) {
        Ok(out) => Ok(Some(out.trim().to_string())),
        Err(error) if error.contains(NOT_FOUND) => Ok(None),
        Err(error) => Err(format!("cannot look at {full} on GitHub: {error}")),
    }
}

/// Makes an empty repository on GitHub; `gh` does not touch the project.
fn create(connection: &Connection, full: &str) -> Result<(), String> {
    let visibility = if connection.private {
        "--private"
    } else {
        "--public"
    };
    let mut command = quiet_command(connection.gh);
    command.args(["repo", "create", full, visibility]);
    run_for(command, "", connection.secrets, GH_LIMIT)
        .map(drop)
        .map_err(|e| format!("GitHub did not make the repository {full}: {e}"))
}

/// Stops when any branch on GitHub has a commit this project does not
/// know: someone else's work, or pull requests merged there and not brought
/// in yet. Pushing would then fail or mix the two. (The task branches the
/// harness pushed itself are known, wherever they are.)
fn check_remote(repo: &Repo, url: &str, full: &str) -> Result<(), String> {
    let branches = repo
        .remote_branches(url)
        .map_err(|e| format!("cannot look at the repository on GitHub: {e}"))?;
    match branches.iter().find(|(commit, _)| !repo.has_commit(commit)) {
        Some((commit, branch)) => Err(format!(
            "{full} on GitHub has commits this project does not have ({branch} is at \
             {commit}); nothing was pushed. If it is this project's own repository, bring \
             those commits into the project first; otherwise use another name"
        )),
        None => Ok(()),
    }
}

/// The text `harness.toml` must have once `[github]` is in: the last
/// committed text with `github` added (when it is not there yet). The file
/// on disk may be that committed text, or already the new one (an earlier
/// try stopped before its commit); anything else is an edit of the user's that
/// is not committed, and it must not go to GitHub with this commit.
fn settings_to_commit(repo: &Repo, github: &GitHubConfig) -> Result<String, String> {
    let path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    let on_disk =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let shown = format!("{HARNESS_DIR}/{CONFIG_FILE}");
    let committed = repo.file_at_head(&shown).unwrap_or_default();
    let config = Config::parse(&committed).map_err(|e| e.to_string())?;
    let wanted = match config.github {
        Some(_) => committed.clone(),
        None => config::edit::set_github(&committed, github).map_err(|e| e.to_string())?,
    };
    // Git may keep Windows line ends on disk (`core.autocrlf`): they are
    // not a change.
    let on_disk = on_disk.replace("\r\n", "\n");
    if on_disk == committed.replace("\r\n", "\n") || on_disk == wanted {
        Ok(wanted)
    } else {
        Err(format!(
            "{shown} has changes that are not committed: save or undo them first, so they \
             do not go to GitHub by accident"
        ))
    }
}

/// Writes and commits `harness.toml` with `[github]`. Everything is checked
/// again here, not only at the start: the user may have saved other settings,
/// or started a task on another branch, while GitHub was asked.
fn write_github(repo: &Repo, github: &GitHubConfig) -> Result<(), String> {
    main_branch(repo, Some(github))?;
    let wanted = settings_to_commit(repo, github)?;
    let path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    fs::write(&path, wanted).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let message = format!("harness: on GitHub as {}", github.repo);
    repo.commit_paths(&[path.as_path()], &message)
        .map(drop)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// A project with its settings committed on `main`, a folder standing
    /// in for GitHub, and a fake `gh` signed in as `lisa` that records every
    /// call in `calls.txt` and finds no repository.
    struct Setup {
        dir: tempfile::TempDir,
        root: std::path::PathBuf,
        github: String,
        gh: std::path::PathBuf,
    }

    fn setup(gh_script: &str) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        harness_core::config::projects::init(&root).unwrap();
        let repo = Repo::open(&root).unwrap();
        let branch = repo.current_branch().unwrap().unwrap();
        if branch != "main" {
            git(&root, &["branch", "-m", "main"]);
        }
        let github = format!("{}/", dir.path().join("github").display());
        let calls = dir.path().join("calls.txt");
        let script = format!("append-args {}\n{gh_script}", calls.display());
        let gh = harness_fake::install(&dir.path().join("bin"), "gh", &script);
        Setup {
            dir,
            root,
            github,
            gh,
        }
    }

    /// `gh` that answers `lisa` to `api user`, finds no repository with
    /// `repo view`, and says yes to `repo create` (the test makes the
    /// empty repository itself).
    const NEW_REPOSITORY_GH: &str = "when-arg 1 api print lisa\n\
         when-arg 2 view eprint GraphQL: Could not resolve to a Repository with the name\n\
         when-arg 2 view exit 1\n\
         when-arg 2 create exit 0\n";

    /// `gh` that finds the repository, private.
    const PRIVATE_REPOSITORY_GH: &str = "when-arg 2 view print PRIVATE\n";

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn bare(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "--quiet", "--bare"]);
    }

    fn connection<'a>(s: &'a Setup, name: &'a str) -> Connection<'a> {
        Connection {
            gh: &s.gh,
            name,
            private: true,
            push_start: &s.github,
            secrets: &[],
        }
    }

    /// The folder that stands in for `owner/name.git` on GitHub.
    fn dir_of(s: &Setup, repo: &str) -> std::path::PathBuf {
        Path::new(&s.github).join(repo)
    }

    fn calls(s: &Setup) -> String {
        std::fs::read_to_string(s.dir.path().join("calls.txt")).unwrap_or_default()
    }

    /// Another project's history, pushed to `branch` of `repo` on GitHub.
    fn someone_elses_work(s: &Setup, repo: &str, branch: &str) {
        let other = s.dir.path().join("other");
        harness_core::config::projects::init(&other).unwrap();
        std::fs::write(other.join("README.md"), "theirs\n").unwrap();
        git(&other, &["add", "README.md"]);
        git(&other, &["commit", "--quiet", "-m", "theirs"]);
        let theirs = dir_of(s, repo);
        bare(&theirs);
        let refspec = format!("HEAD:refs/heads/{branch}");
        git(
            &other,
            &["push", "--quiet", &theirs.to_string_lossy(), &refspec],
        );
    }

    #[test]
    fn a_new_repository_is_made_written_down_and_pushed() {
        let s = setup(NEW_REPOSITORY_GH);
        // `gh repo create` is fake, so the empty repository is made here.
        bare(&dir_of(&s, "lisa/vgs.git"));

        let full = connect_with(&s.root, &connection(&s, "vgs")).unwrap();

        assert_eq!(full, "lisa/vgs");
        assert!(
            calls(&s).contains("repo\ncreate\nlisa/vgs\n--private\n"),
            "{}",
            calls(&s)
        );
        let config = Config::load(&s.root.join(HARNESS_DIR)).unwrap();
        assert_eq!(
            config.github,
            Some(GitHubConfig {
                repo: "lisa/vgs".into(),
                base: "main".into()
            })
        );
        let repo = Repo::open(&s.root).unwrap();
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        let url = format!("{}lisa/vgs.git", s.github);
        assert_eq!(
            repo.remote_branch_head(&url, "main").unwrap(),
            repo.head().unwrap()
        );
        assert_eq!(git(&s.root, &["remote", "get-url", "origin"]).trim(), url);
    }

    #[test]
    fn connecting_again_only_pushes() {
        let s = setup(NEW_REPOSITORY_GH);
        bare(&dir_of(&s, "lisa/vgs.git"));
        connect_with(&s.root, &connection(&s, "vgs")).unwrap();
        let head = Repo::open(&s.root).unwrap().head().unwrap();

        // Capitals do not matter to GitHub; the saved name is kept.
        let full = connect_with(&s.root, &connection(&s, "Lisa/VGS")).unwrap();

        assert_eq!(full, "lisa/vgs");
        assert_eq!(Repo::open(&s.root).unwrap().head().unwrap(), head);
        let error = connect_with(&s.root, &connection(&s, "lisa/other")).unwrap_err();
        assert!(error.contains("already on GitHub as lisa/vgs"), "{error}");
    }

    #[test]
    fn a_try_that_stopped_before_its_commit_is_finished_next_time() {
        let s = setup(PRIVATE_REPOSITORY_GH);
        bare(&dir_of(&s, "lisa/vgs.git"));
        let path = s.root.join(HARNESS_DIR).join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        let github = GitHubConfig {
            repo: "lisa/vgs".into(),
            base: "main".into(),
        };
        std::fs::write(&path, config::edit::set_github(&text, &github).unwrap()).unwrap();

        connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap();

        let repo = Repo::open(&s.root).unwrap();
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        let url = format!("{}lisa/vgs.git", s.github);
        assert_eq!(
            repo.remote_branch_head(&url, "main").unwrap(),
            repo.head().unwrap()
        );
    }

    #[test]
    fn task_branches_the_harness_pushed_do_not_stop_a_second_try() {
        let s = setup(PRIVATE_REPOSITORY_GH);
        bare(&dir_of(&s, "lisa/vgs.git"));
        connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap();
        // A finished task's branch, pushed to GitHub as a pull request.
        git(&s.root, &["checkout", "--quiet", "-b", "harness/task-001"]);
        std::fs::write(s.root.join("work.txt"), "done\n").unwrap();
        git(&s.root, &["add", "work.txt"]);
        git(&s.root, &["commit", "--quiet", "-m", "work"]);
        let url = format!("{}lisa/vgs.git", s.github);
        git(&s.root, &["push", "--quiet", &url, "harness/task-001"]);
        git(&s.root, &["checkout", "--quiet", "main"]);

        connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap();
    }

    #[test]
    fn settings_changed_by_hand_and_not_committed_are_not_sent() {
        let s = setup(PRIVATE_REPOSITORY_GH);
        bare(&dir_of(&s, "lisa/vgs.git"));
        let path = s.root.join(HARNESS_DIR).join(CONFIG_FILE);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("# a note for me only\n");
        std::fs::write(&path, &text).unwrap();

        let error = connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap_err();

        assert!(
            error.contains("has changes that are not committed"),
            "{error}"
        );
        assert!(!calls(&s).contains("repo"), "{}", calls(&s));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn an_existing_repository_with_other_work_is_not_touched() {
        for branch in ["main", "master"] {
            let s = setup(PRIVATE_REPOSITORY_GH);
            someone_elses_work(&s, "lisa/taken.git", branch);

            let error = connect_with(&s.root, &connection(&s, "lisa/taken")).unwrap_err();

            assert!(
                error.contains("has commits this project does not have"),
                "{error}"
            );
            assert!(error.contains(&format!("({branch} is at")), "{error}");
            assert!(!calls(&s).contains("create"), "{}", calls(&s));
            assert_eq!(
                Config::load(&s.root.join(HARNESS_DIR)).unwrap().github,
                None
            );
        }
    }

    #[test]
    fn a_public_repository_is_not_used_for_a_private_project() {
        let s = setup("when-arg 2 view print PUBLIC\n");
        bare(&dir_of(&s, "lisa/open.git"));

        let error = connect_with(&s.root, &connection(&s, "lisa/open")).unwrap_err();

        assert!(error.contains("is not private"), "{error}");
        assert_eq!(
            Config::load(&s.root.join(HARNESS_DIR)).unwrap().github,
            None
        );
        // Asked for a public one, it is fine.
        let public = Connection {
            private: false,
            ..connection(&s, "lisa/open")
        };
        connect_with(&s.root, &public).unwrap();
    }

    #[test]
    fn a_github_error_is_not_taken_for_a_missing_repository() {
        let s = setup("when-arg 2 view eprint HTTP 502: Bad gateway\nwhen-arg 2 view exit 1\n");

        let error = connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap_err();

        assert!(error.contains("Bad gateway"), "{error}");
        assert!(!calls(&s).contains("create"), "{}", calls(&s));
    }

    #[test]
    fn a_bad_name_or_branch_is_refused_before_anything_is_made() {
        let s = setup(NEW_REPOSITORY_GH);
        for wrong in ["lisa/../x", "lisa/vgs.git"] {
            let error = connect_with(&s.root, &connection(&s, wrong)).unwrap_err();
            assert!(error.contains("not a name GitHub takes"), "{error}");
        }

        git(&s.root, &["checkout", "--quiet", "-b", "harness/task-001"]);
        let error = connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap_err();
        assert!(error.contains("is a task's branch"), "{error}");

        // A base written in harness.toml must be the branch checked out.
        git(&s.root, &["checkout", "--quiet", "main"]);
        let path = s.root.join(HARNESS_DIR).join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        let github = GitHubConfig {
            repo: "lisa/vgs".into(),
            base: "develop".into(),
        };
        std::fs::write(&path, config::edit::set_github(&text, &github).unwrap()).unwrap();
        let error = connect_with(&s.root, &connection(&s, "lisa/vgs")).unwrap_err();
        assert!(error.contains("check out develop first"), "{error}");
        assert!(!calls(&s).contains("repo"), "{}", calls(&s));
    }
}
