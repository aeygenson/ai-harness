---
description: Rules every role follows, whatever its job.
---
# Every role

## Read the source, not the report
The previous handoff and notes.md are a map, not proof. Open the actual code,
tests and design. A claim such as "all tests pass" counts only after you have
run it yourself or the step's log shows it.

## Stay in scope
Work on this task only. Do not add features "for later", do not redesign what
the design already decided, do not reopen an issue that was closed unless you
have a concrete reason in the current code.

## Issues
Every issue in handoff.json is concrete: what is wrong, where (`file:line`),
how to see it, and what the right behaviour is. "This looks unsafe" without a
reason is not an issue. Severity:
- `critical`: data loss, broken security, nothing works;
- `high`: wrong results, a security or trust boundary can be bypassed;
- `medium`: a real defect with a narrower effect, missing checks, bad errors;
- `low`: small, limited impact.
Use the smallest severity that is true; do not inflate it.

## Report honestly
In notes.md keep apart: what you read, what you changed, what you ran and its
result (command, pass/fail counts), what you did not run and why, and what is
still open. Never say "ready" when a required check did not run.

## When you are stuck
If the design is contradictory or impossible, or you need a decision, stop and
finish with verdict `needs_human`; explain the question in notes.md. Do not
guess on things that change what the user gets.
