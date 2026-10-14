//! Tests of the bottom line: a message too long for it opens over several lines.

use super::*;

/// A message wider than the 120 columns of the test screen, ending in `END`.
const LONG: &str = "task-001: the design is ready. Read it, then send your answer to the \
                    developer (go on) or the architect (changes), and the rest of this \
                    sentence is here only to be cut off at the right edge END";

/// The bottom rows of the screen, where the message is.
fn bottom(app: &mut App, rows: usize) -> String {
    let text = screen(app);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len() - rows..].join("\n")
}

#[test]
fn a_short_message_has_no_open_mark() {
    let env = Env::new();
    let mut app = env.app(env.code.path());
    app.message = Some(Message::info("saved"));

    let text = bottom(&mut app, 1);

    assert!(text.contains("✓ saved"), "{text}");
    assert!(!text.contains('⤢'), "{text}");
}

#[test]
fn a_long_message_opens_and_closes_with_a_click() {
    let env = Env::new();
    let mut app = env.app(env.code.path());
    app.message = Some(Message::info(LONG));
    let text = bottom(&mut app, 1);
    assert!(text.contains("⤢ ✓ task-001"), "{text}");
    assert!(!text.contains("END"), "{text}");

    click(&mut app, "⤢");
    let text = bottom(&mut app, 3);
    assert!(text.contains("⤡ ✓ task-001"), "{text}");
    assert!(text.contains("END"), "{text}");
    assert!(
        text.contains("H help"),
        "the hot keys stay under it:\n{text}"
    );

    click(&mut app, "⤡");
    let text = bottom(&mut app, 1);
    assert!(text.contains("⤢"), "{text}");
    assert!(!text.contains("END"), "{text}");
}

#[test]
fn m_opens_a_long_message_and_esc_closes_it() {
    let env = Env::new();
    let mut app = env.app(env.code.path());
    app.message = Some(Message::error(LONG));

    // «Ь» is the M key with the Russian layout.
    app.on_key(KeyEvent::new(KeyCode::Char('Ь'), KeyModifiers::SHIFT));
    assert!(bottom(&mut app, 3).contains("END"));

    key(&mut app, KeyCode::Esc);
    assert!(!app.quit, "the first Esc only closes the message");
    assert!(!bottom(&mut app, 3).contains("END"));

    key(&mut app, KeyCode::F(4));
    assert!(bottom(&mut app, 3).contains("END"));
}

#[test]
fn m_without_a_message_opens_nothing() {
    let env = Env::new();
    let mut app = env.app(env.code.path());

    key(&mut app, KeyCode::Char('M'));
    app.message = Some(Message::info(LONG));

    assert!(!bottom(&mut app, 3).contains("END"));
}
