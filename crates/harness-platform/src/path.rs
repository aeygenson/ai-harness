//! Paths shown to Lisa and compared with git's always use `/`, as git does,
//! so a path reads and compares the same on every system.

use std::path::Path;

/// `path` with `/` between its parts. Windows writes `\`, which can never be
/// part of a file name there; elsewhere `\` is a letter and stays.
pub fn slashed(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_gets_slashes() {
        let path = Path::new("commands").join("more.md");
        assert_eq!(slashed(&path), "commands/more.md");
        assert_eq!(slashed(Path::new("docs/parser.md")), "docs/parser.md");
    }
}
