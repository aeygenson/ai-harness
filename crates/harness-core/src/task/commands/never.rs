//! The programs an extra command may never start with (see
//! [`super::check_extra_command`]).

/// Programs an extra command may never start with. They run any code or
/// program they are given (shells, interpreters, wrappers such as `timeout`
/// or `docker`), build or install things (package managers and build tools,
/// whose tests and build scripts are code too), change files, or act with
/// the user's own cloud login (like `gh`, which no role may run). `git` is
/// here as well: the built-in checks already read the change with git, and
/// other git commands can change the repository (`git config` can even make
/// git itself run a command later). The list cannot be complete, so allow
/// only programs you know.
const NEVER_EXTRA: &[&str] = &[
    // Shells and interpreters.
    "bash",
    "sh",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "rbash",
    "csh",
    "tcsh",
    "nu",
    "xonsh",
    "cmd",
    "powershell",
    "pwsh",
    "py",
    "ipython",
    "jupyter",
    "jython",
    "jruby",
    "nodejs",
    "deno",
    "bun",
    "tsx",
    "ts-node",
    "java",
    "jshell",
    "groovy",
    "r",
    "rscript",
    "julia",
    "osascript",
    "cscript",
    "wscript",
    "mshta",
    "wsl",
    "busybox",
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "sed",
    "gsed",
    "expect",
    "wish",
    "ash",
    "toybox",
    "sqlite3",
    "psql",
    // Programs and shell built-ins that start other programs.
    "env",
    "genv",
    "sudo",
    "doas",
    "su",
    "xargs",
    "gxargs",
    "find",
    "eval",
    "exec",
    "source",
    "command",
    "builtin",
    "script",
    "timeout",
    "gtimeout",
    "nice",
    "nohup",
    "time",
    "watch",
    "setsid",
    "stdbuf",
    "parallel",
    "flock",
    "chroot",
    "unshare",
    "strace",
    "ltrace",
    "valgrind",
    "pkexec",
    "runuser",
    "nsenter",
    "bwrap",
    "firejail",
    "fakeroot",
    "ionice",
    "taskset",
    "at",
    "crontab",
    "launchctl",
    "gdb",
    "man",
    "tmux",
    "screen",
    "systemd-run",
    "arch",
    "caffeinate",
    "open",
    "xdg-open",
    "rundll32",
    "msiexec",
    "forfiles",
    "schtasks",
    "start",
    "wmic",
    "certutil",
    "bitsadmin",
    "regsvr32",
    "reg",
    "docker",
    "podman",
    "ssh",
    "scp",
    // Editors and pagers that can run a command.
    "vi",
    "vim",
    "nvim",
    "emacs",
    "less",
    "more",
    "ed",
    "nano",
    // Package managers, compilers and build tools.
    "npm",
    "npx",
    "pnpm",
    "pnpx",
    "yarn",
    "yarnpkg",
    "bunx",
    "corepack",
    "pipx",
    "uv",
    "uvx",
    "poetry",
    "pdm",
    "hatch",
    "conda",
    "pixi",
    "tox",
    "nox",
    "pytest",
    "composer",
    "cargo",
    "rustc",
    "rustup",
    "dotnet",
    "msbuild",
    "gcc",
    "clang",
    "javac",
    "make",
    "gmake",
    "cmake",
    "ninja",
    "meson",
    "bazel",
    "just",
    "gradle",
    "gradlew",
    "mvn",
    "mvnw",
    "sbt",
    "ant",
    "swift",
    "mix",
    "cabal",
    "zig",
    "nix",
    "rake",
    "bundle",
    "gem",
    "brew",
    "apt",
    "apt-get",
    "dnf",
    "yum",
    "pacman",
    "winget",
    "choco",
    "scoop",
    // Commands that change files.
    "tee",
    "gtee",
    "touch",
    "chmod",
    "chown",
    "ln",
    "dd",
    "tar",
    "gtar",
    "rsync",
    "install",
    "ginstall",
    "truncate",
    "patch",
    "mkdir",
    "rmdir",
    "unlink",
    "shred",
    "zip",
    "unzip",
    "gzip",
    "xz",
    "7z",
    "cpio",
    "split",
    "sort",
    "openssl",
    "yq",
    // Tools that act with the user's own cloud login.
    "aws",
    "az",
    "gcloud",
    "glab",
    "kubectl",
    "gsutil",
    "bq",
    "terraform",
    "helm",
    "heroku",
    "snyk",
    "hub",
    "op",
    "vault",
    "security",
    // The repository: see above.
    "git",
];

/// Interpreters and tools that are also installed under a name with their
/// version, such as `python3.12`, `pip3.12` or `perl5.34`: the name alone or
/// followed only by digits and dots is refused (`phpstan` is not).
const VERSIONED: &[&str] = &[
    "python", "pypy", "pip", "perl", "ruby", "php", "lua", "tclsh", "node", "go",
];

/// Is `program` (lower case, without a Windows ending) one an extra command
/// may never start with? `git-lfs` and other `git-` helpers count as git.
pub(super) fn is_never_extra(program: &str) -> bool {
    NEVER_EXTRA.contains(&program)
        || program.starts_with("git-")
        || VERSIONED.iter().any(|name| is_versioned(program, name))
}

/// Is `program` exactly `name`, or `name` followed by a version such as `3.12`?
fn is_versioned(program: &str, name: &str) -> bool {
    program
        .strip_prefix(name)
        .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_names_are_refused_but_longer_names_are_not() {
        for program in ["python", "python3.12", "pip3", "pip3.12", "go1.22", "node"] {
            assert!(is_never_extra(program), "{program}");
        }
        for program in ["phpstan", "perlcritic", "gosec", "pip-audit"] {
            assert!(!is_never_extra(program), "{program}");
        }
    }
}
