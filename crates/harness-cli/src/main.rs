//! `harness`: the command line. A thin layer: every real decision lives in
//! `harness-core`, so the TUI can later reuse the same functions.
//!
//! ```text
//! harness init                         prepare .harness/ in the project
//! harness secret set context7          save a secret an MCP server needs
//! harness secret list                  show the names of the saved secrets
//! harness marketplace add anthropics/claude-plugins-official
//! harness plugin list                  plugins in all added catalogs
//! harness plugin add code-review --role security
//! harness plugin update code-review    take the catalog's newer version
//! harness task new task-001 "Build a CSV parser"
//! harness run task-001                 run roles until someone must look
//! harness approve task-001 --notes "Looks good"
//! harness reject task-001 --to architect --notes "Use serde"
//! harness status task-001
//! harness retro task-001               statistics of one task (or --all)
//! harness retro task-001 --suggest     ... and skill proposals from the [retro] agent
//! harness retro show 004               the notes and proposals, with diffs
//! harness retro apply 004 1 3          apply proposals 1 and 3
//! harness tui                          full-screen window: tasks, settings, projects
//! ```

mod catalogs;
mod cli;
mod info;
mod login;
mod retro;
mod secrets;
mod tasks;

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;
use harness_agents::install::credentials;
use harness_agents::launcher;
use harness_core::config::projects;
use harness_core::config::CONFIG_FILE;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::task::handoff::Verdict;
use harness_core::task::store::TaskStore;
use harness_core::task::TaskState;

use crate::cli::{
    Cli, Command, MarketplaceCommand, McpCommand, PluginCommand, RetroCommand, SecretCommand,
    TaskCommand,
};
use crate::info::{agents, models};
use crate::login::login;
use crate::retro::{retro, retro_apply, retro_show};
use crate::secrets::{list_secrets, mcp_login, set_secret};
use crate::tasks::{decide, new_task, run, status, until_ctrl_c};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let project = &cli.project;
    match cli.command {
        Command::Init => init(project),
        Command::Login { agent } => login(&agent),
        Command::Secret {
            command: SecretCommand::Set { name },
        } => set_secret(&name),
        Command::Secret {
            command: SecretCommand::List,
        } => list_secrets(),
        Command::Mcp {
            command: McpCommand::Login { name },
        } => mcp_login(project, &name),
        Command::Mcp {
            command: McpCommand::Logout { name },
        } => {
            let dir = credentials::default_dir().context("no home folder found")?;
            harness_agents::mcp::oauth::logout(&dir, &name)?;
            println!("Forgot the sign-in of {name}.");
            Ok(())
        }
        Command::Marketplace { command } => match command {
            MarketplaceCommand::Add { source, name } => {
                catalogs::marketplace_add(&source, name.as_deref())
            }
            MarketplaceCommand::List => catalogs::marketplace_list(),
            MarketplaceCommand::Update { name } => catalogs::marketplace_update(name.as_deref()),
            MarketplaceCommand::Remove { name } => catalogs::marketplace_remove(&name),
        },
        Command::Plugin { command } => match command {
            PluginCommand::List { agent } => catalogs::plugin_list(project, agent.as_deref()),
            PluginCommand::Add {
                name,
                agent,
                role,
                allow_hooks,
                allow_mcp,
            } => catalogs::plugin_add(
                project,
                &name,
                &catalogs::AddOptions {
                    agent: agent.as_deref(),
                    role,
                    allow_hooks,
                    allow_mcp,
                },
            ),
            PluginCommand::Update { name } => catalogs::plugin_update(project, &name),
            PluginCommand::Remove { name } => catalogs::plugin_remove(project, &name),
        },
        Command::Task {
            command:
                TaskCommand::New {
                    task_id,
                    description,
                    file,
                    allow_duplicate,
                },
        } => {
            let description = match (description, file) {
                (Some(text), _) => text,
                (None, Some(file)) => fs::read_to_string(&file)
                    .with_context(|| format!("cannot read {}", file.display()))?,
                (None, None) => bail!("give a description or --file"),
            };
            new_task(project, &task_id, &description, allow_duplicate)
        }
        Command::Run { task_id } => until_ctrl_c(run(project, &task_id)).await,
        Command::Approve { task_id, to, notes } => {
            decide(project, &task_id, Verdict::Approved, to, &notes)
        }
        Command::Reject { task_id, to, notes } => {
            decide(project, &task_id, Verdict::Rejected, Some(to), &notes)
        }
        Command::Status { task_id } => status(project, &task_id),
        Command::Retro(args) => match args.command {
            Some(RetroCommand::Show { number }) => retro_show(project, &number),
            Some(RetroCommand::Apply { number, ids }) => retro_apply(project, &number, &ids),
            None => until_ctrl_c(retro(project, args.task_id.as_deref(), args.suggest)).await,
        },
        Command::Models { refresh } => models(refresh),
        Command::Agents => {
            agents();
            Ok(())
        }
        Command::Tui => {
            // Inside a project it opens that project, anywhere else the last one.
            let start =
                Repo::open(project).map_or_else(|_| project.clone(), |r| r.root().to_path_buf());
            harness_tui::run(&start)
        }
        Command::McpExec { file } => {
            let error = launcher::exec_server(&file);
            bail!(
                "cannot start the MCP server from {}: {error}",
                file.display()
            )
        }
        Command::McpRemote => {
            harness_agents::mcp::remote::run_from_env().map_err(anyhow::Error::msg)
        }
    }
}

/// Opens the project's git repository, or says that `git init` is needed.
pub(crate) fn open_repo(project: &Path) -> Result<Repo> {
    Repo::open(project).context("the project must be a git repository (run `git init`)")
}

/// Opens the task folder `runs/<task_id>` and reads where the task stands.
pub(crate) fn open_task(repo: &Repo, task_id: &str) -> Result<(TaskStore, TaskState)> {
    TaskStore::open(&repo.runs_dir(), task_id)
        .with_context(|| format!("cannot open task {task_id}"))
}

/// `harness init`: creates the git repository and `.harness/harness.toml` if
/// they are missing; an existing config is left as it is.
pub(crate) fn init(project: &Path) -> Result<()> {
    let done = projects::init(project)?;
    let config = project.join(HARNESS_DIR).join(CONFIG_FILE);
    if done.created_git {
        println!("Created a git repository in {}.", project.display());
    }
    if done.created_config {
        println!("Created {}.", config.display());
    } else {
        println!("{} already exists, left as it is.", config.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::task::handoff::Role;
    use harness_core::task::Stage;

    #[test]
    fn init_then_new_task() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        init(dir.path()).unwrap();
        init(dir.path()).unwrap(); // a second time changes nothing
        new_task(dir.path(), "task-001", "Build a parser", false).unwrap();

        assert!(repo.changed_files().unwrap().is_empty());
        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Architect));
    }
}
