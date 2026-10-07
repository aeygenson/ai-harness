//! The fake program itself. It stands in for an agent (`claude`, `codex`,
//! `agy`, `dsh`), for `curl` or for an MCP server in the tests, the same way
//! on Linux, macOS and Windows, where a shell script would not run.
//!
//! What it does is written in a small script next to it: the program
//! `bin/claude` (or `bin/claude.exe`) reads `bin/claude.fake`. The commands
//! are listed in `lib.rs`. Only the standard library is used, because
//! `build.rs` compiles this file with `rustc` alone.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{self, Child, Command};
use std::thread;
use std::time::Duration;

/// Set for a helper started by `spawn-sleep`: it only sleeps this many seconds.
const SLEEP_VARIABLE: &str = "HARNESS_FAKE_SLEEP";

fn main() {
    if let Ok(seconds) = env::var(SLEEP_VARIABLE) {
        sleep(&seconds);
        return;
    }
    let me = env::current_exe().expect("the fake knows where it is");
    let script_path = me.with_extension("fake");
    let script = fs::read_to_string(&script_path)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", script_path.display())));
    let mut fake = Fake {
        args: env::args().skip(1).collect(),
        stdin: String::new(),
        helpers: Vec::new(),
    };
    let mut lines = script.lines();
    while let Some(line) = lines.next() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        fake.run(line, &mut lines);
    }
}

/// What the fake knows while it works through its script.
struct Fake {
    args: Vec<String>,
    stdin: String,
    helpers: Vec<Child>,
}

impl Fake {
    /// Runs one line of the script; `write` also takes the lines after it.
    fn run(&mut self, line: &str, rest: &mut std::str::Lines) {
        let (command, text) = line.split_once(' ').unwrap_or((line, ""));
        let text = self.expand(text);
        let words: Vec<&str> = text.split_whitespace().collect();
        match command {
            "read-stdin" => self.read_stdin(),
            "read-line" => {
                // One line only, like `read line` in a shell: the caller
                // may be waiting for an answer before it writes the next.
                let mut line = String::new();
                let _ = io::stdin().read_line(&mut line);
            }
            "save-stdin" => {
                self.read_stdin();
                save(&text, &self.stdin);
            }
            "save-env" => {
                let all: BTreeMap<String, String> = env::vars().collect();
                let lines: String = all.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
                save(&text, &lines);
            }
            "save-args" => save(&text, &self.args_text()),
            "append-args" => {
                let old = fs::read_to_string(&text).unwrap_or_default();
                save(&text, &(old + &self.args_text()));
            }
            "save-text" => {
                let (file, value) = text.split_once(' ').unwrap_or((&text, ""));
                save(file, value);
            }
            "write" => {
                let mut content = String::new();
                for line in rest.by_ref().take_while(|l| l.trim() != "end") {
                    content.push_str(line);
                    content.push('\n');
                }
                save(&text, &content);
            }
            "copy" => save(words[1], &read(words[0])),
            "list" => {
                let mut names: Vec<String> = fs::read_dir(words[0])
                    .unwrap_or_else(|e| fail(&format!("cannot list {}: {e}", words[0])))
                    .map(|entry| entry.expect("a folder entry").file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .collect();
                names.sort();
                save(words[1], &names.iter().map(|n| format!("{n}\n")).collect::<String>());
            }
            "print" => out(&format!("{text}\n")),
            "eprint" => err(&format!("{text}\n")),
            "print-file" => out(&read(&text)),
            "eprint-file" => err(&read(&text)),
            "need-args" => {
                let (wanted, code) = condition(&words);
                if !self.args.iter().map(String::as_str).eq(wanted.iter().copied()) {
                    exit(code);
                }
            }
            "need-file" => {
                let (wanted, code) = condition(&words);
                if !Path::new(wanted[0]).is_file() {
                    exit(code);
                }
            }
            "need-text" => {
                let (wanted, code) = condition(&words);
                if !read_or_empty(wanted[0]).contains(wanted[1]) {
                    exit(code);
                }
            }
            "when-arg" => {
                let number: usize = words[0].parse().expect("when-arg NUMBER VALUE COMMAND");
                if self.args.get(number - 1).map(String::as_str) == Some(words[1]) {
                    let command = words[2..].join(" ");
                    self.run(&command, rest);
                }
            }
            "spawn-sleep" => self.spawn_sleep(words[0], words[1]),
            "wait" => {
                for helper in &mut self.helpers {
                    let _ = helper.wait();
                }
            }
            "sleep" => sleep(&text),
            "exit" => exit(text.trim().parse().expect("exit CODE")),
            other => fail(&format!("unknown command in the fake's script: {other}")),
        }
    }

    fn read_stdin(&mut self) {
        let mut all = String::new();
        // A caller that gives no input closes it; an error means the same.
        let _ = io::stdin().read_to_string(&mut all);
        self.stdin = all;
    }

    fn args_text(&self) -> String {
        self.args.iter().map(|a| format!("{a}\n")).collect()
    }

    /// Starts a copy of itself that only sleeps and keeps standard output
    /// open, like `sleep 30 &` in a shell, and saves its process id.
    fn spawn_sleep(&mut self, seconds: &str, pid_file: &str) {
        let me = env::current_exe().expect("the fake knows where it is");
        let helper = Command::new(me)
            .env(SLEEP_VARIABLE, seconds)
            .spawn()
            .unwrap_or_else(|e| fail(&format!("cannot start the helper: {e}")));
        save(pid_file, &helper.id().to_string());
        self.helpers.push(helper);
    }

    /// Replaces `$1`…`$9`, `${N}`, `${N#PREFIX}` (without PREFIX),
    /// `${after:FLAG}`, `${stdin}` and `${NAME}` (an environment variable).
    fn expand(&self, text: &str) -> String {
        let mut result = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                result.push(c);
                continue;
            }
            match chars.peek().copied() {
                Some(digit) if digit.is_ascii_digit() => {
                    chars.next();
                    result.push_str(&self.arg(&digit.to_string()));
                }
                Some('{') => {
                    chars.next();
                    let name: String = chars.by_ref().take_while(|&c| c != '}').collect();
                    result.push_str(&self.variable(&name));
                }
                _ => result.push('$'),
            }
        }
        result
    }

    fn variable(&self, name: &str) -> String {
        if let Some((number, prefix)) = name.split_once('#') {
            let value = self.arg(number);
            return value.strip_prefix(prefix).unwrap_or(&value).to_string();
        }
        if let Some(flag) = name.strip_prefix("after:") {
            let position = self.args.iter().position(|a| a == flag);
            return position
                .and_then(|i| self.args.get(i + 1))
                .cloned()
                .unwrap_or_default();
        }
        if name == "stdin" {
            // Like `$(cat)` in a shell: without the line end at the end.
            return self.stdin.trim_end_matches(['\n', '\r']).to_string();
        }
        if name.chars().all(|c| c.is_ascii_digit()) {
            return self.arg(name);
        }
        env::var(name).unwrap_or_default()
    }

    fn arg(&self, number: &str) -> String {
        let number: usize = number.parse().unwrap_or(0);
        number
            .checked_sub(1)
            .and_then(|i| self.args.get(i))
            .cloned()
            .unwrap_or_default()
    }
}

/// `WORDS… else CODE`: the words and the exit code.
fn condition<'a>(words: &[&'a str]) -> (Vec<&'a str>, i32) {
    let Some(position) = words.iter().position(|w| *w == "else") else {
        fail("a need- line must end with `else CODE`");
    };
    let code = words
        .get(position + 1)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| fail("`else` needs an exit code"));
    (words[..position].to_vec(), code)
}

fn save(file: &str, content: &str) {
    let path = Path::new(file);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, content).unwrap_or_else(|e| fail(&format!("cannot write {file}: {e}")));
}

fn read(file: &str) -> String {
    fs::read_to_string(file).unwrap_or_else(|e| fail(&format!("cannot read {file}: {e}")))
}

fn read_or_empty(file: &str) -> String {
    fs::read_to_string(file).unwrap_or_default()
}

fn out(text: &str) {
    let mut stdout = io::stdout();
    // A reader that went away is not the fake's problem.
    let _ = stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush());
}

fn err(text: &str) {
    let _ = io::stderr().write_all(text.as_bytes());
}

fn sleep(seconds: &str) {
    let seconds: f64 = seconds.trim().parse().expect("sleep SECONDS");
    thread::sleep(Duration::from_secs_f64(seconds));
}

fn exit(code: i32) -> ! {
    let _ = io::stdout().flush();
    process::exit(code)
}

fn fail(message: &str) -> ! {
    err(&format!("harness-fake: {message}\n"));
    process::exit(101)
}
