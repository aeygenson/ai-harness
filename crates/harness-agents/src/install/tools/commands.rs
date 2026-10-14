//! The commands that install the tools of [`super`] on Linux, macOS and
//! Windows, when the harness knows one.

use super::Tool;

impl Tool {
    /// The command that installs it on this system; `None` when the harness
    /// knows none (then [`Tool::site`] says how). On Linux it uses the
    /// system's package manager with `sudo`, so it asks for the password.
    pub fn install_command(self) -> Option<String> {
        harness_platform::program::on_each_system(
            self.linux_command(&|name| harness_platform::program::find(name).is_some()),
            self.mac_command(),
            self.windows_command(),
        )
    }

    /// The command on Linux; `has` says whether a program is in `PATH`.
    pub(super) fn linux_command(self, has: &dyn Fn(&str) -> bool) -> Option<String> {
        match self {
            Tool::Git => linux_package("git", has),
            Tool::Curl => linux_package("curl", has),
            Tool::Bubblewrap => linux_package("bubblewrap", has),
            // The harness's installer puts a new enough Node.js (with npm and
            // npx) into ~/.local; distributions often have an older one.
            Tool::Node | Tool::Npx => {
                Some(harness_platform::program::installer_command().to_string())
            }
            Tool::Zed => Some("curl -fsSL https://zed.dev/install.sh | sh".to_string()),
            Tool::GitHubCli => linux_package("gh", has),
        }
    }

    /// The command on macOS, with Homebrew (as the harness's installer does).
    pub(super) fn mac_command(self) -> Option<String> {
        let command = match self {
            Tool::Git => "brew install git",
            Tool::Node | Tool::Npx => "brew install node",
            Tool::Curl => "brew install curl",
            Tool::Zed => "brew install --cask zed",
            Tool::GitHubCli => "brew install gh",
            Tool::Bubblewrap => return None,
        };
        Some(command.to_string())
    }

    /// The command on Windows, with winget (as the harness's installer does).
    pub(super) fn windows_command(self) -> Option<String> {
        let id = match self {
            Tool::Git => "Git.Git",
            Tool::Node | Tool::Npx => "OpenJS.NodeJS.LTS",
            Tool::Curl => "cURL.cURL",
            Tool::Zed => "ZedIndustries.Zed",
            Tool::GitHubCli => "GitHub.cli",
            Tool::Bubblewrap => return None,
        };
        Some(format!("{WINGET_INSTALL} {id}"))
    }
}

/// winget's install command. Its questions about the terms of its sources
/// and packages are answered here, so it does not stop to ask.
const WINGET_INSTALL: &str =
    "winget install --exact --accept-source-agreements --accept-package-agreements --id";

/// The package managers of the common Linux distributions, with the start
/// of their install command (the same ones the harness's installer knows).
const LINUX_MANAGERS: [(&str, &str); 4] = [
    (
        "apt-get",
        "sudo apt-get update -q && sudo apt-get install -y",
    ),
    ("dnf", "sudo dnf install -y"),
    ("pacman", "sudo pacman -S --needed --noconfirm"),
    ("zypper", "sudo zypper --non-interactive install"),
];

/// Packages a package manager calls by another name: (manager, usual name,
/// its name there).
const RENAMED_PACKAGES: [(&str, &str, &str); 1] = [("pacman", "gh", "github-cli")];

/// The command that installs `package` with the first package manager found.
fn linux_package(package: &str, has: &dyn Fn(&str) -> bool) -> Option<String> {
    let (manager, command) = LINUX_MANAGERS.iter().find(|(manager, _)| has(manager))?;
    let package = RENAMED_PACKAGES
        .iter()
        .find(|(m, usual, _)| m == manager && *usual == package)
        .map_or(package, |(_, _, name)| name);
    Some(format!("{command} {package}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_github_cli_has_its_own_install_commands() {
        let apt = |name: &str| name == "apt-get";
        let pacman = |name: &str| name == "pacman";

        assert_eq!(
            Tool::GitHubCli.linux_command(&apt).as_deref(),
            Some("sudo apt-get update -q && sudo apt-get install -y gh")
        );
        // Arch Linux calls the package github-cli.
        assert_eq!(
            Tool::GitHubCli.linux_command(&pacman).as_deref(),
            Some("sudo pacman -S --needed --noconfirm github-cli")
        );
        assert_eq!(
            Tool::Git.linux_command(&pacman).as_deref(),
            Some("sudo pacman -S --needed --noconfirm git")
        );
        assert_eq!(
            Tool::GitHubCli.mac_command().as_deref(),
            Some("brew install gh")
        );
        assert!(Tool::GitHubCli
            .windows_command()
            .is_some_and(|c| c.ends_with("--id GitHub.cli")));
        assert!(!Tool::GitHubCli.from_installer());
        assert!(Tool::here().contains(&Tool::GitHubCli));
    }

    #[test]
    fn linux_uses_the_package_manager_it_finds() {
        let apt = |name: &str| name == "apt-get";
        let dnf = |name: &str| name == "dnf";
        let none = |_: &str| false;

        assert_eq!(
            Tool::Git.linux_command(&apt).as_deref(),
            Some("sudo apt-get update -q && sudo apt-get install -y git")
        );
        assert_eq!(
            Tool::Bubblewrap.linux_command(&dnf).as_deref(),
            Some("sudo dnf install -y bubblewrap")
        );
        // Without a known package manager only the site can help.
        assert_eq!(Tool::Curl.linux_command(&none), None);
    }

    #[test]
    fn npx_is_installed_with_node() {
        let none = |_: &str| false;

        // On Linux the harness's own installer brings Node.js with npx. The
        // test runs on every system, and the installer command is this system's.
        assert_eq!(
            Tool::Npx.linux_command(&none),
            Tool::Node.linux_command(&none)
        );
        assert_eq!(
            Tool::Npx.linux_command(&none).as_deref(),
            Some(harness_platform::program::installer_command())
        );
        assert_eq!(
            Tool::Npx.mac_command().as_deref(),
            Some("brew install node")
        );
        assert_eq!(Tool::Npx.windows_command(), Tool::Node.windows_command());
        assert_eq!(Tool::Npx.site(), Tool::Node.site());
    }

    #[test]
    fn windows_commands_do_not_stop_to_ask() {
        let command = Tool::Git.windows_command().unwrap();

        assert!(command.starts_with("winget install"), "{command}");
        assert!(command.contains("--accept-package-agreements"), "{command}");
        assert!(command.ends_with("--id Git.Git"), "{command}");
        // Bubblewrap is only for Linux.
        assert_eq!(Tool::Bubblewrap.windows_command(), None);
        assert_eq!(Tool::Bubblewrap.mac_command(), None);
    }
}
