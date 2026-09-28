//! `harness`: the command line. A thin layer: every real decision lives in
//! `harness-core`, so the TUI can later reuse the same functions.
//!
//! ```text
//! harness init                         prepare .harness/ in the project
//! harness login claude                 save the token from `claude setup-token`
//! harness login codex                  log in to Codex (ChatGPT subscription)
//! harness login deepseek               save the DeepSeek API key
//! harness login antigravity            log in to Antigravity CLI (Google account)
//! harness task new task-001 "Build a CSV parser"
//! harness run task-001                 run roles until someone must look
//! harness approve task-001 --notes "Looks good"
//! harness reject task-001 --to architect --notes "Use serde"
//! harness status task-001
//! ```

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use harness_agents::credentials::{self, Secret};
use harness_agents::{codex, Antigravity, AnyAgent, ClaudeCode, Codex, Team};
use harness_core::config::{Config, CONFIG_FILE, DEFAULT_CONFIG};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::{NextStep, Role, Verdict};
use harness_core::orchestrator::{self, StopReason};
use harness_core::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason};

#[derive(Parser)]
#[command(name = "harness", about = "Runs a team of AI roles on a project")]
struct Cli {
    /// The project folder (a git repository).
    #[arg(short = 'C', long, default_value = ".", global = true)]
    project: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create .harness/ with a default harness.toml and commit it.
    Init,
    /// Save an agent's login token (outside the project, never in git).
    Login {
        /// `claude`, `codex`, `deepseek` or `antigravity`.
        agent: String,
    },
    /// Work with tasks.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Run roles until the task is done or needs Lisa.
    Run { task_id: String },
    /// Accept the work and send it on.
    Approve {
        task_id: String,
        /// Who works next: a role or `done`. Usually chosen automatically.
        #[arg(long, value_parser = parse_next)]
        to: Option<NextStep>,
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// Send the work back to a role.
    Reject {
        task_id: String,
        #[arg(long, value_parser = parse_next)]
        to: NextStep,
        #[arg(long)]
        notes: String,
    },
    /// Show where a task is and its history.
    Status { task_id: String },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Create a task. The description comes as text or from a file.
    New {
        task_id: String,
        description: Option<String>,
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
    },
}

/// `architect`, `developer`, ... or `done`: the same words as in handoff.json.
fn parse_next(text: &str) -> Result<NextStep, String> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| format!("{text:?} is not a role or `done`"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let project = &cli.project;
    match cli.command {
        Command::Init => init(project),
        Command::Login { agent } => login(&agent),
        Command::Task {
            command:
                TaskCommand::New {
                    task_id,
                    description,
                    file,
                },
        } => {
            let description = match (description, file) {
                (Some(text), _) => text,
                (None, Some(file)) => fs::read_to_string(&file)
                    .with_context(|| format!("cannot read {}", file.display()))?,
                (None, None) => bail!("give a description or --file"),
            };
            new_task(project, &task_id, &description)
        }
        Command::Run { task_id } => run(project, &task_id).await,
        Command::Approve { task_id, to, notes } => {
            decide(project, &task_id, Verdict::Approved, to, &notes)
        }
        Command::Reject { task_id, to, notes } => {
            decide(project, &task_id, Verdict::Rejected, Some(to), &notes)
        }
        Command::Status { task_id } => status(project, &task_id),
    }
}

fn open_repo(project: &Path) -> Result<Repo> {
    Repo::open(project).context("the project must be a git repository (run `git init`)")
}

fn open_task(repo: &Repo, task_id: &str) -> Result<(TaskStore, TaskState)> {
    TaskStore::open(&repo.runs_dir(), task_id)
        .with_context(|| format!("cannot open task {task_id}"))
}

fn init(project: &Path) -> Result<()> {
    let repo = open_repo(project)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = harness_dir.join(CONFIG_FILE);
    if config.exists() {
        println!("{} already exists, left as it is.", config.display());
    } else {
        fs::create_dir_all(&harness_dir)?;
        fs::write(&config, DEFAULT_CONFIG)?;
        println!("Created {}.", config.display());
    }
    repo.ensure_harness_ignores()?;
    repo.commit_paths(&[&config], "harness: settings")?;
    Ok(())
}

fn login(agent: &str) -> Result<()> {
    let dir = credentials::default_dir().context("HOME is not set")?;
    match agent {
        "claude" => login_claude(&dir),
        "codex" => login_codex(&dir),
        "deepseek" => login_deepseek(&dir),
        "antigravity" => login_antigravity(&dir),
        other => {
            bail!("unknown agent {other:?}; use `claude`, `codex`, `deepseek` or `antigravity`")
        }
    }
}

fn login_claude(dir: &Path) -> Result<()> {
    println!("1. In another terminal run:  claude setup-token");
    println!("2. Paste the token it prints here and press Enter.");
    print!("Token: ");
    io::stdout().flush()?;
    let mut token = String::new();
    io::stdin().lock().read_line(&mut token)?;
    let token = Secret::new(token.trim());
    if token.expose().is_empty() {
        bail!("no token given");
    }
    let path = credentials::save_token(dir, "claude", &token)?;
    println!("Saved to {} (only you can read it).", path.display());
    Ok(())
}

/// Saves the DeepSeek API key like the Claude token: in
/// `~/.harness/credentials/deepseek/`, readable only by Lisa. The key is typed
/// without showing it on the screen.
fn login_deepseek(dir: &Path) -> Result<()> {
    println!("Paste your DeepSeek API key (from platform.deepseek.com) and press Enter.");
    println!("It is not shown while you type: paste it once, then press Enter.");
    let key = read_hidden("API key: ")?;
    check_deepseek_key(key.expose())?;
    let path = credentials::save_token(dir, "deepseek", &key)?;
    println!(
        "Saved {} to {} (only you can read it).",
        masked(key.expose()),
        path.display()
    );
    Ok(())
}

/// A DeepSeek key is `sk-` and letters or digits, with no spaces. Pasting it
/// twice into the hidden prompt is easy, because nothing shows up.
fn check_deepseek_key(key: &str) -> Result<()> {
    let copies = key.matches("sk-").count();
    if copies > 1 {
        bail!("the key seems to be pasted {copies} times; run the command again and paste it once");
    }
    let body = key.strip_prefix("sk-").unwrap_or("");
    if body.len() < 16 || !body.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("this does not look like a DeepSeek API key (it starts with sk-); nothing was saved");
    }
    Ok(())
}

/// `sk-…1a2b (35 characters)`: enough to recognise the key, not to use it.
fn masked(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("sk-…{tail} ({} characters)", key.chars().count())
}

/// Reads one line from the terminal without echoing it.
fn read_hidden(prompt: &str) -> Result<Secret> {
    print!("{prompt}");
    io::stdout().flush()?;
    let stty = |arg: &str| {
        std::process::Command::new("stty")
            .arg(arg)
            .stdin(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let hidden = stty("-echo");
    let mut line = String::new();
    let read = io::stdin().lock().read_line(&mut line);
    if hidden {
        stty("echo");
        println!();
    }
    read?;
    Ok(Secret::new(line.trim()))
}

/// Runs `codex login` with its home in our credentials folder, so the login is
/// saved there and not in `~/.codex`.
fn login_codex(dir: &Path) -> Result<()> {
    let home = dir.join("codex");
    fs::create_dir_all(&home)?;
    println!("Starting `codex login`; finish the login in your browser.");
    let status = std::process::Command::new("codex")
        .arg("login")
        .env("CODEX_HOME", &home)
        .status()
        .context("cannot start `codex`; is Codex CLI installed?")?;
    if !status.success() {
        bail!("`codex login` failed ({status})");
    }
    let auth = home.join("auth.json");
    if !auth.exists() {
        bail!(
            "`codex login` finished but {} was not created",
            auth.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o600))?;
    }
    println!("Saved to {} (only you can read it).", auth.display());
    Ok(())
}

/// Starts `agy` with `HOME` set to our credentials folder, so the login is
/// saved there and not in `~/.gemini`. `agy` has no separate login command:
/// Lisa signs in with Google, then quits with `/quit`. The environment is
/// empty like for the agents, so the login goes to files, not the keyring.
fn login_antigravity(dir: &Path) -> Result<()> {
    let home = dir.join("antigravity");
    fs::create_dir_all(&home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    }
    println!("Starting `agy`. Sign in with Google, then type /quit to come back here.");
    let mut command = std::process::Command::new("agy");
    command.env_clear().env("HOME", &home);
    for name in [
        "PATH",
        "TERM",
        "LANG",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let status = command
        .status()
        .context("cannot start `agy`; is Antigravity CLI installed?")?;
    if !status.success() {
        bail!("`agy` failed ({status})");
    }
    if !home.join(".gemini/antigravity-cli").is_dir() {
        bail!(
            "`agy` finished but did not save a login in {}",
            home.display()
        );
    }
    println!("Saved in {} (only you can read it).", home.display());
    Ok(())
}

fn new_task(project: &Path, task_id: &str, description: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let (store, _) = orchestrator::create_task(&repo, task_id, description, config.max_rounds)?;
    println!(
        "Created {}. Next: harness run {task_id}",
        store.dir().display()
    );
    Ok(())
}

/// Builds the team from harness.toml: each role gets the agent and model set there.
fn build_team(config: &Config) -> Result<Team> {
    let dir = credentials::default_dir().context("HOME is not set")?;
    let timeout = Duration::from_secs(config.agent_timeout_minutes * 60);
    let mut team = Team::new();
    for role in [
        Role::Architect,
        Role::Developer,
        Role::Tester,
        Role::Security,
    ] {
        let settings = config.role(role)?;
        let agent = match settings.agent.as_str() {
            "claude" => {
                let token = credentials::load_token(&dir, "claude")
                    .context("no Claude token saved; run `harness login claude` first")?;
                let mut agent = ClaudeCode::new(token).with_timeout(timeout);
                if let Some(model) = &settings.model {
                    agent = agent.with_model(role, model);
                }
                AnyAgent::Claude(agent)
            }
            "codex" => {
                let auth_dir = dir.join("codex");
                if !auth_dir.join("auth.json").exists() {
                    bail!("no Codex login saved; run `harness login codex` first");
                }
                let mut agent = Codex::new(auth_dir).with_timeout(timeout);
                if let Some(model) = &settings.model {
                    agent = agent.with_model(role, model);
                }
                AnyAgent::Codex(agent)
            }
            "codex+deepseek" => {
                // A key in the shell wins; otherwise the one `harness login deepseek` saved.
                let key = match std::env::var(codex::DEEPSEEK_KEY_ENV) {
                    Ok(key) if !key.trim().is_empty() => Secret::new(key.trim()),
                    _ => credentials::load_token(&dir, "deepseek")
                        .context("no DeepSeek API key saved; run `harness login deepseek` first")?,
                };
                let mut agent = Codex::deepseek(key).with_timeout(timeout);
                if let Some(model) = &settings.model {
                    agent = agent.with_model(role, model);
                }
                AnyAgent::Codex(agent)
            }
            "antigravity" => {
                let auth_dir = dir.join("antigravity");
                if !auth_dir.join(".gemini/antigravity-cli").is_dir() {
                    bail!("no Antigravity login saved; run `harness login antigravity` first");
                }
                let mut agent = Antigravity::new(auth_dir).with_timeout(timeout);
                if let Some(model) = &settings.model {
                    agent = agent.with_model(role, model);
                }
                AnyAgent::Antigravity(agent)
            }
            other => {
                bail!("{role:?} uses agent {other:?}; use \"claude\", \"codex\", \"codex+deepseek\" or \"antigravity\"")
            }
        };
        team = team.with(role, agent);
    }
    Ok(team)
}

async fn run(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let agent = build_team(&config)?;
    let (store, mut state) = open_task(&repo, task_id)?;
    println!("Running {task_id} (round {})...", state.round);
    let stop = orchestrator::run(&repo, &store, &mut state, &agent).await?;
    println!("{}", explain(&stop, task_id));
    Ok(())
}

fn explain(stop: &StopReason, task_id: &str) -> String {
    match stop {
        StopReason::Done => "Done: security approved the work.".into(),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => format!(
            "The architect's design is ready. Read it, then:\n  \
             harness approve {task_id}   or   harness reject {task_id} --to architect --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => format!(
            "The {role:?} needs your help; see its notes.md. Answer with:\n  \
             harness approve {task_id} --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::RoundLimitReached) => format!(
            "The round limit is reached. Decide who continues:\n  \
             harness approve {task_id} --to <role|done> --notes \"...\""
        ),
        StopReason::UsageLimitReached(role) => format!(
            "The subscription limit stopped the {role:?}. Nothing was saved; run again later."
        ),
        StopReason::RoleFailed { role, problem } => {
            format!("The {role:?} failed twice. Last problem:\n{problem}")
        }
        StopReason::StepLimitReached => "Stopped after too many steps in one run.".into(),
        StopReason::DirtyWorkingTree(files) => format!(
            "The project has uncommitted changes. Commit or remove them first:\n  {}",
            files.join("\n  ")
        ),
        StopReason::ForbiddenChanges { role, files } => format!(
            "The {role:?} changed files it may not touch (left uncommitted for you to check):\n  {}",
            files.join("\n  ")
        ),
        StopReason::AgentCommitted(role) => format!(
            "The {role:?} made a git commit itself. Check `git log` before running again."
        ),
    }
}

fn decide(
    project: &Path,
    task_id: &str,
    verdict: Verdict,
    to: Option<NextStep>,
    notes: &str,
) -> Result<()> {
    let repo = open_repo(project)?;
    let (store, mut state) = open_task(&repo, task_id)?;
    let next = match to {
        Some(next) => next,
        None => match state.stage {
            Stage::WaitingForHuman(WaitReason::ApproveDesign) => NextStep::To(Role::Developer),
            Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => NextStep::To(role),
            _ => bail!("say who continues with --to <role|done>"),
        },
    };
    orchestrator::record_human_decision(&repo, &store, &mut state, verdict, next, notes)?;
    println!("Saved. Next: harness run {task_id}");
    Ok(())
}

fn status(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let (store, state) = open_task(&repo, task_id)?;
    println!(
        "{task_id}: round {} of {}, {:?}",
        state.round, state.max_rounds, state.stage
    );
    for handoff in store.history()? {
        println!(
            "  round {} {:?} {:?} -> {:?}: {}",
            handoff.round, handoff.role, handoff.verdict, handoff.next_role, handoff.summary
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deepseek_key_pasted_twice_is_refused() {
        let key = "sk-0123456789abcdef0123456789abcdef";
        assert!(check_deepseek_key(key).is_ok());
        let twice = format!("{key}{key}");
        let error = check_deepseek_key(&twice).unwrap_err().to_string();
        assert!(error.contains("2 times"), "{error}");
        assert!(check_deepseek_key("hello").is_err());
        assert!(check_deepseek_key("").is_err());
    }

    #[test]
    fn the_saved_key_is_shown_masked() {
        assert_eq!(
            masked("sk-0123456789abcdef0123456789abcdef"),
            "sk-…cdef (35 characters)"
        );
    }

    #[test]
    fn next_step_words_are_the_handoff_words() {
        assert_eq!(parse_next("tester"), Ok(NextStep::To(Role::Tester)));
        assert_eq!(parse_next("done"), Ok(NextStep::Done));
        assert!(parse_next("boss").is_err());
    }

    #[test]
    fn the_command_line_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn init_then_new_task() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        init(dir.path()).unwrap();
        init(dir.path()).unwrap(); // a second time changes nothing
        new_task(dir.path(), "task-001", "Build a parser").unwrap();

        assert!(repo.changed_files().unwrap().is_empty());
        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Architect));
    }
}
