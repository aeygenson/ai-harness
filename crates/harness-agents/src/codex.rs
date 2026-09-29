//! Runs a role with OpenAI Codex CLI (`codex exec`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.1):
//! - an empty environment plus a short whitelist (see `process`);
//! - `CODEX_HOME` points to `<project>/.harness/agents/codex/`, and
//!   `--ignore-user-config` / `--ignore-rules` skip every config and rules file,
//!   so the agent sees no MCP servers, plugins, hooks or profiles;
//! - login: `harness login codex` runs `codex login` into
//!   `~/.harness/credentials/codex/`. Only `auth.json` is copied into the
//!   project for the run and removed right after it;
//! - Codex's own sandbox (`workspace-write`): commands may write only inside
//!   the project. Codex has no per-folder rules like Claude Code, so the git
//!   check after the role is what enforces "Architect writes only docs/".
//!
//! DeepSeek (`agent = "codex+deepseek"`): the same Codex, with DeepSeek as its
//! model provider. DeepSeek speaks the Responses API that Codex uses. No
//! ChatGPT login is copied in for these roles.
//!
//! Secrets (the DeepSeek key, MCP server keys) are never in Codex's own
//! environment: commands the agent runs inherit it, and a live run showed that
//! `shell_environment_policy` does not keep them out. They go into files in a
//! private temporary folder outside the project, deleted after the role, and
//! Codex reads them through `harness mcp-exec` / `harness print-secret` (see
//! `launcher`).
//!
//! Plugins (`[plugins.<name>] agent = "codex"`): before a role, only that
//! role's plugin folders are copied into Codex's plugin cache
//! (`plugins/cache/harness/<name>/local/` in `CODEX_HOME`) and switched on with
//! `-c plugins={"<name>@harness"={enabled=true}}`. The `plugins` feature is on
//! only for roles with plugins, `hooks` only if a plugin may have them; remote
//! plugins and apps stay off. Codex's own bundled skills (one of them installs
//! skills from GitHub) are always off, and the `skills` folder older runs left
//! in `CODEX_HOME` is removed.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use harness_core::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::handoff::Role;
use harness_core::mcp::McpServer;
use harness_core::plugins::Plugin;

use crate::credentials::Secret;
use crate::launcher;
use crate::process::{self, failed};

/// Where the agent's own settings live inside the project (ignored by git).
pub const CONFIG_DIR: &str = ".harness/agents/codex";
const AUTH_FILE: &str = "auth.json";
/// Codex's plugin folder inside `CODEX_HOME`; cleared before and after each role.
const PLUGINS_DIR: &str = "plugins";
/// Codex's own skills folder inside `CODEX_HOME`. Older runs left the bundled
/// skills there; the harness gives skills through the prompt, so it is cleared too.
const SKILLS_DIR: &str = "skills";
/// The marketplace name the harness's plugins are filed under.
const MARKETPLACE: &str = "harness";
/// The version folder Codex prefers over any other.
const PLUGIN_VERSION: &str = "local";

/// Codex features that reach outside the project or add tools we did not
/// choose. Written as `-c features.<name>=false`: an unknown name is only a
/// warning, so an older or newer Codex still starts.
const DISABLED_FEATURES: &[&str] = &[
    "apps",
    "plugins",
    "remote_plugin",
    "hooks",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "in_app_browser",
    "image_generation",
    "skill_mcp_dependency_install",
    // Without this, a network problem makes Codex retry forever instead of
    // failing, and the role only ends at the time-out.
    "unbounded_connection_retries",
];

/// The variable in Lisa's shell that holds the DeepSeek API key.
pub const DEEPSEEK_KEY_ENV: &str = "DEEPSEEK_API_KEY";
/// The file with the DeepSeek key in the role's secrets folder.
const DEEPSEEK_KEY_FILE: &str = "deepseek-key";
/// DeepSeek's endpoint for Codex; Codex adds `responses` to it.
const DEEPSEEK_URL: &str = "https://api.deepseek.com/";
/// Used when harness.toml sets no model for the role.
pub const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-flash";

/// Who Codex talks to.
#[derive(Debug, Clone)]
enum Provider {
    /// OpenAI, with the ChatGPT login saved by `harness login codex`.
    ChatGpt,
    /// DeepSeek, with an API key.
    DeepSeek(Secret),
}

#[derive(Debug, Clone)]
pub struct Codex {
    program: PathBuf,
    provider: Provider,
    /// `~/.harness/credentials/codex`: holds the saved `auth.json`.
    auth_dir: PathBuf,
    models: HashMap<Role, String>,
    mcp: HashMap<Role, Vec<McpServer>>,
    plugins: HashMap<Role, Vec<Plugin>>,
    /// The `harness` program, which starts MCP servers and hands over keys.
    launcher: PathBuf,
    timeout: Duration,
}

impl Codex {
    pub fn new(auth_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("codex"),
            provider: Provider::ChatGpt,
            auth_dir: auth_dir.into(),
            models: HashMap::new(),
            mcp: HashMap::new(),
            plugins: HashMap::new(),
            launcher: PathBuf::from("harness"),
            timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Codex with DeepSeek models, paid by the API key.
    pub fn deepseek(api_key: Secret) -> Self {
        Self {
            provider: Provider::DeepSeek(api_key),
            ..Self::new(PathBuf::new())
        }
    }

    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    pub fn with_model(mut self, role: Role, model: impl Into<String>) -> Self {
        self.models.insert(role, model.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The MCP servers this role gets (from harness.toml).
    pub fn with_mcp_servers(mut self, role: Role, servers: Vec<McpServer>) -> Self {
        self.mcp.insert(role, servers);
        self
    }

    /// The plugins this role gets (from harness.toml).
    pub fn with_plugins(mut self, role: Role, plugins: Vec<Plugin>) -> Self {
        self.plugins.insert(role, plugins);
        self
    }

    /// The `harness` program to use for `mcp-exec` and `print-secret`; the
    /// command line passes its own path.
    pub fn with_launcher(mut self, launcher: impl Into<PathBuf>) -> Self {
        self.launcher = launcher.into();
        self
    }

    /// Builds the full command for one job, without running it. `secrets_dir`
    /// is the private folder `write_secrets` filled.
    pub fn command(&self, job: &RoleJob, secrets_dir: &Path) -> Command {
        let mut command = process::base_command(&self.program, &job.project_dir);
        command
            .env("CODEX_HOME", job.project_dir.join(CONFIG_DIR))
            .arg("exec")
            // One JSON event per line; the prompt comes on standard input ("-").
            .arg("--json")
            .arg("--ephemeral")
            .args(["--ignore-user-config", "--ignore-rules"])
            .arg("--skip-git-repo-check")
            .args(["--cd".as_ref(), job.project_dir.as_os_str()])
            .args(["--sandbox", "workspace-write"])
            .args(["-c", "approval_policy=\"never\""])
            .args([
                "-c",
                &format!(
                    "sandbox_workspace_write.network_access={}",
                    needs_network(job.role)
                ),
            ]);
        let plugins = self.role_plugins(job.role);
        let hooks = plugins.iter().any(|p| p.allow_hooks);
        for feature in DISABLED_FEATURES {
            let on = match *feature {
                "plugins" => !plugins.is_empty(),
                "hooks" => hooks,
                _ => false,
            };
            command.args(["-c", &format!("features.{feature}={on}")]);
        }
        // Bundled skills such as `skill-installer` are not the project's choice.
        command.args(["-c", "skills.bundled.enabled=false"]);
        if !plugins.is_empty() {
            let enabled: Vec<String> = plugins
                .iter()
                .map(|p| format!("\"{}@{MARKETPLACE}\"={{enabled=true}}", p.name))
                .collect();
            command.args(["-c", &format!("plugins={{{}}}", enabled.join(","))]);
        }
        if let Provider::DeepSeek(_) = &self.provider {
            let key_file = secrets_dir.join(DEEPSEEK_KEY_FILE);
            for setting in [
                "model_provider=\"deepseek\"".to_string(),
                "model_providers.deepseek.name=\"DeepSeek\"".to_string(),
                format!("model_providers.deepseek.base_url=\"{DEEPSEEK_URL}\""),
                "model_providers.deepseek.wire_api=\"responses\"".to_string(),
                // Codex runs this command to get the key.
                format!(
                    "model_providers.deepseek.auth={{command={},args={}}}",
                    toml_value(self.launcher.to_string_lossy().as_ref()),
                    toml_value(vec![
                        launcher::PRINT_SECRET.to_string(),
                        key_file.to_string_lossy().into_owned(),
                    ])
                ),
                "forced_login_method=\"api\"".to_string(),
                // DeepSeek has no web search tool for Codex.
                "web_search=\"disabled\"".to_string(),
            ] {
                command.args(["-c", &setting]);
            }
        }
        self.add_mcp_servers(&mut command, job.role, secrets_dir);
        if let Some(model) = self.model(job.role) {
            command.args(["--model", model]);
        }
        command.arg("-");
        command
    }

    /// Each server as `-c mcp_servers.<name>.*` settings that start it through
    /// `harness mcp-exec <file>`; only the file knows the server's secrets.
    fn add_mcp_servers(&self, command: &mut Command, role: Role, secrets_dir: &Path) {
        for server in self.servers(role) {
            let key = format!("mcp_servers.{}", server.name);
            let spec = secrets_dir.join(format!("{}.json", server.name));
            let args = vec![
                launcher::MCP_EXEC.to_string(),
                spec.to_string_lossy().into_owned(),
            ];
            for setting in [
                format!(
                    "{key}.command={}",
                    toml_value(self.launcher.to_string_lossy().as_ref())
                ),
                format!("{key}.args={}", toml_value(args)),
            ] {
                command.args(["-c", &setting]);
            }
        }
    }

    fn servers(&self, role: Role) -> &[McpServer] {
        self.mcp.get(&role).map_or(&[], Vec::as_slice)
    }

    fn role_plugins(&self, role: Role) -> &[Plugin] {
        self.plugins.get(&role).map_or(&[], Vec::as_slice)
    }

    /// Leaves in Codex's plugin cache exactly this role's plugins.
    fn put_plugins(&self, job: &RoleJob) -> io::Result<()> {
        remove_extras(job);
        let cache = job
            .project_dir
            .join(CONFIG_DIR)
            .join(PLUGINS_DIR)
            .join("cache")
            .join(MARKETPLACE);
        for plugin in self.role_plugins(job.role) {
            copy_dir(&plugin.path, &cache.join(&plugin.name).join(PLUGIN_VERSION))?;
        }
        Ok(())
    }

    /// A private temporary folder outside the project with the role's secrets:
    /// one file per MCP server and the DeepSeek key. Deleted when dropped.
    fn write_secrets(&self, role: Role) -> io::Result<tempfile::TempDir> {
        let dir = tempfile::Builder::new()
            .prefix("harness-codex-")
            .tempdir()?;
        for server in self.servers(role) {
            launcher::write_server(dir.path(), server)?;
        }
        if let Provider::DeepSeek(key) = &self.provider {
            launcher::write_secret(dir.path(), DEEPSEEK_KEY_FILE, key)?;
        }
        Ok(dir)
    }

    /// The role's model from harness.toml; DeepSeek needs one, so it has a default.
    fn model(&self, role: Role) -> Option<&str> {
        match (self.models.get(&role), &self.provider) {
            (Some(model), _) => Some(model),
            (None, Provider::DeepSeek(_)) => Some(DEEPSEEK_DEFAULT_MODEL),
            (None, Provider::ChatGpt) => None,
        }
    }

    /// Copies the saved login into the project's config folder for this run.
    fn put_auth(&self, job: &RoleJob) -> io::Result<()> {
        let dir = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(&dir)?;
        let saved = fs::read(self.auth_dir.join(AUTH_FILE)).map_err(|e| {
            io::Error::new(
                e.kind(),
                "no Codex login saved; run `harness login codex` first",
            )
        })?;
        write_private(&dir.join(AUTH_FILE), &saved)
    }

    /// Codex may refresh its login during the run. Keep the newest one, then
    /// remove the copy so other roles' agents cannot read it.
    fn take_auth_back(&self, job: &RoleJob) {
        let copy = job.project_dir.join(CONFIG_DIR).join(AUTH_FILE);
        if let Ok(bytes) = fs::read(&copy) {
            let still_json = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
            let saved = fs::read(self.auth_dir.join(AUTH_FILE)).ok();
            if still_json && saved.as_deref() != Some(bytes.as_slice()) {
                let _ = write_private(&self.auth_dir.join(AUTH_FILE), &bytes);
            }
        }
        let _ = fs::remove_file(copy);
    }

    fn header(&self, job: &RoleJob) -> String {
        let model = self.model(job.role).unwrap_or("default");
        let agent = match self.provider {
            Provider::ChatGpt => "codex",
            Provider::DeepSeek(_) => "codex+deepseek",
        };
        format!(
            "agent: {agent}, model: {model}, role: {:?}, round: {}\n",
            job.role, job.round
        )
    }
}

impl AgentRunner for Codex {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let mut log = self.header(job);
        let chatgpt = matches!(self.provider, Provider::ChatGpt);
        let prepared = if chatgpt {
            self.put_auth(job)
        } else {
            fs::create_dir_all(job.project_dir.join(CONFIG_DIR))
        };
        let prepared = prepared.and_then(|()| self.put_plugins(job));
        if let Err(e) = prepared {
            if chatgpt {
                self.take_auth_back(job);
            }
            remove_extras(job);
            return failed(log, format!("cannot prepare {CONFIG_DIR}: {e}"));
        }
        let secrets = match self.write_secrets(job.role) {
            Ok(dir) => dir,
            Err(e) => {
                if chatgpt {
                    self.take_auth_back(job);
                }
                remove_extras(job);
                return failed(log, format!("cannot write the role's secrets: {e}"));
            }
        };
        let command = self.command(job, secrets.path());
        let result = process::run(command, &job.prompt, self.timeout).await;
        drop(secrets);
        if chatgpt {
            self.take_auth_back(job);
        }
        remove_extras(job);
        let deepseek_key = match &self.provider {
            Provider::DeepSeek(key) => Some(key.expose()),
            Provider::ChatGpt => None,
        };
        let result = process::hide_secrets(
            result,
            self.servers(job.role)
                .iter()
                .flat_map(|server| server.env.values().map(|v| v.expose()))
                .chain(deepseek_key),
        );
        let finished = match result {
            Ok(finished) => finished,
            Err(message) => return failed(log, message),
        };
        process::append_output(&mut log, &finished);

        let end = TurnEnd::find(&finished.stdout);
        let success = finished.status.success() && end == Some(TurnEnd::Completed);
        let message = match &end {
            _ if success => String::new(),
            Some(TurnEnd::Failed(message)) => message.clone(),
            _ => format!("codex exited ({}) without finishing", finished.status),
        };
        let usage_limit_reached =
            !success && process::looks_like_usage_limit(&format!("{message}\n{}", finished.stderr));
        AgentOutcome {
            success: success && !usage_limit_reached,
            usage_limit_reached,
            log,
            message,
        }
    }
}

/// Removes Codex's plugin and skills folders, so the next role starts
/// without plugins or skills left by an earlier run.
fn remove_extras(job: &RoleJob) {
    let home = job.project_dir.join(CONFIG_DIR);
    for dir in [PLUGINS_DIR, SKILLS_DIR] {
        let _ = fs::remove_dir_all(home.join(dir));
    }
}

/// Copies a plugin folder. Symbolic links are refused: one could point
/// outside the project.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(io::Error::other(format!(
                "{} is a link or a special file; plugins may hold only files and folders",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

/// A string or a list of strings in TOML syntax. JSON writes them the same way.
fn toml_value(value: impl Into<serde_json::Value>) -> String {
    value.into().to_string()
}

/// Roles that run commands such as `cargo test` or `cargo audit` may need to
/// download crates. The Architect only reads and writes documents.
fn needs_network(role: Role) -> bool {
    matches!(role, Role::Developer | Role::Tester | Role::Security)
}

/// How the turn ended, from `--json` output: `{"type":"turn.completed",...}` or
/// `{"type":"turn.failed","error":{"message":"..."}}`.
#[derive(Debug, PartialEq, Eq)]
enum TurnEnd {
    Completed,
    Failed(String),
}

impl TurnEnd {
    fn find(stdout: &str) -> Option<Self> {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|event| match event["type"].as_str() {
                Some("turn.completed") => Some(TurnEnd::Completed),
                Some("turn.failed") => Some(TurnEnd::Failed(
                    event["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                        .to_string(),
                )),
                _ => None,
            })
            .next_back()
    }
}

/// `auth.json` is a password too: readable only by its owner.
#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(role: Role) -> RoleJob {
        RoleJob {
            task_id: "task-001".into(),
            round: 1,
            role,
            project_dir: PathBuf::from("/work/app"),
            prompt: "do it".into(),
            output_dir: PathBuf::from("/work/app/.harness/runs/task-001/inbox"),
        }
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn codex_runs_isolated_in_the_project_sandbox() {
        let command =
            Codex::new("/creds/codex").command(&job(Role::Developer), Path::new("/secrets"));
        let args = args(&command);
        for flag in [
            "exec",
            "--json",
            "--ignore-user-config",
            "--ignore-rules",
            "workspace-write",
            "approval_policy=\"never\"",
            "features.plugins=false",
            "features.hooks=false",
            "skills.bundled.enabled=false",
        ] {
            assert!(args.contains(&flag.to_string()), "missing {flag}: {args:?}");
        }
        assert_eq!(args.last().map(String::as_str), Some("-"));
        let home = command
            .get_envs()
            .find(|(k, _)| *k == "CODEX_HOME")
            .and_then(|(_, v)| v)
            .unwrap();
        assert_eq!(home, "/work/app/.harness/agents/codex");
    }

    #[test]
    fn mcp_servers_go_to_their_role_and_secrets_stay_out_of_the_arguments() {
        use harness_core::mcp::McpServer;
        use std::collections::BTreeMap;

        let server = McpServer {
            name: "context7".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@upstash/context7-mcp".into()],
            env: BTreeMap::from([("CONTEXT7_API_KEY".into(), Secret::new("ctx-secret"))]),
        };
        let codex = Codex::new("/creds")
            .with_launcher("/bin/harness")
            .with_mcp_servers(Role::Developer, vec![server]);
        let command = codex.command(&job(Role::Developer), Path::new("/secrets"));
        let args = args(&command);
        for setting in [
            r#"mcp_servers.context7.command="/bin/harness""#,
            r#"mcp_servers.context7.args=["mcp-exec","/secrets/context7.json"]"#,
        ] {
            assert!(
                args.contains(&setting.to_string()),
                "missing {setting}: {args:?}"
            );
        }
        // Neither the arguments nor Codex's own environment hold the secret:
        // the agent's commands inherit that environment.
        assert!(!args.iter().any(|a| a.contains("ctx-secret")));
        assert!(!command.get_envs().any(|(k, _)| k == "CONTEXT7_API_KEY"));

        // The file mcp-exec reads is written into the role's secrets folder.
        let dir = codex.write_secrets(Role::Developer).unwrap();
        let spec = fs::read_to_string(dir.path().join("context7.json")).unwrap();
        assert!(spec.contains("ctx-secret"), "{spec}");
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(!path.exists());

        let tester = args_of(&codex, Role::Tester);
        assert!(!tester.iter().any(|a| a.contains("mcp_servers")));
    }

    fn args_of(codex: &Codex, role: Role) -> Vec<String> {
        args(&codex.command(&job(role), Path::new("/secrets")))
    }

    #[test]
    fn a_role_gets_only_its_plugins_and_hooks_stay_off_unless_allowed() {
        let plugin = |name: &str, allow_hooks| Plugin {
            name: name.into(),
            path: PathBuf::from(format!("/work/app/.harness/plugins/{name}")),
            allow_hooks,
        };
        let codex = Codex::new("/creds/codex")
            .with_plugins(Role::Security, vec![plugin("review", false)])
            .with_plugins(
                Role::Developer,
                vec![plugin("fmt", true), plugin("lint", false)],
            );

        let security = args_of(&codex, Role::Security);
        assert!(security.contains(&"features.plugins=true".to_string()));
        assert!(security.contains(&"features.hooks=false".to_string()));
        assert!(security.contains(&r#"plugins={"review@harness"={enabled=true}}"#.to_string()));
        // Plugins never bring remote plugins or ChatGPT apps.
        assert!(security.contains(&"features.remote_plugin=false".to_string()));
        assert!(security.contains(&"features.apps=false".to_string()));

        let developer = args_of(&codex, Role::Developer);
        assert!(developer.contains(&"features.hooks=true".to_string()));
        assert!(developer.contains(
            &r#"plugins={"fmt@harness"={enabled=true},"lint@harness"={enabled=true}}"#.to_string()
        ));

        let tester = args_of(&codex, Role::Tester);
        assert!(tester.contains(&"features.plugins=false".to_string()));
        assert!(!tester.iter().any(|a| a.starts_with("plugins=")));
    }

    #[test]
    fn plugin_folders_are_copied_for_the_role_and_removed_after() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join(".harness/plugins/review");
        fs::create_dir_all(source.join(".codex-plugin")).unwrap();
        fs::write(source.join(".codex-plugin/plugin.json"), "{}").unwrap();
        fs::create_dir_all(source.join("skills/audit")).unwrap();
        fs::write(source.join("skills/audit/SKILL.md"), "audit").unwrap();
        let codex = Codex::new("/creds/codex").with_plugins(
            Role::Security,
            vec![Plugin {
                name: "review".into(),
                path: source.clone(),
                allow_hooks: false,
            }],
        );
        let job = RoleJob {
            project_dir: project.path().to_path_buf(),
            ..job(Role::Security)
        };
        let plugins = project.path().join(CONFIG_DIR).join(PLUGINS_DIR);
        // Something left from an earlier role or run is cleared.
        fs::create_dir_all(plugins.join("cache/harness/old/local")).unwrap();
        let skills = project.path().join(CONFIG_DIR).join(SKILLS_DIR);
        fs::create_dir_all(skills.join(".system/skill-installer")).unwrap();

        codex.put_plugins(&job).unwrap();
        let copied = plugins.join("cache/harness/review/local");
        assert_eq!(
            fs::read_to_string(copied.join("skills/audit/SKILL.md")).unwrap(),
            "audit"
        );
        assert!(!plugins.join("cache/harness/old").exists());
        assert!(!skills.exists());

        remove_extras(&job);
        assert!(!plugins.exists());
        assert!(source.join("skills/audit/SKILL.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_plugin_with_a_link_is_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("plugin");
        fs::create_dir_all(&source).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", source.join("passwd")).unwrap();
        let error = copy_dir(&source, &dir.path().join("copy")).unwrap_err();
        assert!(error.to_string().contains("link"), "{error}");
    }

    #[test]
    fn only_command_running_roles_get_the_network() {
        let codex = Codex::new("/creds/codex");
        let architect = args(&codex.command(&job(Role::Architect), Path::new("/secrets")));
        assert!(architect.contains(&"sandbox_workspace_write.network_access=false".to_string()));
        let developer = args(&codex.command(&job(Role::Developer), Path::new("/secrets")));
        assert!(developer.contains(&"sandbox_workspace_write.network_access=true".to_string()));
    }

    #[test]
    fn no_api_keys_in_the_environment() {
        let command = Codex::new("/creds/codex").command(&job(Role::Tester), Path::new("/secrets"));
        for (name, _) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                process::INHERITED_ENV.contains(&name.as_ref()) || name == "CODEX_HOME",
                "{name} should not be passed"
            );
        }
    }

    #[test]
    fn the_model_is_set_per_role() {
        let codex = Codex::new("/creds").with_model(Role::Tester, "gpt-5-codex");
        assert!(
            args(&codex.command(&job(Role::Tester), Path::new("/secrets")))
                .contains(&"gpt-5-codex".to_string())
        );
        assert!(
            !args(&codex.command(&job(Role::Architect), Path::new("/secrets")))
                .contains(&"--model".to_string())
        );
    }

    #[test]
    fn deepseek_gets_its_key_and_provider_but_no_chatgpt_login() {
        let codex =
            Codex::deepseek(Secret::new("sk-test-0123456789")).with_launcher("/bin/harness");
        let command = codex.command(&job(Role::Tester), Path::new("/secrets"));
        let args = args(&command);
        for setting in [
            "model_provider=\"deepseek\"",
            "model_providers.deepseek.wire_api=\"responses\"",
            r#"model_providers.deepseek.auth={command="/bin/harness",args=["print-secret","/secrets/deepseek-key"]}"#,
            DEEPSEEK_DEFAULT_MODEL,
        ] {
            assert!(
                args.contains(&setting.to_string()),
                "missing {setting}: {args:?}"
            );
        }
        // The key is neither in the arguments nor in Codex's environment.
        assert!(!args.iter().any(|a| a.contains("sk-test")));
        assert!(!command.get_envs().any(|(k, _)| k == DEEPSEEK_KEY_ENV));
        assert!(format!("{codex:?}").contains("Secret(***)"));
        let dir = codex.write_secrets(Role::Tester).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("deepseek-key")).unwrap(),
            "sk-test-0123456789"
        );
    }

    #[test]
    fn chatgpt_codex_gets_no_deepseek_key() {
        let command = Codex::new("/creds").command(&job(Role::Tester), Path::new("/secrets"));
        assert!(!command.get_envs().any(|(k, _)| k == DEEPSEEK_KEY_ENV));
        assert!(!args(&command).contains(&"model_provider=\"deepseek\"".to_string()));
    }

    #[test]
    fn the_last_turn_event_decides() {
        let ok = "{\"type\":\"thread.started\"}\n{\"type\":\"turn.completed\",\"usage\":{}}";
        assert_eq!(TurnEnd::find(ok), Some(TurnEnd::Completed));
        let bad = "{\"type\":\"error\",\"message\":\"Reconnecting...\"}\n\
                   {\"type\":\"turn.failed\",\"error\":{\"message\":\"You've hit your usage limit\"}}";
        assert_eq!(
            TurnEnd::find(bad),
            Some(TurnEnd::Failed("You've hit your usage limit".into()))
        );
        assert_eq!(TurnEnd::find("WARNING: text\n"), None);
    }
}
