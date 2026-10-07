//! The terminal around the app: the loop that draws and reads events, and
//! handing the terminal to another program (an editor, `harness login`) and back.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use harness_platform::editor;
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

/// How often the open project is read again.
const RELOAD_EVERY: Duration = Duration::from_secs(3);

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

/// Draws the app and hands it every key, click and paste until Lisa quits.
pub(crate) fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut loaded = Instant::now();
    while !app.quit {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
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
        if loaded.elapsed() >= RELOAD_EVERY {
            if let Some(tasks) = &mut app.tasks {
                tasks.reload();
            }
            loaded = Instant::now();
        }
    }
    Ok(())
}

/// Gives the terminal to the editor and takes it back when the editor is closed.
fn edit_outside(terminal: &mut DefaultTerminal, file: &Path, tr: &I18n) -> Result<(), String> {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    println!("{}", tr.f("skills.editing", &[("path", &file.display())]));
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
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    println!("{}\n", tr.f("agents.signing_in", &[("name", &name)]));
    let result = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|harness| {
            std::process::Command::new(harness)
                .args(["login", login])
                .status()
                .map_err(|e| e.to_string())
        })
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(status.to_string())
            }
        });
    println!("\n{}", tr.t("agents.sign_in_back"));
    let mut line = String::new();
    let _ = io::BufRead::read_line(&mut io::stdin().lock(), &mut line);
    take_terminal_back(terminal);
    result
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
