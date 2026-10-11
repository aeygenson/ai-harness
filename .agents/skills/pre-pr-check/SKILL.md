---
name: pre-pr-check
description: Checks your own diff before a commit or pull request for weakened checks, untested conditions and risky dependency updates. Use before every push in this repository.
---

# Pre-PR check

Based on `constraint-driven-development` and `code-review-and-quality` from
addyosmani/agent-skills (MIT). It adds to the checklist in AGENTS.md (fmt,
clippy, tests); it does not replace it.

## 1. Did I lower the bar to get green?

Read `git diff origin/main...` (and new untracked files) and look for:

- a test skipped or removed: `#[ignore]`, a deleted test file, a test that
  lost its `assert!`/`assert_eq!` lines;
- a lint switched off: a new `#[expect(...)]` (it needs a real `reason`) or
  any `#[allow(...)]` (AGENTS.md forbids it);
- unfinished work: `todo!()`, `unimplemented!()`, an error turned into
  silence (`let _ =` on a `Result` that matters, `.ok()` that drops an error
  the user should see);
- a limit or constant made looser only so a test passes.

Each one needs a reason in the commit message or the PR, or it goes.

## 2. Can the new tests fail?

For one or two conditions the change added (the riskiest ones), turn the
condition around (drop a `!`, swap `&&` and `||`, change `<` to `<=`), run
the tests of that crate, then put the code back (`git checkout -- <file>`, or
keep a copy). If the tests still pass, a test is missing: write it, and see
it fail on the turned-around code.

## 3. Dependencies

If `Cargo.toml` or `Cargo.lock` changed: one dependency per change, read its
changelog (not just the version number), look at the `Cargo.lock` diff for
crates nobody chose, and run `cargo deny check` if it is installed.

## Done when

Every finding from step 1 has a reason or is gone, the mutation in step 2
made a test fail, and the PR description names anything you kept on purpose.
