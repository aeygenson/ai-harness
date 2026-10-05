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

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

mod catalogs;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use harness_agents::build::{build_team, retro_agent};
use harness_agents::credentials::{self, Secret};
use harness_agents::launcher;
use harness_core::config::{Config, CONFIG_FILE};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::{NextStep, Role, Verdict};
use harness_core::mcp;
use harness_core::orchestrator::{self, StopReason};
use harness_core::projects;
use harness_core::proposals::FileChange;
use harness_core::retro_ops;
use harness_core::skills::Skills;
use harness_core::store::TaskStore;
use harness_core::suggest::{self, Applied};
use harness_core::task::{Stage, TaskState, WaitReason};

#[derive(Parser)]
#[command(
    name = "harness",
    version,
    about = "Runs a team of AI roles on a project"
)]
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
struct RetroArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    task_id: Option<String>,
    /// All tasks of the project.
    #[arg(long)]
    all: bool,
    /// Also let the [retro] agent read the history and propose skill changes.
    #[arg(long)]
    suggest: bool,
    #[command(subcommand)]
    command: Option<RetroCommand>,
}

#[derive(Subcommand)]
enum RetroCommand {
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
enum SecretCommand {
    /// Save a secret; harness.toml uses it as "secret:<name>".
    Set { name: String },
    /// Show the names of the saved secrets (never their values).
    List,
}

#[derive(Subcommand)]
enum McpCommand {
    /// Sign in to [mcp.<name>] (auth = "oauth") in the browser, once; the
    /// harness keeps the tokens in ~/.harness/credentials/oauth/.
    Login { name: String },
    /// Forget the sign-in of [mcp.<name>].
    Logout { name: String },
}

#[derive(Subcommand)]
enum MarketplaceCommand {
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
enum PluginCommand {
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
enum TaskCommand {
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
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| format!("{text:?} is not a role or `done`"))
}

/// `architect`, `developer`, `tester` or `security`.
fn parse_role(text: &str) -> Result<Role, String> {
    match parse_next(text) {
        Ok(NextStep::To(role)) if role != Role::Human => Ok(role),
        _ => Err(format!("{text:?} is not a role")),
    }
}

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
            harness_agents::mcp_oauth::logout(&dir, &name)?;
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
            harness_agents::mcp_remote::run_from_env().map_err(anyhow::Error::msg)
        }
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

/// `harness models`: the saved lists, after asking the agents again with
/// `--refresh`.
fn models(refresh: bool) -> Result<()> {
    use harness_core::models;
    let home = projects::harness_home().context("no home folder found")?;
    if refresh {
        let dir = credentials::default_dir().context("no home folder found")?;
        println!("Asking the agents with a saved login…");
        for (agent, result) in harness_agents::models::ask_all(&dir, &Default::default()) {
            match result {
                Ok(list) => {
                    models::save(&home, &list)
                        .with_context(|| format!("cannot save the models of {agent}"))?;
                }
                Err(error) => println!("{agent}: {error}"),
            }
        }
    }
    let mut any = false;
    for agent in harness_core::config::AGENTS {
        let Some(list) = models::load(&home, agent) else {
            continue;
        };
        any = true;
        println!("\n{agent}:");
        for model in &list.models {
            let default = if model.default { " (default)" } else { "" };
            let efforts = if model.efforts.is_empty() {
                String::new()
            } else {
                let levels: Vec<String> = model
                    .efforts
                    .iter()
                    .map(|e| {
                        if model.default_effort.as_ref() == Some(e) {
                            format!("[{e}]")
                        } else {
                            e.clone()
                        }
                    })
                    .collect();
                format!("  effort: {}", levels.join(" "))
            };
            let name = model
                .name
                .as_deref()
                .map(|n| format!("  {n}"))
                .unwrap_or_default();
            println!("  {}{default}{name}{efforts}", model.id);
        }
    }
    if !any {
        println!("No model lists yet: run `harness models --refresh`.");
    }
    Ok(())
}

/// The catalog of agents with what was found on this computer.
fn agents() {
    use harness_agents::catalog;
    let dir = credentials::default_dir();
    for status in catalog::check_all(dir.as_deref()) {
        let entry = &status.entry;
        let mark = if status.old() {
            "!"
        } else if status.installed() {
            "✓"
        } else {
            "○"
        };
        let runs = if entry.runs() {
            ""
        } else {
            "  (NOT IMPLEMENTED YET)"
        };
        println!("{mark} {:<20} {}{runs}", entry.name, entry.vendor);
        match &status.path {
            Some(path) => {
                let version = status.version.as_deref().unwrap_or("?");
                println!("    {} · version {version}", path.display());
            }
            None => println!("    not installed"),
        }
        if let Some(problem) = &status.problem {
            println!("    {problem}");
        }
        if let (true, Some(min)) = (status.old(), entry.min_version) {
            println!("    older than {min}, which the harness was checked with");
        }
        if let Some(host) = entry.inside {
            println!("    runs inside {host}");
        }
        match status.login {
            Some(true) => println!("    login saved"),
            Some(false) => println!("    no login: sign in on the Agents tab (harness tui)"),
            None => {}
        }
        println!("    plan: {}", entry.plan);
        if !entry.runs() {
            println!("    NOT IMPLEMENTED YET: the harness has no adapter for it, so roles");
            println!("    cannot use it. If you need it, ask the developer to implement it.");
        }
        if let Some((action, command)) = status.action() {
            let what = match action {
                catalog::Action::Install => "install",
                catalog::Action::Update => "update",
                catalog::Action::Remove => "remove",
            };
            match (command, status.needs_sudo(catalog::Action::Update)) {
                (Some(command), _) => println!("    {what}: {command}"),
                (None, Some(sudo)) => println!("    {what} (in a terminal): {sudo}"),
                (None, None) => println!("    {what}: see {}", entry.site),
            }
        }
        match (status.removal(), status.needs_sudo(catalog::Action::Remove)) {
            (Some(command), _) => println!("    remove: {command}"),
            (None, Some(sudo)) => println!("    remove (in a terminal): {sudo}"),
            (None, None) => {}
        }
    }
}

fn login(agent: &str) -> Result<()> {
    let dir = credentials::default_dir().context("no home folder found")?;
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

/// Runs `claude setup-token` (it opens the browser and prints a long-lived
/// token), then asks for that token and saves it.
fn login_claude(dir: &Path) -> Result<()> {
    println!("Starting `claude setup-token`; finish the login in your browser.");
    let status = std::process::Command::new(harness_platform::program::resolve("claude"))
        .arg("setup-token")
        .status()
        .context("cannot start `claude`; is Claude Code installed?")?;
    if !status.success() {
        bail!("`claude setup-token` failed ({status})");
    }
    println!();
    println!("Copy the token it printed above, paste it here and press Enter.");
    println!("It is not shown while you type: paste it once, then press Enter.");
    let token = read_hidden("Token: ")?;
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
    let line = read_line_hidden()?;
    Ok(Secret::new(line.trim()))
}

/// One line from the keyboard, not shown on the screen: `stty -echo` turns
/// the echo off while it is typed.
#[cfg(not(windows))]
fn read_line_hidden() -> io::Result<String> {
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
    read.map(|_| line)
}

/// One line from the keyboard, not shown on the screen. Windows has no
/// `stty`, so the keys are read one by one with the console's echo off.
/// Only presses count: Windows also reports each key's release, and a
/// pasted key would otherwise be taken twice.
#[cfg(windows)]
fn read_line_hidden() -> io::Result<String> {
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    use std::io::IsTerminal;

    if !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        return Ok(line);
    }
    crossterm::terminal::enable_raw_mode()?;
    let mut line = String::new();
    let result = loop {
        match read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Backspace => {
                    line.pop();
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                KeyCode::Char(c) => line.push(c),
                _ => {}
            },
            Ok(Event::Paste(text)) => line.push_str(&text),
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    result.map(|()| line)
}

/// Saves a secret for MCP servers in `~/.harness/credentials/secrets/<name>`,
/// readable only by Lisa, typed without showing it on the screen.
fn set_secret(name: &str) -> Result<()> {
    if !mcp::is_simple_name(name) {
        bail!("use lowercase letters, digits, '-' and '_' for the name, for example `context7`");
    }
    let dir = credentials::default_dir().context("no home folder found")?;
    println!("Paste the secret for {name:?} and press Enter.");
    println!("It is not shown while you type: paste it once, then press Enter.");
    let value = read_hidden("Secret: ")?;
    if value.expose().is_empty() {
        bail!("no secret given; nothing was saved");
    }
    let path = credentials::save_secret(&dir, name, &value)?;
    println!(
        "Saved {} characters to {} (only you can read it).",
        value.expose().chars().count(),
        path.display()
    );
    println!("Use it in harness.toml as \"secret:{name}\".");
    Ok(())
}

fn mcp_login(project: &Path, name: &str) -> Result<()> {
    use harness_agents::mcp_oauth;
    let repo = open_repo(project)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let url = mcp_oauth::oauth_url(&config, name).map_err(anyhow::Error::msg)?;
    let dir = credentials::default_dir().context("no home folder found")?;
    println!("Signing in to {name} ({url}).");
    let open = |address: &str| {
        println!("Opening the browser. If it does not open, visit:\n{address}");
        if let Err(why) = mcp_oauth::open_in_browser(address) {
            println!("({why})");
        }
        Ok(())
    };
    let tools = mcp_oauth::Tools {
        curl: "curl".into(),
        open: &open,
        browser_limit: mcp_oauth::BROWSER_LIMIT,
    };
    mcp_oauth::login(&dir, name, &url, &tools).map_err(anyhow::Error::msg)?;
    println!(
        "Signed in. The tokens are in {} (only you can read them).",
        mcp_oauth::path(&dir, name).display()
    );
    Ok(())
}

fn list_secrets() -> Result<()> {
    let dir = credentials::default_dir().context("no home folder found")?;
    let names = credentials::secret_names(&dir)?;
    if names.is_empty() {
        println!("No secrets saved. Add one with `harness secret set <name>`.");
    }
    for name in names {
        println!("{name}");
    }
    Ok(())
}

/// Runs `codex login` with its home in our credentials folder, so the login is
/// saved there and not in `~/.codex`.
fn login_codex(dir: &Path) -> Result<()> {
    let home = dir.join("codex");
    fs::create_dir_all(&home)?;
    println!("Starting `codex login`; finish the login in your browser.");
    let status = std::process::Command::new(harness_platform::program::resolve("codex"))
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
    harness_platform::private::restrict_file(&auth)?;
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
    harness_platform::private::restrict_dir(&home)?;
    println!("Starting `agy`. Sign in with Google, then type /quit to come back here.");
    let mut command = std::process::Command::new(harness_platform::program::resolve("agy"));
    command
        .env_clear()
        .envs(harness_platform::env::inherited_values());
    harness_platform::home::set_for(&mut command, &home);
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

fn new_task(project: &Path, task_id: &str, description: &str, allow_duplicate: bool) -> Result<()> {
    let repo = open_repo(project)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let create = if allow_duplicate {
        orchestrator::create_task_anyway
    } else {
        orchestrator::create_task
    };
    let (store, _) = create(&repo, task_id, description, config.max_rounds)?;
    println!(
        "Created {}. Next: harness run {task_id}",
        store.dir().display()
    );
    Ok(())
}

/// Waits for `work` (which runs agents) until it ends or Lisa presses Ctrl+C.
///
/// Each agent runs in its own process group, so the terminal's Ctrl+C reaches
/// only the harness. Here the harness stops waiting, which drops `work`, and
/// dropping it stops the running agent with everything it started (see
/// `harness_agents::process::run`).
async fn until_ctrl_c(work: impl std::future::Future<Output = Result<()>>) -> Result<()> {
    // `select!` waits for whichever finishes first and drops the other one.
    tokio::select! {
        result = work => result,
        _ = tokio::signal::ctrl_c() => {
            bail!("stopped by Ctrl+C; the agent and everything it started were stopped")
        }
    }
}

async fn run(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = Config::load(&harness_dir)?;
    let skills = Skills::load(&harness_dir, &config)?;
    let agent = build_team(&config, repo.root())?;
    let (store, mut state) = open_task(&repo, task_id)?;
    println!("Running {task_id} (round {})...", state.round);
    let stop = orchestrator::run_with_skills(&repo, &store, &mut state, &agent, &skills).await?;
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
        StopReason::TooLarge { role, files } => format!(
            "The {role:?} made files too big to commit (left uncommitted). Usually this is build \
             output: add it to .gitignore or delete it, commit or remove the rest, then run again:\n  {}",
            files
                .iter()
                .map(|(path, bytes)| format!("{path} ({} MB)", bytes.div_ceil(1024 * 1024)))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
        StopReason::AgentCommitted(role) => format!(
            "The {role:?} made a git commit itself. Check `git log` before running again."
        ),
        StopReason::GitConfigChanged(role) => format!(
            "The {role:?} changed the project's git settings (.git/config). The old settings \
             are back; its other changes are left uncommitted for you to check."
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

/// Statistics of one task, or of all tasks when `task_id` is `None`.
/// Printed, saved in `.harness/retros/<NNN>/` and committed. With `suggest`,
/// the `[retro]` agent then reads the history and proposes skill changes.
async fn retro(project: &Path, task_id: Option<&str>, suggest: bool) -> Result<()> {
    let repo = open_repo(project)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = match Config::load(&harness_dir) {
        Ok(config) => Some(config),
        Err(error) if !suggest => {
            eprintln!("Skills are not compared with harness.toml: {error}");
            None
        }
        Err(error) => return Err(error.into()),
    };
    // Check everything the agent needs before saving anything.
    let agent = match (&config, suggest) {
        (Some(config), true) => {
            retro_ops::check_clean(&repo)?;
            Some(retro_agent(config)?)
        }
        _ => None,
    };

    let (dir, stats) = retro_ops::save_stats(&repo, task_id, config.as_ref())?;
    print!("{}", stats.to_markdown());
    let number = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    println!("\nSaved to {}.", dir.display());

    let (Some(agent), Some(config)) = (agent, config) else {
        return Ok(());
    };
    println!("\nThe retro agent is reading the history...");
    let found = suggest::suggest(&repo, &dir, &stats, &config, &agent, suggest::ENGLISH)
        .await
        .with_context(|| {
            format!(
                "the agent's log is in {}",
                dir.join(suggest::AGENT_LOG).display()
            )
        })?;
    println!("\n{}", found.retro.trim_end());
    if found.proposals.proposals.is_empty() {
        println!("\nNo proposals.");
    } else {
        println!("\nProposals:");
        for proposal in &found.proposals.proposals {
            println!("  {}. {}", proposal.id, proposal.summary);
        }
        println!(
            "\nSee the changes with `harness retro show {number}`, \
             apply the ones you want with `harness retro apply {number} <ids>`."
        );
    }
    Ok(())
}

/// `.harness/retros/<NNN>` for `4`, `04` or `004`; it must exist.
fn retro_dir(repo: &Repo, number: &str) -> Result<PathBuf> {
    Ok(retro_ops::dir(repo, number)?)
}

fn retro_show(project: &Path, number: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let dir = retro_dir(&repo, number)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let found = suggest::load(&dir).with_context(|| {
        format!(
            "retrospective {number} has no proposals; they come from `harness retro <task> --suggest`"
        )
    })?;
    let config = Config::load(&harness_dir)?;
    let applied = Applied::load(&dir);
    let mut text = format!("{}\n", found.retro.trim_end());
    if found.proposals.proposals.is_empty() {
        text.push_str("\nNo proposals.\n");
    }
    for proposal in &found.proposals.proposals {
        let mark = if applied.applied.contains(&proposal.id) {
            " [applied]"
        } else {
            ""
        };
        text.push_str(&format!(
            "\n{}.{mark} {}",
            proposal.id,
            proposal.describe(&harness_dir, &config)
        ));
    }
    let _ = io::stdout().write_all(text.as_bytes());
    Ok(())
}

fn retro_apply(project: &Path, number: &str, ids: &[u32]) -> Result<()> {
    let repo = open_repo(project)?;
    let dir = retro_dir(&repo, number)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let found = suggest::load(&dir)?;
    let applied = Applied::load(&dir);
    // Say what will happen before it happens.
    for id in ids {
        if applied.applied.contains(id) {
            println!("Proposal {id} is already applied.");
        } else if let Some(proposal) = found.proposals.get(*id) {
            let file = match proposal.file_change(&harness_dir) {
                FileChange::New => " (new skill file)",
                FileChange::Changed { .. } => " (skill file changed)",
                FileChange::Unchanged => "",
            };
            println!("{id}. {}{file}", proposal.summary);
        }
    }
    if !retro_ops::apply(&repo, &dir, ids)?.is_empty() {
        println!("Applied and committed.");
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
        new_task(dir.path(), "task-001", "Build a parser", false).unwrap();

        assert!(repo.changed_files().unwrap().is_empty());
        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Architect));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retro_is_saved_and_committed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        assert!(retro(dir.path(), None, false).await.is_err()); // no tasks yet
        init(dir.path()).unwrap();
        new_task(dir.path(), "task-001", "Build a parser", false).unwrap();

        retro(dir.path(), Some("task-001"), false).await.unwrap();
        retro(dir.path(), None, false).await.unwrap();
        assert!(retro(dir.path(), Some("task-404"), false).await.is_err());

        let retros = dir.path().join(".harness/retros");
        assert!(retros.join("001/stats.md").exists());
        let json = fs::read_to_string(retros.join("002/stats.json")).unwrap();
        assert!(json.contains("\"scope\": \"all\""), "{json}");
        assert!(repo.changed_files().unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retro_proposals_are_shown_and_applied() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        init(dir.path()).unwrap();
        new_task(dir.path(), "task-001", "Build a parser", false).unwrap();
        retro(dir.path(), Some("task-001"), false).await.unwrap();
        assert!(retro_show(dir.path(), "1").is_err(), "no proposals yet");

        // What `retro --suggest` would have saved.
        let saved = dir.path().join(".harness/retros/001");
        fs::write(saved.join("retro.md"), "Went well.").unwrap();
        fs::write(
            saved.join("proposals.json"),
            r#"{"proposals": [{"id": 1, "summary": "Check empty input", "reason": "r",
                "skill": "empty-input",
                "content": "---\ndescription: Check empty input.\n---\nTest it.\n",
                "roles": [{"role": "developer", "list": "skills"}]}]}"#,
        )
        .unwrap();
        repo.commit_all("saved proposals").unwrap();

        retro_show(dir.path(), "001").unwrap();
        assert!(retro_show(dir.path(), "9").is_err());
        assert!(retro_apply(dir.path(), "1", &[2]).is_err());

        retro_apply(dir.path(), "1", &[1]).unwrap();
        assert!(repo.changed_files().unwrap().is_empty());
        let harness = dir.path().join(".harness");
        let config = Config::load(&harness).unwrap();
        assert_eq!(config.roles[&Role::Developer].skills, ["empty-input"]);
        assert!(harness.join("skills/empty-input.md").exists());
        assert_eq!(Applied::load(&saved).applied, [1]);
        // A second time it only says so.
        retro_apply(dir.path(), "1", &[1]).unwrap();
    }
}
