# Agent Guidelines for the AI Harness

Rules for every AI coding agent (Claude Code, Codex, Antigravity, DeepSeek Harness, ...) and
every contributor working on **this repository**. They are adapted from a general Rust
guidelines file: the rules that fit this project are kept, the ones that do not apply here
(web front ends, WASM, `axum`, `polars`, PyO3) are left out, and a few are changed on
purpose. Where a rule below differs from common Rust advice, the reason is given.

## What this project is

- A Rust program that leads a task through four roles (Architect, Developer, Tester,
  Security) by running ready-made console agents. The harness is a **conductor**: it has no
  agent loop of its own. Design: `docs/design.md`; the handoff file: `docs/handoff-format.md`
  (both in Russian).
- It is a **learning project**. Lisa, the owner, is learning Rust. Code must be easy for a
  Rust learner to read: plain names, small functions, a short comment where Rust does
  something surprising (ownership, lifetimes, `?`, trait magic, `async`). No clever tricks.
- It runs on Linux, macOS and Windows (CI checks all three).
- It must stay **technology-neutral**: the target projects the harness works on can be of any
  kind, so built-in skills and prompts never assume Rust, a language or a framework.

## Workspace layout

| Crate | What lives there |
|---|---|
| `harness-platform` | Everything that differs between Linux, macOS and Windows. No dependencies. |
| `harness-core` | The engine: config, handoffs, routes, git, skills, MCP config, retro. No `tokio`, no UI. |
| `harness-agents` | Adapters that start each agent; process control; MCP check/OAuth/bridge. |
| `harness-tui` | The full-screen Ratatui interface. |
| `harness-cli` | The `harness` program (`clap`). |

- Put OS-specific code (`#[cfg(unix)]`, `#[cfg(windows)]`) in `harness-platform`. Outside it,
  `#[cfg(...)]` is fine only in tests (for example a shell script standing in for an agent).
- Keep `harness-core` free of `tokio` and of UI. It may declare `async fn` that only wait for
  an agent (the `AgentRunner` trait and the run loop in `orchestrator`); everything else in
  it is synchronous. Code that needs a runtime, timers or processes goes in
  `harness-agents`, `harness-tui` or `harness-cli`.
- The built-in role skills are `crates/harness-core/skills/*.md` (embedded with
  `include_str!`).

## Core principles

- **Clarity first, then performance.** Write the simple, correct version. Optimise only where
  it matters (a hot loop, a large file, a slow command) and say why in a comment. Big-O still
  matters: do not read a file twice or scan a list inside a loop over the same list.
- **No extra code.** Solve the task asked, nothing more. No speculative options, no dead code,
  no commented-out code.
- **Reuse before writing.** Look for an existing helper in the workspace first (for example
  `ui::selector` in the TUI, `config_edit` for changing `harness.toml`).
- **New crates need a reason.** Every dependency is checked by `cargo deny` (`deny.toml`:
  crates.io only, no known vulnerabilities, no unmaintained crates). Add a crate only when it
  removes a lot of code or risk, is small and well maintained, and say why in the PR.
- **Ask before big changes.** Design changes, new dependencies and changes to file formats
  (`harness.toml`, `handoff.json`) are discussed with Lisa first.

## Looking up library documentation (Context7)

Do not rely on memory for crate APIs: `ratatui` 0.30, `clap` 4, `toml_edit`, `tokio` and
others change between versions. When the **Context7** MCP server is available, use it:
first `resolve-library-id` (for example `ratatui`), then `query-docs` with the exact
question. Check the version in the crate's `Cargo.toml` and ask for docs of that version.
Without Context7, read the docs on docs.rs for the version in `Cargo.lock`.

## Microsoft Pragmatic Rust Guidelines

`docs/rust-guidelines.md` is a copy of Microsoft's Pragmatic Rust Guidelines (MIT license).
Follow them where they fit this project; where they differ from this file, **this file
wins**. In short:

- **Follow**: M-DESIGN-FOR-AI, M-APP-ERROR, M-FIRST-DOC-SENTENCE, M-MODULE-DOCS,
  M-DOCUMENTED-MAGIC, M-LINT-OVERRIDE-EXPECT, M-PANIC-IS-STOP, M-PANIC-ON-BUG,
  M-PUBLIC-DEBUG, M-PUBLIC-DISPLAY, M-CONCISE-NAMES, M-STATIC-VERIFICATION,
  M-UPSTREAM-GUIDELINES, M-UNSAFE, M-UNSOUND, M-AVOID-STATICS, M-STRONG-TYPES,
  M-NO-GLOB-REEXPORTS, M-ESSENTIAL-FN-INHERENT, M-REGULAR-FN, M-SIMPLE-ABSTRACTIONS,
  M-AVOID-WRAPPERS, M-DI-HIERARCHY, M-INIT-BUILDER, M-TEST-UTIL.
- **Replaced by this file**:
  - M-CANONICAL-DOCS: `# Errors` / `# Panics` sections only for complex functions (see
    Documentation).
  - M-ERRORS-CANONICAL-STRUCTS: errors are `thiserror` enums; matching on variants is easier
    for a learner than an error struct with a `kind()`.
  - M-LOG-STRUCTURED: `tracing` is not used yet.
  - M-MIMALLOC-APPS: the harness waits for agents, it does not compute; no new allocator.
- **Not relevant here**: FFI and `-sys` crates, rules for published libraries and their
  feature flags, and the performance rules (M-HOTPATH, M-THROUGHPUT, M-YIELD-POINTS).
- Lint overrides use `#[expect(lint, reason = "...")]`, not `#[allow(...)]`: the build then
  says when the override is no longer needed.

## Preferred tools and libraries

These are what the project already uses; prefer them over alternatives.

- `cargo` for building, testing and dependencies.
- `serde` + `serde_json` for JSON, `toml` for reading TOML, `toml_edit` for changing
  `harness.toml` without losing Lisa's comments and order.
- `clap` (derive) for the command line.
- `ratatui` + `crossterm` for the TUI.
  - **Everything must work with both mouse and keyboard**: click, double click, wheel.
  - **Always account for scrolling offsets** when turning a click position into a list row.
    Register clickable areas through the TUI's existing hit map rather than computing
    positions by hand.
  - Every visible string goes through the translation in `i18n.rs` (English and Russian).
  - Check new screens against all five themes.
- `tokio` only in the async crates, with the smallest feature set that works.
- `tempfile` for temporary files and folders in code and tests.
- Messages for the user: the CLI prints with `println!`/`eprintln!` (this is the program's
  output, not debugging). `tracing` is not used yet; do not add it without a reason.

## Code style

- `rustfmt` defaults: 4 spaces, 100 columns. Run `cargo fmt --all`.
- Names: `snake_case` for functions, variables, modules; `PascalCase` for types and traits;
  `SCREAMING_SNAKE_CASE` for constants. Use full, descriptive names.
- Follow the Rust API Guidelines and idiomatic Rust.
- **Comments are written in English**, in plain words. Explain *why*, and Rust nuances a
  learner might miss. Do not write comments that only repeat the code, and do not leak the
  prompt or task text into comments.
- **Symbols in the TUI are allowed.** The interface uses marks such as `✓`, `✗`, `●`, `→`
  and box-drawing characters on purpose; keep them consistent with the existing screens.
  Do not use emoji (pictures like 🚀) in code, comments, commit messages or output.

## Documentation

- Every module starts with a `//!` comment saying what it is for.
- Every public item (function, struct, enum, method, important field) has a `///` comment:
  one or two sentences on what it does, plus what is not obvious (errors, side effects such
  as writing files or starting processes, which OS behaves differently).
- `# Arguments` / `# Returns` / `# Errors` sections and examples are welcome for complex
  functions, but not required for simple ones. A long template on a three-line function hurts
  readability more than it helps.
- The first sentence of a `///` comment fits on one line (about 15 words): it is what lists
  and tooltips show.
- Magic values (a size limit, an error code, a width) are named constants with a comment
  saying why that value.
- When you change behaviour, update the comment, `docs/` and the help text in the same PR.

## Types and error handling

- Use the type system to prevent mistakes: enums instead of string flags, `Option<T>`
  instead of sentinel values, newtypes where two values of one type are easy to mix up.
- Fallible operations return `Result`. Propagate with `?`.
- Errors:
  - `harness-core` defines its errors with `thiserror`.
  - `harness-cli` and `harness-tui` use `anyhow` with `.context(...)` for application-level
    errors.
  - Many existing functions return `Result<T, String>` with a ready message for Lisa. That is
    acceptable where the error is only shown, never matched on. New code where the caller
    must react to different kinds of error should use a `thiserror` enum.
- Error messages tell the user what happened and what to do next, in plain words.
- **No `.unwrap()` in production code paths.** Use `?`, a clear error, or `.expect("...")`
  only for a true invariant, with a message that says what was expected. `.unwrap()` is fine
  in tests. (Existing non-test unwraps: fix them when you touch the code.)
- No `unsafe`. If it ever seems needed, ask first and document the safety invariant.

## Functions, structs and enums

- One responsibility per function and per type.
- Prefer borrowing (`&T`, `&str`, `&[T]`) over taking ownership; call `.clone()` explicitly
  and only when needed.
- Five parameters at most; beyond that, use a struct.
- Return early to reduce nesting. Prefer `if let`/`let else` for one pattern, exhaustive
  `match` for enums (avoid `_` catch-alls so a new variant breaks the build where it must be
  handled).
- Use iterators and adapters (`map`, `filter`, `enumerate`) when they are clearer than a loop;
  a plain `for` loop is fine when it reads better for a learner.
- Every public type implements `Debug` (types holding secrets write their own that prints
  `***`). Types shown to the user implement `Display` instead of being printed with `{:?}`.
- Derive `Clone`, `PartialEq` (and `Default` where sensible) where they make sense.
- Avoid global state (`static` with a `Mutex` inside): pass what a function needs.
- **File size**: at most about 300 lines of production code per file. Unit tests inside the
  file (`#[cfg(test)] mod tests`) do not count. When a file grows past that, split it by
  topic into modules; the file must stay quick to read.
- Avoid unnecessary allocations (`&str` over `String`, `Vec::with_capacity` when the size is
  known), but do not trade readability for micro-optimisations.

## Concurrency

- Async with `tokio` for running agents and other processes; the core does not use `tokio`.
- Stop child processes together with everything they started (see
  `harness_platform::process`); never leave an agent running after the harness stops.
- Prefer message passing (channels) over shared mutable state.

## Security

This program starts AI agents with access to the user's files, so security rules are strict.

- **Never put secrets in code, logs, commits, test output or command lines.**
- Secrets live in `~/.harness/credentials/` with mode 600 (Windows: only the user may read);
  use the helpers in `harness_platform::private` and `harness_core::secret`. Do not use
  `.env` files.
- **Never pass a secret through an agent's environment**: agents can read their own
  environment from shell commands. Use private temporary files outside the project, deleted
  when the role ends. The one exception is Claude Code, which takes its login only from
  `CLAUDE_CODE_OAUTH_TOKEN`; there the harness sets `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB`, so
  Claude Code removes the token from every command it runs (on Linux this needs bubblewrap).
- Secrets are replaced with `***` in `agent.log`, messages and the live log; keep it that way
  when adding new output (`harness_core::secret::hide`; do not write another one).
- MCP keys and OAuth tokens go only into the MCP server's own settings (a private temporary
  file), never into the agent's environment, the project, git or a handoff.
- Know the limit: an agent runs as the same user, so a role with a shell (Developer, Tester)
  could still read a private file if it tried. Only an operating-system sandbox would stop
  that; until there is one, do not claim that an agent cannot see a secret.
- Never weaken the role permissions (`harness_core::permissions`, deny rules, temporary HOME
  for Antigravity) without discussing it.
- Treat everything an agent or an MCP server returns as untrusted input: strip control
  characters before showing it (`harness_core::text::safe` / `safe_line`; do not write
  another cleaning helper), validate `handoff.json` before using it.

## Testing

- Every new function or behaviour gets a unit test (`#[cfg(test)] mod tests` in the same
  file, `#[test]`, Arrange-Act-Assert). Bug fixes get a test that fails without the fix.
- External things are faked: agents with the mock adapter or a small script, the file system
  with `tempfile`, no network in normal tests.
- Tests make real git commits in temporary repositories, so git needs a user name and email.
- Tests that need real agents or the network are `#[ignore]` (see
  `crates/harness-agents/tests/live_install.rs`) and run only on request.
- **Never touch `~/code/harness-test` or the real `~/.harness`** in tests or experiments;
  work on copies in a temporary folder.
- Do not commit commented-out or skipped tests.

## Imports and dependencies

- No wildcard imports, except `use super::*;` in test modules and real preludes.
- Order: standard library, external crates, workspace crates, local modules
  (`rustfmt` keeps each group sorted).
- Every dependency has a version constraint in `Cargo.toml`.
- Do not read `Cargo.lock` unless the task is about a dependency version: it is large.

## Version control

- Work on a branch and open a pull request; Lisa reviews and merges.
- Clear commit messages in English that say what changed and why.
- No commented-out code, no `dbg!`, no debugging `println!`, no credentials.

## Before committing

Run exactly what CI runs:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] If `install/install.sh` changed: `bash -n install/install.sh`
- [ ] New public items have doc comments; changed behaviour is reflected in `docs/`
- [ ] No commented-out code, debug output or secrets

---

**Remember:** this is a learning project. Prefer code Lisa can read and understand over code
that is clever.
