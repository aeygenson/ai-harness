//! Agents start with an empty environment plus a short list of variables
//! from Lisa's terminal: none of them holds a secret, so no `*_API_KEY`
//! reaches an agent by accident. Windows needs a few more of its own:
//! without `SystemRoot`, for example, programs cannot use the network.

/// Variables an agent may inherit on every system.
const COMMON: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "TERM",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// Only on Linux and macOS.
const UNIX: &[&str] = &["USER", "TMPDIR"];

/// Only on Windows: where the system, the user's folders and the temporary
/// folder are, and how programs are found (`PATHEXT`).
const WINDOWS: &[&str] = &[
    "SystemRoot",
    "windir",
    "SystemDrive",
    "ComSpec",
    "PATHEXT",
    "USERNAME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "TEMP",
    "TMP",
    "OS",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
];

/// The variables an agent inherits on this system.
pub fn inherited() -> impl Iterator<Item = &'static str> {
    let own = if cfg!(windows) { WINDOWS } else { UNIX };
    COMMON.iter().chain(own).copied()
}

/// The inherited variables that are set here, with their values.
pub fn inherited_values() -> impl Iterator<Item = (&'static str, std::ffi::OsString)> {
    inherited().filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
}

/// Is `name` one of the inherited variables on this system? Windows ignores
/// case in variable names, so the check does too there.
pub fn is_inherited(name: &str) -> bool {
    inherited().any(|own| same_name(own, name))
}

/// Variables an MCP server may not set, on any system: the agents read them
/// themselves. The same list everywhere, so a `harness.toml` written on one
/// system is checked the same way on another.
pub fn is_reserved(name: &str) -> bool {
    COMMON
        .iter()
        .chain(UNIX)
        .chain(WINDOWS)
        .any(|own| own.eq_ignore_ascii_case(name))
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_secret_is_inherited() {
        for name in inherited() {
            let upper = name.to_ascii_uppercase();
            assert!(
                !["KEY", "TOKEN", "SECRET", "PASSWORD"]
                    .iter()
                    .any(|word| upper.contains(word)),
                "{name}"
            );
        }
    }

    #[test]
    fn each_system_gets_its_own_variables() {
        assert!(is_inherited("PATH"));
        assert_eq!(is_inherited("SystemRoot"), cfg!(windows));
        assert_eq!(is_inherited("TMPDIR"), !cfg!(windows));
        assert!(!is_inherited("OPENAI_API_KEY"));
    }

    #[test]
    fn reserved_names_are_the_same_everywhere() {
        assert!(is_reserved("PATH"));
        assert!(is_reserved("path"));
        assert!(is_reserved("SystemRoot"));
        assert!(is_reserved("TMPDIR"));
        assert!(!is_reserved("CONTEXT7_API_KEY"));
    }
}
