//! The terminal around the app: the loop that draws and reads events, and
//! handing the terminal to another program (an editor, `harness login`, a
//! program's installer) and back.

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use harness_platform::editor;
use harness_platform::input::{self, Input};
use harness_platform::lock::{self, Held};
use harness_platform::stop::StopSignals;
use ratatui::crossterm::cursor::Hide;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{enable_raw_mode, Clear, ClearType, EnterAlternateScreen};
use ratatui::prelude::CrosstermBackend;
use ratatui::{DefaultTerminal, Terminal};

use crate::ui::i18n::I18n;
use crate::App;

/// The lock file in `~/.harness` that the open TUI holds.
const WINDOW_LOCK: &str = "tui.lock";

/// How often the open project is read again.
const RELOAD_EVERY: Duration = Duration::from_secs(3);

/// How long one turn of the loop waits for a key before it ticks (the
/// spinner, a finished agent): short enough that the screen looks alive.
const WAIT_FOR_INPUT: Duration = Duration::from_millis(250);

/// Stops with a clear error when the terminal has no size (0x0).
///
/// That happens in a pseudo-terminal made by a script: nothing could be
/// drawn, and the TUI would only wait for `q` on an empty screen.
pub(crate) fn check_size() -> Result<()> {
    // When the size cannot be read at all, the drawing reports that itself.
    let Ok((columns, rows)) = ratatui::crossterm::terminal::size() else {
        return Ok(());
    };
    check_size_of(columns, rows)
}

fn check_size_of(columns: u16, rows: u16) -> Result<()> {
    if columns == 0 || rows == 0 {
        bail!(
            "the terminal has no size ({columns}x{rows}), so the TUI cannot draw anything; \
             start it in a terminal window, or set a size first, for example \
             `stty rows 40 cols 120`"
        );
    }
    Ok(())
}

/// Stops with a clear error when another TUI window is already open.
///
/// Two windows would read and change the same tasks and settings at once,
/// for example both start the same role. One window per user is allowed,
/// whatever project it shows, since all of them share `~/.harness`. Without
/// a home folder there is nothing to share and nothing is locked.
pub(crate) fn hold_the_window(harness_home: Option<&Path>) -> Result<Option<Held>> {
    let Some(home) = harness_home else {
        return Ok(None);
    };
    let path = home.join(WINDOW_LOCK);
    match lock::try_hold(&path) {
        Ok(Some(held)) => Ok(Some(held)),
        Ok(None) => bail!(
            "the harness is already open in another window; switch to that window, \
             or close it there with `q` and start `harness tui` again"
        ),
        Err(error) => Err(error).with_context(|| format!("cannot lock {}", path.display())),
    }
}

/// Starts catching the system's stop signals (the terminal window closed,
/// `kill`; see `harness_platform::stop`) and returns a flag a thread sets
/// when one comes. Without it the TUI would die at once and leave a running
/// agent behind; with it the event loop ends normally, and closing the app
/// stops the agent with everything it started.
pub(crate) fn watch_stop_signals() -> io::Result<Arc<AtomicBool>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    // Listening starts here, before this function returns: `enter` makes
    // the runtime current for this block, which `listen` needs.
    let mut signals = {
        let _inside = runtime.enter();
        StopSignals::listen()?
    };
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    // The thread waits until a signal comes, or until the program ends.
    std::thread::spawn(move || {
        runtime.block_on(signals.recv());
        flag.store(true, Ordering::SeqCst);
    });
    Ok(stop)
}

/// Draws the app and hands it every key, click and paste until Lisa quits,
/// `stop` is set (see [`watch_stop_signals`]) or the terminal is closed.
pub(crate) fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    stop: &AtomicBool,
) -> Result<()> {
    let mut loaded = Instant::now();
    while !app.quit && !stop.load(Ordering::SeqCst) {
        terminal.draw(|frame| app.draw(frame))?;
        match input::wait_for_input(WAIT_FOR_INPUT)? {
            // The window is gone: end like a stop signal, which stops the agent.
            Input::Closed => break,
            Input::Nothing => {}
            // One read may bring several events (a paste, fast typing); crossterm
            // keeps the extra ones, so take everything it has before drawing.
            Input::Ready => {
                while event::poll(Duration::ZERO)? {
                    handle(app, event::read()?);
                }
            }
            Input::Unwatched => {
                if event::poll(WAIT_FOR_INPUT)? {
                    handle(app, event::read()?);
                }
            }
        }
        app.tick();
        app.open_task_file();
        if let Some(job) = app.edit.take() {
            let result = edit_outside(terminal, &job.path, &app.tr);
            app.finish_edit(&job, result);
        }
        if let Some((login, name)) = app.sign_in.take() {
            let result = sign_in_outside(terminal, login, name, &app.tr);
            app.finish_sign_in(name, result);
        }
        if let Some((name, command)) = app.tool_install.take() {
            let result = install_outside(terminal, name, &command, &app.tr);
            app.finish_tool_install(name, result);
        }
        if let Some((name, path)) = app.tool_sign_in.take() {
            let heading = app.tr.f("agents.signing_in", &[("name", &name)]);
            let commands = harness_agents::github::sign_in_commands(&path);
            let result = run_outside(terminal, &heading, commands, &app.tr);
            app.finish_tool_sign_in(name, result);
        }
        if loaded.elapsed() >= RELOAD_EVERY {
            if let Some(tasks) = &mut app.tasks {
                tasks.reload();
            }
            loaded = Instant::now();
        }
    }
    Ok(())
}

/// Hands one key, click or paste to the app.
fn handle(app: &mut App, event: Event) {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
        Event::Mouse(mouse) => app.on_mouse(mouse),
        Event::Paste(text) => app.on_paste(&text),
        _ => {}
    }
}

/// Gives the terminal to the editor and takes it back when the editor is closed.
fn edit_outside(terminal: &mut DefaultTerminal, file: &Path, tr: &I18n) -> Result<(), String> {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    let _ = ratatui::try_restore();
    say(&tr.f("skills.editing", &[("path", &file.display())]));
    let result = editor::run(file);
    take_terminal_back(terminal);
    result
}

/// Gives the terminal to `harness login` (it asks for a token or opens the
/// browser) and takes it back once Lisa presses Enter.
fn sign_in_outside(
    terminal: &mut DefaultTerminal,
    login: &str,
    name: &str,
    tr: &I18n,
) -> Result<(), String> {
    let heading = tr.f("agents.signing_in", &[("name", &name)]);
    let command = std::env::current_exe().map(|harness| {
        let mut command = Command::new(harness);
        command.args(["login", login]);
        command
    });
    match command {
        Ok(command) => run_outside(terminal, &heading, vec![command], tr),
        Err(error) => Err(error.to_string()),
    }
}

/// Gives the terminal to the command that installs a program (it may ask
/// for a password) and takes it back once Lisa presses Enter.
fn install_outside(
    terminal: &mut DefaultTerminal,
    name: &str,
    line: &str,
    tr: &I18n,
) -> Result<(), String> {
    let heading = tr.f(
        "agents.installing_tool",
        &[("name", &name), ("command", &line)],
    );
    let (shell, flags) = harness_platform::program::shell();
    let mut command = Command::new(shell);
    command.args(flags).arg(line);
    run_outside(terminal, &heading, vec![command], tr)
}

/// Leaves the full screen, prints `heading`, runs `commands` one after
/// another in the plain terminal (they read the keyboard themselves) and
/// takes the terminal back once Lisa has read the output and pressed Enter.
/// A command that fails stops the ones after it.
fn run_outside(
    terminal: &mut DefaultTerminal,
    heading: &str,
    commands: Vec<Command>,
    tr: &I18n,
) -> Result<(), String> {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    let _ = ratatui::try_restore();
    say(&format!("{heading}\n"));
    let result = commands
        .into_iter()
        .try_for_each(|mut command| match command.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(status.to_string()),
            Err(error) => Err(error.to_string()),
        });
    say(&format!("\n{}", tr.t("agents.sign_in_back")));
    let mut line = String::new();
    let _ = io::BufRead::read_line(&mut io::stdin().lock(), &mut line);
    take_terminal_back(terminal);
    result
}

/// Prints a line for Lisa in the plain terminal. Unlike `println!`, which
/// panics when it cannot write, this ignores a closed window: the loop then
/// notices the closed terminal and ends normally.
fn say(text: &str) {
    let _ = writeln!(io::stdout(), "{text}");
}

/// Full screen again after a program had the terminal.
fn take_terminal_back(terminal: &mut DefaultTerminal) {
    let _ = enable_raw_mode();
    let _ = execute!(
        io::stdout(),
        EnterAlternateScreen,
        Clear(ClearType::All),
        Hide,
        EnableMouseCapture,
        EnableBracketedPaste
    );
    // A new terminal draws everything again. (`Terminal::clear` would ask the
    // terminal where its cursor is, and not every terminal answers.)
    if let Ok(fresh) = Terminal::new(CrosstermBackend::new(io::stdout())) {
        *terminal = fresh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_window_is_refused_while_the_first_is_open() {
        let home = tempfile::tempdir().unwrap();

        let first = hold_the_window(Some(home.path())).unwrap();
        let second = hold_the_window(Some(home.path()));

        assert!(first.is_some());
        let error = second.unwrap_err().to_string();
        assert!(error.contains("already open"), "{error}");
    }

    #[test]
    fn without_a_home_folder_nothing_is_locked() {
        assert!(hold_the_window(None).unwrap().is_none());
    }

    // Sending a signal to the test program itself works only on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_closed_terminal_sets_the_stop_flag_instead_of_killing_the_tui() {
        let stop = watch_stop_signals().unwrap();
        assert!(!stop.load(Ordering::SeqCst));

        let status = std::process::Command::new("kill")
            .args(["-HUP", &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());

        let start = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            assert!(start.elapsed() < Duration::from_secs(10), "not noticed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_terminal_without_size_is_refused() {
        let error = check_size_of(0, 0).unwrap_err().to_string();

        assert!(error.contains("no size (0x0)"), "{error}");
        assert!(error.contains("stty rows 40 cols 120"), "{error}");
        assert!(check_size_of(120, 0).is_err());
        assert!(check_size_of(0, 40).is_err());
    }

    #[test]
    fn a_terminal_with_size_is_accepted() {
        check_size_of(120, 40).unwrap();
        check_size_of(1, 1).unwrap();
    }
}
