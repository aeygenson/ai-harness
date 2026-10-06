//! The agents of the catalog: their programs, versions and the makers' install commands.

use harness_core::config::AgentKind;

/// A command for Linux and macOS, and one for Windows. `None`: the maker's
/// site says how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commands {
    /// The command for Linux and macOS.
    pub unix: Option<&'static str>,
    /// The command for Windows.
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
pub(super) fn for_npm(command: &str, prefix: Option<&str>) -> String {
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
    /// The name shown to Lisa, such as `Claude Code`.
    pub name: &'static str,
    /// The company that makes the agent, such as `Anthropic`.
    pub vendor: &'static str,
    /// The program, by name; the first one found counts.
    pub programs: &'static [&'static str],
    /// The agent is a model inside this other agent's program, as GLM
    /// runs inside Codex: installing that program is all it needs.
    pub inside: Option<&'static str>,
    /// The subscription or payment it works with.
    pub plan: &'static str,
    /// The lowest version the harness was checked with.
    pub min_version: Option<&'static str>,
    /// The maker's commands that install the agent.
    pub install: Commands,
    /// The maker's commands that update the agent to its newest version.
    pub update: Commands,
    /// The maker's own way to take it off, when deleting the program file
    /// is not enough (Claude Code keeps its versions next to it).
    pub remove: Commands,
    /// The maker's web page about the agent; shown when there is no command.
    pub site: &'static str,
}

impl Entry {
    /// The harness can run it: it is one of the agents of `harness.toml`.
    pub fn runs(&self) -> bool {
        self.id.parse::<AgentKind>().is_ok()
    }

    /// The npm package it is installed from (`npm install -g <package>@latest`).
    pub fn npm_package(&self) -> Option<&'static str> {
        let rest = self.install.unix?.strip_prefix("npm install -g ")?;
        Some(rest.strip_suffix("@latest").unwrap_or(rest))
    }
}

pub(super) const NPM_CODEX: &str = "npm install -g @openai/codex@latest";
pub(super) const NPM_DSH: &str = "npm install -g @deepseek-ai/dsh@latest";
pub(super) const CLAUDE_INSTALL: Commands = Commands {
    unix: Some("curl -fsSL https://claude.ai/install.sh | bash"),
    windows: Some("irm https://claude.ai/install.ps1 | iex"),
};
pub(super) const AGY_INSTALL: Commands = Commands {
    unix: Some("curl -fsSL https://antigravity.google/cli/install.sh | bash"),
    windows: Some("irm https://antigravity.google/cli/install.ps1 | iex"),
};
pub(super) const CURSOR_INSTALL: Commands = Commands {
    unix: Some("curl https://cursor.com/install -fsS | bash"),
    windows: Some("irm 'https://cursor.com/install?win32=true' | iex"),
};
pub(super) const GROK_INSTALL: Commands = Commands {
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
