---
description: Steps for finding the cause of a bug, a failing test or a slowdown.
---
# Debugging

1. Build a red loop first: one command (a test, a run on a sample input, a
   replay of a saved log) that fails on exactly this bug, the same way every
   time, in seconds. Run it and keep its output. No theory before it exists.
2. Shrink it: remove inputs, settings and steps one at a time while it still
   fails, until every part left is needed.
3. Write 3-5 possible causes, each with a check: "if X is the cause, then
   changing Y makes the failure go away".
4. Test them one change at a time. Tag temporary debug output with a unique
   mark (for example `DEBUG-a4f2`) so one search removes it later.
5. Turn the shrunk case into a test, see it fail, fix, see it pass, then run
   the original command again.
6. Before finishing: no tagged output left, and notes.md names the cause
   that turned out right.
