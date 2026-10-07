//! A fake program for the tests, the same on Linux, macOS and Windows.
//!
//! Tests used to stand in for an agent, `curl` or an MCP server with a small
//! shell script, which does not run on Windows. Instead, [`install`] copies a
//! tiny Rust program (built by `build.rs` from `src/program.rs`) under the
//! wanted name and writes its script next to it: `bin/claude` (or
//! `bin/claude.exe`) reads `bin/claude.fake`.
//!
//! The script has one command per line; empty lines and lines starting with
//! `#` are skipped. In the text after a command, `$1`…`$9` and `${N}` are the
//! arguments, `${N#PREFIX}` is argument N without PREFIX, `${after:FLAG}` is
//! the argument after FLAG, `${stdin}` is what `read-stdin` read (without the
//! last line end), and `${NAME}` is an environment variable.
//!
//! | Command | What it does |
//! |---|---|
//! | `read-stdin` | Reads all of standard input. |
//! | `read-line` | Reads one line of standard input and forgets it. |
//! | `save-stdin FILE` | Reads all of standard input and saves it. |
//! | `save-env FILE` | Saves the environment, sorted `NAME=VALUE` lines. |
//! | `save-args FILE` / `append-args FILE` | Saves the arguments, one per line. |
//! | `save-text FILE TEXT` | Saves TEXT, without a line end. |
//! | `write FILE` | Saves the next lines as they are, up to a line `end`. |
//! | `copy FROM TO` | Copies a file. |
//! | `list DIR FILE` | Saves the sorted names in DIR. |
//! | `print TEXT` / `eprint TEXT` | Prints a line to standard output / error. |
//! | `print-file FILE` / `eprint-file FILE` | Prints a file. |
//! | `need-args WORDS… else CODE` | Exits with CODE unless the arguments are WORDS. |
//! | `need-file FILE else CODE` | Exits with CODE unless FILE exists. |
//! | `need-text FILE TEXT else CODE` | Exits with CODE unless FILE contains TEXT. |
//! | `when-arg N VALUE COMMAND…` | Runs COMMAND when argument N is VALUE. |
//! | `spawn-sleep SECONDS PIDFILE` | Starts a sleeping child (like `sleep 30 &`), saves its id. |
//! | `wait` | Waits for the children started by `spawn-sleep`. |
//! | `sleep SECONDS` | Sleeps; fractions are fine. |
//! | `exit CODE` | Stops with that exit code. |
//!
//! Files are written with their folders created. A mistake in the script
//! stops the program with code 101 and a message on standard error.

use std::fs;
use std::path::{Path, PathBuf};

/// Where `build.rs` put the compiled fake program.
const PROGRAM: &str = env!("HARNESS_FAKE_PROGRAM");

/// Puts a fake program called `name` into `dir`, doing what `script` says.
///
/// Returns the program's full path (with `.exe` on Windows); `dir` can be
/// added to `PATH` so the program is found by name.
///
/// # Panics
///
/// When the program cannot be copied or the script cannot be written: in a
/// test that is a broken test folder, not something to handle.
pub fn install(dir: &Path, name: &str, script: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("cannot create the folder for the fake program");
    let program = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    // A program installed earlier under this name goes first: it may be a
    // link to `PROGRAM`, and copying over it would change `PROGRAM` itself.
    let _ = fs::remove_file(&program);
    // A hard link writes nothing. A copy opens the new file for writing, and
    // on Linux another test starting a program at that moment holds that
    // file open for an instant, so starting the copy can fail with "Text
    // file busy". The copy is only for a folder on another disk.
    if fs::hard_link(PROGRAM, &program).is_err() {
        // `fs::copy` keeps the "executable" permission on Unix.
        fs::copy(PROGRAM, &program).expect("cannot copy the fake program");
    }
    fs::write(dir.join(format!("{name}.fake")), script).expect("cannot write the fake's script");
    program
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::*;

    fn run(program: &Path, args: &[&str], input: &str) -> std::process::Output {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    #[test]
    fn the_fake_prints_reads_and_exits_as_its_script_says() {
        let dir = tempfile::tempdir().unwrap();
        let script = "# a comment\n\
            read-stdin\n\
            print got ${stdin} with $1 and ${after:-m} ${2#@}\n\
            eprint oops\n\
            exit 3\n\
            print never\n";
        let program = install(dir.path(), "agent", script);

        let output = run(&program, &["one", "@two", "-m", "big"], "hello\n");

        assert_eq!(output.status.code(), Some(3));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "got hello with one and big two\n"
        );
        assert_eq!(String::from_utf8_lossy(&output.stderr), "oops\n");
    }

    #[test]
    fn the_fake_writes_files_and_checks_conditions() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let script = format!(
            "save-args {out}/args\n\
             write {out}/deep/note.json\n\
             {{\"a\": \"$1\"}}\n\
             end\n\
             need-file {out}/deep/note.json else 5\n\
             need-text {out}/args first else 6\n\
             when-arg 1 first print matched\n\
             need-args first second else 7\n\
             need-file {out}/missing else 8\n",
            out = out.display()
        );
        let program = install(&dir.path().join("bin"), "tool", &script);

        let output = run(&program, &["first"], "");

        assert_eq!(output.status.code(), Some(7));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "matched\n");
        assert_eq!(fs::read_to_string(out.join("args")).unwrap(), "first\n");
        // Lines of `write` are kept as they are, `$1` too.
        assert_eq!(
            fs::read_to_string(out.join("deep/note.json")).unwrap(),
            "{\"a\": \"$1\"}\n"
        );
    }

    #[test]
    fn an_unknown_command_stops_the_fake_with_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let program = install(dir.path(), "broken", "dance\n");

        let output = run(&program, &[], "");

        assert_eq!(output.status.code(), Some(101));
        assert!(String::from_utf8_lossy(&output.stderr).contains("dance"));
    }
}
