//! The live log: every line a running agent prints also goes to the TUI,
//! which shows it while the agent works.

use std::sync::mpsc::Sender;
use std::sync::Mutex;

use harness_core::secret;

/// Where the live log goes: the TUI shows what the running agent prints.
static LIVE_LOG: Mutex<Option<Sender<String>>> = Mutex::new(None);

/// From now on every line an agent prints is also sent to `sink`, with the
/// agent's secrets hidden; `None` stops it. The command line never sets it.
pub fn set_live_log(sink: Option<Sender<String>>) {
    if let Ok(mut live_log) = LIVE_LOG.lock() {
        *live_log = sink;
    }
}

/// Sends a line of the harness's own (not an agent's) to the live log, for
/// example what a merge is waiting for. Nothing happens without a live log.
pub fn note_live(line: &str) {
    send_live(line.as_bytes(), &[]);
}

/// Sends one line to the live log, with `secrets` hidden.
pub(super) fn send_live(line: &[u8], secrets: &[&str]) {
    let Ok(mut live_log) = LIVE_LOG.lock() else {
        return;
    };
    let Some(sink) = live_log.as_ref() else {
        return;
    };
    let line = secret::hide(String::from_utf8_lossy(line).trim_end(), secrets);
    if sink.send(line).is_err() {
        // Nobody listens any more.
        *live_log = None;
    }
}
