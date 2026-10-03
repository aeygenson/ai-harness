//! Choosing a folder. On a desktop the system's own dialog opens (on Linux
//! KDE's `kdialog` or `zenity`, on macOS the Finder's «Choose folder», on
//! Windows the «Browse for folder» window);
//! without one (over SSH, or neither installed) a folder browser inside the
//! TUI does the same job.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ratatui::layout::{Constraint, Flex, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;
use ratatui::Frame;

use crate::i18n::I18n;
use crate::tasks::draw_list;
use crate::theme;
use crate::ui::{self, buttons, panel, ButtonId, Hits, ListId, Target};

/// What the system dialog answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    Chosen(PathBuf),
    Cancelled,
    /// There is no dialog program or no desktop: use the browser.
    Unavailable,
}

/// The command for the system's folder dialog: `desktop` says whether there
/// is a graphical session, `installed` whether a program can be found.
fn native_command(
    title: &str,
    start: &Path,
    desktop: bool,
    installed: impl Fn(&str) -> bool,
) -> Option<Command> {
    if !desktop {
        None
    } else if cfg!(target_os = "macos") {
        Some(mac_command(title, start))
    } else if cfg!(windows) {
        Some(windows_command(title, start))
    } else {
        linux_command(title, start, installed)
    }
}

/// macOS: AppleScript's «choose folder», run by `osascript`, which comes
/// with every Mac. The title and the folder are passed as arguments, never
/// written into the script, so quotes in them cannot change it. Cancel ends
/// it with an error (exit code 1).
fn mac_command(title: &str, start: &Path) -> Command {
    let mut command = Command::new("osascript");
    command
        .args(["-e", "on run argv"])
        .args([
            "-e",
            "POSIX path of (choose folder with prompt (item 1 of argv) \
             default location (POSIX file (item 2 of argv)))",
        ])
        .args(["-e", "end run"])
        .arg(title)
        .arg(start);
    command
}

/// The PowerShell script for Windows' folder window: it prints the chosen
/// folder, or ends with 1 on Cancel.
const WINDOWS_SCRIPT: &str = "\
Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = $env:HARNESS_PICK_TITLE
$dialog.SelectedPath = $env:HARNESS_PICK_START
if ($dialog.ShowDialog() -eq 'OK') { $dialog.SelectedPath } else { exit 1 }
";

/// Windows: the folder window of Windows Forms, run by PowerShell, which
/// comes with every Windows. The title and the folder are passed in
/// variables, never written into the script, so quotes cannot change it.
fn windows_command(title: &str, start: &Path) -> Command {
    let mut command = Command::new(harness_platform::program::resolve("powershell"));
    command
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command"])
        .arg(WINDOWS_SCRIPT)
        .env("HARNESS_PICK_TITLE", title)
        .env("HARNESS_PICK_START", start);
    command
}

/// Linux: KDE's dialog, or GNOME's `zenity`.
fn linux_command(title: &str, start: &Path, installed: impl Fn(&str) -> bool) -> Option<Command> {
    if installed("kdialog") {
        let mut command = Command::new("kdialog");
        command
            .arg("--title")
            .arg(title)
            .arg("--getexistingdirectory")
            .arg(start);
        Some(command)
    } else if installed("zenity") {
        let mut command = Command::new("zenity");
        command
            .arg("--file-selection")
            .arg("--directory")
            .arg(format!("--title={title}"))
            .arg(format!("--filename={}/", start.display()));
        Some(command)
    } else {
        None
    }
}

/// Opens the system's folder dialog and waits for it.
pub fn native_folder(title: &str, start: &Path) -> Native {
    let desktop = if cfg!(any(target_os = "macos", windows)) {
        // A Mac or Windows always has its desktop, unless the TUI runs over SSH.
        std::env::var_os("SSH_CONNECTION").is_none()
    } else {
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
    };
    let installed = |name: &str| {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
    };
    match native_command(title, start, desktop, installed) {
        Some(command) => run_dialog(command),
        None => Native::Unavailable,
    }
}

/// Runs a dialog that prints the chosen folder, or exits with 1 on Cancel.
fn run_dialog(mut command: Command) -> Native {
    match command.output() {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // macOS ends a folder with `/`; the root folder keeps it.
            let path = match path.strip_suffix('/') {
                Some(inner) if !inner.is_empty() => inner.to_string(),
                _ => path,
            };
            if path.is_empty() {
                Native::Cancelled
            } else {
                Native::Chosen(PathBuf::from(path))
            }
        }
        // Cancel (exit code 1) closes the dialog without a folder.
        Ok(_) => Native::Cancelled,
        Err(_) => Native::Unavailable,
    }
}

/// A folder browser inside the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    pub title: String,
    pub dir: PathBuf,
    /// The subfolders of `dir`, sorted.
    pub folders: Vec<String>,
    pub selected: usize,
    /// A folder in the list was picked (clicked or moved to): «Choose» takes
    /// it. Otherwise «Choose» takes the folder that is open.
    pub marked: bool,
    /// Show folders whose names start with a dot.
    pub hidden: bool,
    /// The name of a new folder being typed.
    pub naming: Option<String>,
    pub error: Option<String>,
}

impl Browser {
    pub fn new(title: &str, start: &Path) -> Self {
        let mut browser = Self {
            title: title.to_string(),
            dir: start.to_path_buf(),
            folders: Vec::new(),
            selected: 0,
            marked: false,
            hidden: false,
            naming: None,
            error: None,
        };
        browser.read();
        browser
    }

    fn read(&mut self) {
        self.error = None;
        self.marked = false;
        self.folders = match fs::read_dir(&self.dir) {
            Ok(entries) => {
                let mut folders: Vec<String> = entries
                    .filter_map(Result::ok)
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| self.hidden || !name.starts_with('.'))
                    .collect();
                folders.sort_by_key(|name| name.to_lowercase());
                folders
            }
            Err(error) => {
                self.error = Some(error.to_string());
                Vec::new()
            }
        };
        self.selected = self.selected.min(self.folders.len().saturating_sub(1));
    }

    pub fn open_selected(&mut self) {
        if let Some(name) = self.folders.get(self.selected) {
            self.dir = self.dir.join(name);
            self.selected = 0;
            self.read();
        }
    }

    pub fn up(&mut self) {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            return;
        };
        let child = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.dir = parent;
        self.read();
        // Keep the folder we came from selected.
        if let Some(index) = child.and_then(|c| self.folders.iter().position(|f| *f == c)) {
            self.selected = index;
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.hidden = !self.hidden;
        self.read();
    }

    /// Picks row `index` of the list.
    pub fn mark(&mut self, index: usize) {
        if index < self.folders.len() {
            self.selected = index;
            self.marked = true;
        }
    }

    /// The folder «Choose» takes.
    pub fn chosen(&self) -> PathBuf {
        match self.folders.get(self.selected) {
            Some(name) if self.marked => self.dir.join(name),
            _ => self.dir.clone(),
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.marked = !self.folders.is_empty();
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.folders.len().saturating_sub(1));
    }

    /// Creates the folder typed in `naming` and goes into it.
    pub fn create(&mut self, tr: &I18n) {
        let Some(name) = self.naming.take() else {
            return;
        };
        let name = name.trim();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            self.error = Some(tr.t("picker.bad_name").to_string());
            return;
        }
        let path = self.dir.join(name);
        match fs::create_dir(&path) {
            Ok(()) => {
                self.dir = path;
                self.selected = 0;
                self.read();
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn draw(&self, frame: &mut Frame, hits: &mut Hits, tr: &I18n) {
        let area = frame.area();
        let [area] = Layout::vertical([Constraint::Percentage(80)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::horizontal([Constraint::Max(90)])
            .flex(Flex::Center)
            .areas(area);
        crate::ui::clear(frame, area);
        let block = panel(&format!(" {} ", self.title), true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        // Clicks anywhere in the window stay in it.
        hits.add(area, Target::Field(usize::MAX));

        let [path, list, message, hint, bar] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        let label = tr.t("picker.folder");
        let room = usize::from(path.width).saturating_sub(label.chars().count());
        frame.render_widget(
            Line::from(vec![
                Span::raw(label.to_string()),
                Span::styled(
                    keep_end(&self.chosen().display().to_string(), room),
                    theme::accent(),
                ),
            ]),
            path,
        );
        let items: Vec<ListItem> = self
            .folders
            .iter()
            .map(|name| ListItem::new(format!("{name}/")))
            .collect();
        let title = if self.folders.is_empty() {
            tr.t("picker.no_subfolders")
        } else {
            tr.t("picker.subfolders")
        };
        draw_list(
            frame,
            hits,
            list,
            ListId::Folders,
            title,
            items,
            self.selected,
            true,
        );
        match (&self.naming, &self.error) {
            (Some(name), _) => frame.render_widget(
                Line::from(vec![
                    Span::raw(tr.t("picker.new_name").to_string()),
                    Span::styled(format!(" {name}▏"), ui::input()),
                ]),
                message,
            ),
            (None, Some(error)) => {
                frame.render_widget(Span::styled(error.clone(), theme::bad()), message)
            }
            (None, None) => {}
        }
        let hint_text = if self.naming.is_some() {
            tr.t("picker.naming_hint")
        } else {
            tr.t("picker.hint")
        };
        frame.render_widget(Span::styled(hint_text.to_string(), theme::dim()), hint);
        let hidden = if self.hidden {
            tr.t("picker.hide_hidden")
        } else {
            tr.t("picker.show_hidden")
        };
        buttons(
            frame,
            bar,
            hits,
            &[
                (tr.t("picker.choose"), ButtonId::Choose, true),
                (tr.t("picker.up"), ButtonId::Up, self.dir.parent().is_some()),
                (tr.t("picker.new_folder"), ButtonId::NewFolder, true),
                (hidden, ButtonId::ToggleHidden, true),
                (tr.t("form.cancel"), ButtonId::Cancel, true),
            ],
        );
    }
}

/// A long path cut from the left, so its end (the folder itself) stays visible.
pub(crate) fn keep_end(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count + 1 - width.max(1)).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_dialog_is_used_when_there_is_one() {
        let start = Path::new("/home/me/code");
        let args = |command: Option<Command>| {
            command.map(|c| {
                let mut all = vec![c.get_program().to_string_lossy().into_owned()];
                all.extend(c.get_args().map(|a| a.to_string_lossy().into_owned()));
                all
            })
        };
        assert_eq!(
            args(linux_command("Pick", start, |_| true)).unwrap(),
            [
                "kdialog",
                "--title",
                "Pick",
                "--getexistingdirectory",
                "/home/me/code"
            ]
        );
        assert_eq!(
            args(linux_command("Pick", start, |n| n == "zenity")).unwrap()[0],
            "zenity"
        );
        assert!(linux_command("Pick", start, |_| false).is_none());
        assert!(native_command("Pick", start, false, |_| true).is_none());

        // On a Mac: the title and the folder are arguments, not script text.
        let mac = args(Some(mac_command("Pick \"x\"", start))).unwrap();
        assert_eq!(mac[0], "osascript");
        assert_eq!(mac[mac.len() - 2..], ["Pick \"x\"", "/home/me/code"]);
        assert!(mac
            .iter()
            .all(|a| !a.contains("/home/me") || a == "/home/me/code"));
        let on_this_system = args(native_command("Pick", start, true, |_| true)).unwrap();
        let expected = if cfg!(target_os = "macos") {
            "osascript"
        } else if cfg!(windows) {
            "powershell"
        } else {
            "kdialog"
        };
        assert!(on_this_system[0].contains(expected), "{on_this_system:?}");

        // On Windows: the title and the folder are variables, not script text.
        let windows = windows_command("Pick \"x\"", start);
        let vars: Vec<_> = windows.get_envs().collect();
        assert!(vars.contains(&(
            std::ffi::OsStr::new("HARNESS_PICK_TITLE"),
            Some(std::ffi::OsStr::new("Pick \"x\""))
        )));
        assert!(windows
            .get_args()
            .all(|a| !a.to_string_lossy().contains("Pick")));

        // `sh` and `false` stand in for a dialog that answers or is cancelled.
        #[cfg(unix)]
        {
            let mut chosen = Command::new("sh");
            chosen.args(["-c", "echo /home/me/code/app"]);
            assert_eq!(
                run_dialog(chosen),
                Native::Chosen("/home/me/code/app".into())
            );
            // As macOS prints it: with a `/` at the end.
            let mut chosen = Command::new("sh");
            chosen.args(["-c", "echo /Users/me/code/app/"]);
            assert_eq!(
                run_dialog(chosen),
                Native::Chosen("/Users/me/code/app".into())
            );
            assert_eq!(run_dialog(Command::new("false")), Native::Cancelled);
        }
        assert_eq!(
            run_dialog(Command::new("/no/such/dialog")),
            Native::Unavailable
        );
    }

    #[test]
    fn a_long_path_keeps_its_end() {
        assert_eq!(keep_end("/home/me/code", 20), "/home/me/code");
        assert_eq!(keep_end("/home/me/code/app", 8), "…ode/app");
        assert_eq!(keep_end("/home/me/code/app", 8).chars().count(), 8);
    }

    #[test]
    fn the_browser_walks_folders_and_creates_new_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for name in ["beta", "Alpha", ".hidden"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        fs::write(root.join("file.txt"), "").unwrap();
        let tr = I18n::load(None);

        let mut browser = Browser::new("Pick", &root);
        assert_eq!(browser.folders, ["Alpha", "beta"]);
        browser.toggle_hidden();
        assert_eq!(browser.folders, [".hidden", "Alpha", "beta"]);
        browser.toggle_hidden();

        browser.move_by(1);
        browser.open_selected();
        assert_eq!(browser.dir, root.join("beta"));
        assert!(browser.folders.is_empty());

        browser.naming = Some("new-app".into());
        browser.create(&tr);
        assert_eq!(browser.dir, root.join("beta/new-app"));
        assert!(browser.dir.is_dir());

        browser.naming = Some("a/b".into());
        browser.create(&tr);
        assert!(browser.error.is_some());

        browser.up();
        browser.up();
        assert_eq!(browser.dir, root);
        assert_eq!(browser.folders[browser.selected], "beta");
    }
}
