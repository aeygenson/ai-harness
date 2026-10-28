<p align="center">
  <img src="assets/icons/ai-harness-128.png" alt="AI Harness icon" width="128" height="128">
</p>

<h1 align="center">AI Harness</h1>

<p align="center">
  <b>A conductor for a team of AI coding agents.</b><br>
  It leads a task through Architect, Developer, Tester and Security roles,<br>
  with the Architect checking the result against its own design,<br>
  each played by a real console agent: Claude Code, Codex CLI, Antigravity CLI or DeepSeek Harness<br>
  (with DeepSeek's models or NVIDIA's free ones).
</p>

<p align="center">
  <a href="https://github.com/aeygenson/ai-harness/actions/workflows/ci.yml"><img src="https://github.com/aeygenson/ai-harness/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/aeygenson/ai-harness/actions/workflows/security.yml"><img src="https://github.com/aeygenson/ai-harness/actions/workflows/security.yml/badge.svg" alt="Security"></a>
  <a href="https://github.com/aeygenson/ai-harness/releases/latest"><img src="https://img.shields.io/github/v/release/aeygenson/ai-harness" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/Rust-2021-orange?logo=rust" alt="Rust 2021">
  <img src="https://img.shields.io/badge/Linux%20%7C%20macOS%20%7C%20Windows-supported-blue" alt="Linux, macOS, Windows">
</p>

---

## Why it exists

One AI agent working alone on a project tends to grade its own homework: it writes the code,
decides the code is fine, and moves on. Real teams do better because different people check
each other's work.

AI Harness gives AI agents the same structure. Instead of one long chat, a task goes through
a small team with clear jobs, written handoffs between them, and a human who approves the
plan. The Architect who wrote the plan also checks that the code really follows it, before
testing and once more at the very end. You can put a different agent (and a different subscription) in each role, so Claude
can design, Codex can build and DeepSeek can review, or any other mix.

The harness itself has **no agent loop and calls no model API**. It is the conductor, not
a musician: the agents do the work, and the harness decides who works next, starts each
agent with the right permissions, checks what it did, and keeps the history.

## How a task flows

```mermaid
flowchart LR
    T([Task]) --> A[Architect<br><sub>writes the design</sub>]
    A --> H{You<br>approve?}
    H -- "rework" --> A
    H -- "yes" --> D[Developer<br><sub>writes the code</sub>]
    D --> R[Architect<br><sub>review: code<br>follows the design?</sub>]
    R -- "yes" --> Te[Tester<br><sub>writes and runs tests</sub>]
    Te --> S[Security<br><sub>read-only review</sub>]
    S --> F[Architect<br><sub>final review,<br>security of the design</sub>]
    F -- "yes" --> Done([Done])
    F -- "task asks you<br>to check it" --> Acc{You<br>accept?}
    Acc -- "yes" --> Done
    R -. "gap" .-> D
    Te -. "bug" .-> D
    S -. "fix" .-> D
    F -. "gap" .-> D
    D -. "design unclear" .-> A
    Te -. "design problem" .-> A
    S -. "design problem" .-> A
```

The Architect works three times in a task, always as the same role:

1. **Design.** It reads the project and writes the design to `docs/`. The design always goes
   to you; nothing is built until you approve it.
2. **Review.** When the Developer is done, the Architect compares the code with the approved
   design: every step built, interfaces as designed, nothing out of scope. A gap sends the
   work back to the Developer; a design that turned out wrong comes to you.
3. **Final review.** After the Tester and Security, the Architect looks at the whole result
   once more, including their fixes and the security of the design (input checked where it
   enters, secrets out of code and logs, no wider permissions than the task needs). Only
   this step can finish a task. When the task or the design asks for a step only you can
   do (an acceptance, a live check, a demo), the final review hands the result to you
   instead, with notes on what to check, and the task is done when you accept it.

Every fix the Developer makes goes through the same review and the checks after it again.

Each role finishes by writing short `notes.md` and a structured `handoff.json`: a verdict,
the files it touched, the issues it found with their severity, and who it thinks should work
next. The harness then **trusts, but verifies**:

- **Routes are checked.** A role only suggests the next step; the harness accepts it only if
  that move is in the allowed table (for example, the Tester may send work back to the
  Developer, an Architect's design always goes to you first, and only the Architect's final
  review can finish a task). The harness remembers which of its three jobs the Architect is
  doing, so a design can never skip your approval.
- **File scope is checked.** The Architect may change only `docs/`, the Tester only test
  files, Security nothing at all. Files that steer agents (`.claude/`, `AGENTS.md`,
  `CLAUDE.md`, `.git/` and others) are off limits to every role, at any depth and in any
  letter case.
- **Tampering is caught.** If an agent commits by itself, the run stops. If it edits
  `.git/config` (a classic way to make git run a command later), the harness restores the
  old settings and stops. Git hooks of the project never run on the harness's commits.
- **Every role is one git commit**, with its code, its handoff and its notes, so the whole
  history of a task is readable in git.
- **Loops are bounded.** After five rounds of rework, or whenever a role says
  `needs_human`, the task stops and waits for you.

## Features

| | |
|---|---|
| **Four agents, one interface** | Claude Code, OpenAI Codex CLI, Google Antigravity CLI and DeepSeek Harness (with DeepSeek's API or NVIDIA's free models), behind one Rust trait (plus a mock agent for tests). Choose an agent, model and effort level per role, per project. |
| **Full-screen terminal UI** | `harness tui` (Ratatui) with tabs for Tasks, Roles, Skills, MCP, Plugins, Retro, Agents and Projects. Works with mouse and keyboard, in English and Russian, with five colour themes. |
| **Skills** | Each role gets built-in instructions that work with any language or framework. Projects keep their own copies and add their own skills, shared by all agents. |
| **MCP servers and plugins** | Give a role extra tools: MCP servers (local or on the web, with OAuth sign-in) and agent plugins from plugin catalogs. |
| **Retrospective** | `harness retro` counts what happened across tasks (rounds, rejections, repeated issues, unused skills) without AI, then can ask an agent to propose skill changes that you approve one by one. |
| **Watch from your own AI** | `harness mcp-serve` is a read-only MCP server: connect Claude Code, Codex or Gemini to it and ask what your tasks are doing and why. It cannot change anything; decisions stay in the TUI. |
| **Agent manager** | Install, update, remove and sign in to agents from the Agents tab. `harness doctor` checks that Git, Node.js and at least one agent are ready. |
| **Isolated agents** | Every project configures its agents itself: no personal plugins, MCP servers or saved keys leak in from your own setup. |
| **Secrets handled carefully** | Logins and keys live in `~/.harness/credentials/` (readable only by you), never in the project, git, logs or an agent's environment, and are shown as `***` in every log. |
| **Runs everywhere** | Linux, macOS (Apple silicon and Intel) and Windows, with one-command installers and ready-made binaries. |

## Quick start

**macOS or Linux**

```bash
curl -fsSL https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.sh | bash
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.ps1 | iex
```

The installer adds what is missing (Git, Node.js, the Zed editor) and leaves what is already
there as it is, so a second run takes seconds; the `harness` program is always the newest
release. It also adds **AI Harness** to your applications. After that the
harness updates itself: the TUI shows its version and offers a newer release in its top bar
(«Update Harness» on the Agents tab), and `harness update` does the same from the command
line. Agents are installed and signed in on the **Agents** tab, using the subscriptions you
already have.

To remove the harness, run `install/uninstall.sh` (macOS, Linux) or `install/uninstall.ps1`
(Windows) the same way as the installer. Your projects and `~/.harness` (logins, settings)
stay, so installing again finds everything; add `--purge` (Windows:
`$env:HARNESS_PURGE = "1"` first) to remove `~/.harness` too.

Then open the interface:

```bash
harness tui
```

Everything can be done from the TUI. The same steps from the command line:

```bash
cd ~/code/my-project                 # any git repository
harness init                         # create .harness/harness.toml
harness github vgs-prototype         # put the project on GitHub (private repository)
harness task new task-001 "Add a CSV export to the report page"
harness run task-001                 # Architect writes the design, then waits for you
harness approve task-001             # Developer, Tester and Security take over,
                                     # with the Architect checking the code twice
harness status task-001              # where the task is and its history
harness retro task-001               # what went well and what to improve
```

Building from source needs Rust: `cargo build --release -p harness-cli`, or run the
installer with `bash -s -- --dev`. A harness built from source updates itself from its git
folder: «Update Harness» (or `harness update`) runs `git pull --ff-only` and builds it again
whenever the branch on GitHub has new commits.

## Architecture

A Rust workspace of six crates. The engine is synchronous and has no UI, so it is easy to
test and does not care whether the CLI or the TUI drives it, or which agent runs.

```
 harness-cli ──┐                 ┌── harness-tui
 (clap)        │                 │   (ratatui)
               ▼                 ▼
            ┌─────────────────────────┐
            │       harness-core      │  tasks, routes, handoffs, checks,
            │                         │  config, skills, MCP, retro, git
            └────────────┬────────────┘
                         ▼
            ┌─────────────────────────┐
            │     harness-agents      │  adapters: Claude Code, Codex,
            │      (tokio)            │  Antigravity, DeepSeek, mock
            └────────────┬────────────┘
                         ▼
            ┌─────────────────────────┐
            │    harness-platform     │  everything that differs between
            │                         │  Linux, macOS and Windows
            └─────────────────────────┘
```

`harness-fake` is a tiny test-only program that stands in for an agent, `curl` or an MCP
server identically on all three systems.

More detail: [design](docs/design.md) and [handoff format](docs/handoff-format.md)
(both in Russian).

## Built with AI, on purpose

This project is also an experiment in **how to build real software together with AI
agents**, and a record of what that looks like when it is done carefully.

- **Designed before coded.** Every part started as a written design, discussed and approved
  before any code was written ([docs/design.md](docs/design.md) keeps the decisions).
- **AI writes, a human decides.** The code was written with Claude Code in small pull
  requests, about 120 of them. Each one was reviewed and merged by a person, and each one
  had to pass CI on Linux, macOS and Windows.
- **Rules for agents live in the repo.** [AGENTS.md](AGENTS.md) tells every coding agent how
  to work here: readable code for a Rust learner, no `unwrap()` in production paths, no
  secrets anywhere, a test for every behaviour, files of about 300 lines at most. It also
  adopts Microsoft's [Pragmatic Rust Guidelines](docs/rust-guidelines.md).
- **The machine checks the rules.** Clippy runs with the `pedantic` group and selected
  `restriction` lints as errors, `missing_docs` fails the build, and `cargo deny` checks
  every dependency for known vulnerabilities every day.
- **Tested without the network.** Around 380 tests use a mock agent, a fake program and
  temporary git repositories, so they run the same on every system.
- **Used for real.** The harness has driven its own agents to build other projects, such as
  a Solana escrow program written entirely by the roles.

The harness applies the same idea to every project it runs: AI does the work, structure
and checks keep it honest, and a person stays in charge of the decisions that matter.

## Status

Version 0.9.0. The core flow, four agents, the TUI, MCP, plugins, retrospectives and the
installers all work and are used on Linux. macOS and Windows pass CI; live testing on
those systems is in progress.
