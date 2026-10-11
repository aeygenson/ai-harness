---
name: doubt-review
description: Adversarial review of a risky change in a fresh context before it stands. Use for changes to role permissions, secrets, the tripwire, process stopping, file formats (harness.toml, handoff.json), or any claim like "this is safe" or "this cannot happen".
---

# Doubt review

Based on `doubt-driven-development` from addyosmani/agent-skills (MIT).

A long session turns guesses into "facts" without anyone noticing. Before a
risky change stands, let a reviewer with a clean context try to break it.

## When

Use it when the change touches a trust boundary or is hard to undo:
`harness_core::task::permissions`, `tripwire`, `secret`, `harness_platform`
process handling, the formats of `harness.toml` or `handoff.json`, or when you
are about to write "safe", "cannot happen", "always" about non-trivial code.
Not for renames, formatting, docs or a one-line fix with an obvious result.

## Steps

1. **Claim.** Write in two lines what you believe and why it matters
   ("The Tester cannot commit a file outside `tests/`; otherwise it could
   change the code it checks"). Keep it to yourself.
2. **Extract.** Take the smallest unit a reviewer can hold in one read: the
   diff or the function (the artifact), and what it must satisfy: the design,
   the rule in AGENTS.md, the issue (the contract). If it is a 500-line diff,
   split it first.
3. **Doubt.** Give a reviewer with a fresh context the artifact and the
   contract only, never your claim or your reasoning: a reviewer that sees
   your conclusion tends to confirm it. In Claude Code use a subagent; in
   Codex start a separate read-only run (`codex exec --sandbox read-only`)
   with the prompt in a file passed through stdin. Prompt:

   > Adversarial review. Find what is wrong with this artifact. Assume the
   > author is overconfident. Look for unstated assumptions, unhandled edge
   > cases, hidden shared state, ways the contract can be broken, conventions
   > of this repository it breaks, and failures on unexpected input (Linux,
   > macOS and Windows). Do not praise or summarise. Find issues, or say
   > clearly that you found none after a careful look.

4. **Reconcile.** The reviewer's answer is data, not a verdict. Re-read the
   code for each finding and put it in the first class that fits:
   - the contract was unclear: fix the contract, review again;
   - a real problem: change the code, review again;
   - a real trade-off: keep it and say so in the PR description;
   - noise (the reviewer lacked context): note it and move on.
5. **Stop** when a round brings only trivial or known findings, or after
   three rounds. Serious findings after three rounds mean the change is not
   ready: say so to Lisa instead of starting a fourth.

## Signs you are doing it wrong

- The reviewer got your claim, or a question like "is this good?".
- Two rounds with real findings and none classified as a real problem: that
  is approving, not doubting.
- You accepted a finding without re-reading the code it points at.
