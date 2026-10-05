//! Makes text from outside the harness safe to show in a terminal.
//!
//! Agents, MCP servers and plugin catalogs can send any characters. A terminal
//! treats some of them as commands: `ESC [ 2 J` clears the screen, `ESC ] 8`
//! makes a hidden link, a carriage return lets later text overwrite earlier
//! text. Direction marks (U+202E and others) can make text read differently
//! from what it is. The functions here remove all of that and keep the words.

/// The text without terminal escape sequences, control characters and
/// direction marks. Line breaks stay; a tab becomes a space.
pub fn safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // `peekable` lets us look at the next character without taking it.
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() || is_direction_mark(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// One line of at most `max_chars` characters: like [`safe`], with line
/// breaks turned into spaces and the ends trimmed.
pub fn safe_line(text: &str, max_chars: usize) -> String {
    let one_line = safe(text).replace('\n', " ");
    let short: String = one_line.trim().chars().take(max_chars).collect();
    short.trim_end().to_string()
}

/// Skips the rest of an escape sequence; the `ESC` itself is already taken.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        // CSI, such as colours (`ESC [ 3 1 m`): ends with a character from `@` to `~`.
        Some('[') => {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
        // OSC, such as a window title or a link (`ESC ] ...`): ends with BEL
        // or with `ESC \`.
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        // Any other escape is `ESC` and one more character.
        _ => {}
    }
}

/// Unicode marks that change the direction of the text around them.
fn is_direction_mark(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_screen_commands_are_removed() {
        assert_eq!(safe("\u{1b}[31mred\u{1b}[0m text"), "red text");
        assert_eq!(safe("before\u{1b}[2Jafter"), "beforeafter");
        assert_eq!(safe("a\u{1b}=b"), "ab");
    }

    #[test]
    fn hidden_links_and_titles_are_removed() {
        let link = "\u{1b}]8;;https://evil.example\u{1b}\\click\u{1b}]8;;\u{1b}\\";
        assert_eq!(safe(link), "click");
        assert_eq!(safe("\u{1b}]0;new title\u{7}text"), "text");
    }

    #[test]
    fn control_characters_go_and_line_breaks_stay() {
        assert_eq!(safe("one\r\ntwo\u{7}\u{8}\u{0}"), "one\ntwo");
        assert_eq!(safe("a\tb"), "a b");
        assert_eq!(safe("\u{9b}31mx"), "31mx");
    }

    #[test]
    fn direction_marks_are_removed() {
        assert_eq!(safe("abc\u{202e}fed\u{202c}"), "abcfed");
        assert_eq!(safe("\u{2067}x\u{2069}"), "x");
    }

    #[test]
    fn words_in_any_language_stay() {
        assert_eq!(safe("Готово ✓ → 完成"), "Готово ✓ → 完成");
    }

    #[test]
    fn safe_line_is_one_short_line() {
        assert_eq!(safe_line("  first\nsecond\u{1b}[0m  ", 100), "first second");
        assert_eq!(safe_line("abcdef", 3), "abc");
        assert_eq!(safe_line("ab  cd", 3), "ab");
        assert_eq!(safe_line("\u{1b}[1m", 10), "");
    }
}
