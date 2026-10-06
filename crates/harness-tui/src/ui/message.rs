//! The one-line message at the bottom of the screen: a result or a problem.

/// Whether a message reports something done or something wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Shown with ✓ in the "ok" color.
    Info,
    /// Shown with ✗ in the "bad" color.
    Error,
}

/// A message for the bottom line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// What to show, without the mark in front.
    pub text: String,
    /// Picks the mark and the color.
    pub kind: MessageKind,
}

impl Message {
    /// A result: something was done or started.
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: MessageKind::Info,
        }
    }

    /// A problem: something could not be done.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: MessageKind::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_the_kind() {
        assert_eq!(Message::info("saved").kind, MessageKind::Info);
        let failed = Message::error(String::from("disk full"));
        assert_eq!(failed.kind, MessageKind::Error);
        assert_eq!(failed.text, "disk full");
    }
}
