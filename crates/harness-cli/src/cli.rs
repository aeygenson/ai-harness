//! The command line itself: every `harness` subcommand and its options, as
//! `clap` reads them, plus the parsers for role names.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use harness_core::handoff::{NextStep, Role};

#[derive(Parser)]
#[command(
    name = "harness",
    version,
    about = "Runs a team of AI roles on a project"
)]
pub(crate) struct Cli {
    /// The project folder (a git repository).
    #[arg(short = 'C', long, default_value = ".", global = true)]
    pub(crate) project: PathBuf,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create .harness/ with a default harness.toml and commit it.
    Init,
    /// Save an agent's login (outside the project, never in git). Hidden:
    /// agents are signed in on the TUI's Agents tab, which runs this in its
    /// terminal.
    #[command(hide = true)]
    Login {
        /// `claude`, `codex`, `deepseek` or `antigravity`.
        agent: String,
    },
    /// Secrets for MCP servers, such as API keys (outside the project, never in git).
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// MCP servers on the web that want a sign-in in the browser (OAuth).
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Plugin catalogs, for all projects (kept in ~/.harness/).
    Marketplace {
        #[command(subcommand)]
        command: MarketplaceCommand,
    },
    /// Plugins of this project, copied from the catalogs.
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
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
    /// Count what happened in a task (or in all tasks) and save it in
    /// .harness/retros/<NNN>/; with --suggest also ask for skill proposals.
    Retro(RetroArgs),
    /// A full-screen window with tabs: tasks, role settings, projects.
    Tui,
    /// The models (and effort levels) each agent offers, as last asked.
    Models {
        /// Ask every agent with a saved login again first.
        #[arg(long)]
        refresh: bool,
    },
    /// Which agents of the catalog are installed here, their versions and
    /// logins, and how to install or update them.
    Agents,
    /// Used by Codex: start an MCP server from its private settings file.
    #[command(name = "mcp-exec", hide = true)]
    McpExec { file: PathBuf },
    /// Used by the agents: the bridge to an MCP server on the web.
    #[command(name = "mcp-remote", hide = true)]
    McpRemote,
}

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct RetroArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub(crate) task_id: Option<String>,
    /// All tasks of the project.
    #[arg(long)]
    pub(crate) all: bool,
    /// Also let the [retro] agent read the history and propose skill changes.
    #[arg(long)]
    pub(crate) suggest: bool,
    #[command(subcommand)]
    pub(crate) command: Option<RetroCommand>,
}

#[derive(Subcommand)]
pub(crate) enum RetroCommand {
    /// Show a retrospective's notes and proposals, with the changes they make.
    Show { number: String },
    /// Apply the chosen proposals, for example `harness retro apply 004 1 3`.
    Apply {
        number: String,
        #[arg(required = true)]
        ids: Vec<u32>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SecretCommand {
    /// Save a secret; harness.toml uses it as "secret:<name>".
    Set { name: String },
    /// Show the names of the saved secrets (never their values).
    List,
}

#[derive(Subcommand)]
pub(crate) enum McpCommand {
    /// Sign in to [mcp.<name>] (auth = "oauth") in the browser, once; the
    /// harness keeps the tokens in ~/.harness/credentials/oauth/.
    Login { name: String },
    /// Forget the sign-in of [mcp.<name>].
    Logout { name: String },
}

#[derive(Subcommand)]
pub(crate) enum MarketplaceCommand {
    /// Add a catalog: `owner/repo` on GitHub, a git address or a local folder.
    Add {
        source: String,
        /// Use this name instead of the one in the catalog.
        #[arg(long)]
        name: Option<String>,
    },
    /// Show the added catalogs.
    List,
    /// Download the newest version of one or all catalogs.
    Update { name: Option<String> },
    /// Forget a catalog (plugins already in projects stay).
    Remove { name: String },
}

#[derive(Subcommand)]
pub(crate) enum PluginCommand {
    /// Show the plugins of all added catalogs.
    List {
        /// Only plugins for `claude` or `codex`.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Copy a plugin into the project: `name` or `name@catalog`.
    Add {
        name: String,
        /// `claude` or `codex`, when the catalog has the plugin for both.
        #[arg(long)]
        agent: Option<String>,
        /// Also give it to this role.
        #[arg(long, value_parser = parse_role)]
        role: Option<Role>,
        /// Allow the plugin's hooks (commands that run by themselves).
        #[arg(long)]
        allow_hooks: bool,
        /// Allow the plugin's own MCP or LSP servers.
        #[arg(long)]
        allow_mcp: bool,
    },
    /// Take the newer version from its catalog and show what changed.
    Update { name: String },
    /// Remove a plugin from the project and from every role.
    Remove { name: String },
}

#[derive(Subcommand)]
pub(crate) enum TaskCommand {
    /// Create a task. The description comes as text or from a file.
    New {
        task_id: String,
        description: Option<String>,
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
        /// Create it even if an unfinished task has the same text.
        #[arg(long)]
        allow_duplicate: bool,
    },
}

/// `architect`, `developer`, ... or `done`: the same words as in handoff.json.
fn parse_next(text: &str) -> Result<NextStep, String> {
    if text == "done" {
        return Ok(NextStep::Done);
    }
    text.parse()
        .map(NextStep::To)
        .map_err(|_| format!("{text:?} is not a role or `done`"))
}

/// `architect`, `developer`, `tester` or `security`.
fn parse_role(text: &str) -> Result<Role, String> {
    match text.parse() {
        Ok(role) if role != Role::Human => Ok(role),
        _ => Err(format!("{text:?} is not a role")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::handoff::{NextStep, Role};

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
}
