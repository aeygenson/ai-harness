//! `harness`: the command line. A thin layer: every real decision lives in
//! `harness-core`, so the TUI can later reuse the same functions.
//!
//! ```text
//! harness init                         prepare .harness/ in the project
//! harness login claude                 save the token from `claude setup-token`
//! harness login codex                  log in to Codex (ChatGPT subscription)
//! harness login gemini                 log in to Gemini CLI (Google account)
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
use harness_agents::{AnyAgent, ClaudeCode, Codex, Gemini, Team};
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
        /// `claude`, `codex` or `gemini`.
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
        "gemini" => login_gemini(&dir),
        other => bail!("unknown agent {other:?}; use `claude`, `codex` or `gemini`"),
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

/// Starts `gemini` with its home in our credentials folder. Gemini has no
/// separate login command: Lisa picks "Login with Google", then quits with
/// `/quit`, and the login stays in that folder, not in `~/.gemini`.
fn login_gemini(dir: &Path) -> Result<()> {
    let home = dir.join("gemini");
    fs::create_dir_all(&home)?;
    println!("Starting `gemini`. Choose \"Login with Google\", finish in the browser,");
    println!("then type /quit to come back here.");
    let status = std::process::Command::new("gemini")
        .env("GEMINI_CLI_HOME", &home)
        .env("GOOGLE_GENAI_USE_GCA", "true")
        .env("GEMINI_FORCE_FILE_STORAGE", "true")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .status()
        .context("cannot start `gemini`; is Gemini CLI installed?")?;
    if !status.success() {
        bail!("`gemini` failed ({status})");
    }
    let auth = home.join(".gemini").join("oauth_creds.json");
    if !auth.exists() {
        bail!(
            "`gemini` finished but {} was not created; did the login finish?",
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
            "gemini" => {
                let auth_dir = dir.join("gemini");
                if !auth_dir.join(".gemini/oauth_creds.json").exists() {
                    bail!("no Gemini login saved; run `harness login gemini` first");
                }
                let mut agent = Gemini::new(auth_dir).with_timeout(timeout);
                if let Some(model) = &settings.model {
                    agent = agent.with_model(role, model);
                }
                AnyAgent::Gemini(agent)
            }
            other => {
                bail!("{role:?} uses agent {other:?}; use \"claude\", \"codex\" or \"gemini\"")
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
