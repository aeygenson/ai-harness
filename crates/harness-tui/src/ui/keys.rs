//! Hot keys that work in any keyboard layout. With the Russian layout the key
//! marked Q sends «й»; it is read as `q`, so switching the layout is not needed.

use ratatui::crossterm::event::KeyCode;

/// Russian letters in the order of the Latin keys they share.
const RUSSIAN: &str = "йцукенгшщзфывапролдячсмить";
const LATIN: &str = "qwertyuiopasdfghjklzxcvbnm";

/// The Latin key for a Russian letter, keeping upper case; other keys as they are.
pub fn latin(code: KeyCode) -> KeyCode {
    let KeyCode::Char(c) = code else {
        return code;
    };
    let lower = c.to_lowercase().next().unwrap_or(c);
    match RUSSIAN.chars().position(|r| r == lower) {
        Some(index) => {
            let latin = LATIN.chars().nth(index).unwrap_or(c);
            KeyCode::Char(if c.is_uppercase() {
                latin.to_ascii_uppercase()
            } else {
                latin
            })
        }
        None => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_letters_are_read_as_the_latin_keys() {
        assert_eq!(RUSSIAN.chars().count(), LATIN.chars().count());
        assert_eq!(latin(KeyCode::Char('й')), KeyCode::Char('q'));
        assert_eq!(latin(KeyCode::Char('Д')), KeyCode::Char('L'));
        assert_eq!(latin(KeyCode::Char('т')), KeyCode::Char('n'));
        assert_eq!(latin(KeyCode::Char('ы')), KeyCode::Char('s'));
        assert_eq!(latin(KeyCode::Char('x')), KeyCode::Char('x'));
        assert_eq!(latin(KeyCode::Char('1')), KeyCode::Char('1'));
        assert_eq!(latin(KeyCode::Enter), KeyCode::Enter);
    }
}
