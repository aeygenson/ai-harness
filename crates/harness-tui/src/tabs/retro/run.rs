//! Generating a retrospective in the background and taking its result.

use std::path::Path;
use std::sync::mpsc::channel;

use tokio::sync::oneshot;

use harness_agents::process;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::retro::ops::{self};
use harness_core::retro::suggest::{self};

use super::tab::RetroBuilder;
use super::tab::{Focus, Generating, RetroTab};
use crate::tabs::tasks::runner::{push_line, readable, run_until_stopped, Background};
use crate::ui::i18n::I18n;
use crate::ui::message::Message;

impl RetroTab {
    /// Starts «Generate» in the background; the agent writes in `language`.
    pub fn generate(&mut self, builder: RetroBuilder, language: &str) {
        if self.generating.is_some() {
            return;
        }
        let (log, log_rx) = channel();
        let (done, done_rx) = channel();
        let (root, language) = (self.root.clone(), language.to_string());
        let background = Background::start(move |stop| {
            process::set_live_log(Some(log));
            let result = generate(&root, builder, &language, stop);
            process::set_live_log(None);
            let _ = done.send(result);
        });
        self.log.clear();
        self.scroll = 0;
        self.generating = Some(Generating {
            log: log_rx,
            done: done_rx,
            _background: background,
        });
    }

    /// Takes what the agent printed; once it is done, the message to show.
    pub fn tick(&mut self, tr: &I18n) -> Option<Message> {
        let generating = self.generating.as_ref()?;
        for line in generating.log.try_iter() {
            if let Some(line) = readable(&line) {
                push_line(&mut self.log, line);
            }
        }
        let result = match generating.done.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(tr.t("errors.retro_stopped").to_string())
            }
        };
        self.generating = None;
        self.reload();
        // The newest one is shown, whether the agent managed or not.
        if let Some(newest) = self.list.first().map(|r| r.number.clone()) {
            self.select_number(&newest);
        }
        self.focus = Focus::Retros;
        Some(match result {
            Ok(number) => Message::info(tr.f("retro.generated", &[("number", &number)])),
            Err(error) => Message::error(tr.f("retro.failed", &[("error", &error)])),
        })
    }
}

/// «Generate»: the statistics of every task, then the agent's lessons and
/// proposals. Returns the number of the new retrospective.
pub(super) fn generate(
    root: &Path,
    builder: RetroBuilder,
    language: &str,
    stop: oneshot::Receiver<()>,
) -> Result<String, String> {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let repo = Repo::open(root).map_err(|e| text(&e))?;
    let config = Config::load(&root.join(HARNESS_DIR)).map_err(|e| text(&e))?;
    // Everything the agent needs is checked before anything is saved.
    ops::check_clean(&repo).map_err(|e| text(&e))?;
    let agent = builder(&config).map_err(|e| text(&e))?;
    let (dir, stats) = ops::save_stats(&repo, None, Some(&config)).map_err(|e| text(&e))?;
    let number = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    run_until_stopped(
        suggest::suggest(&repo, &dir, &stats, &config, &agent, language),
        stop,
    )?
    .map_err(|e| text(&e))?;
    Ok(number)
}
