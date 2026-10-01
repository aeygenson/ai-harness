---
description: Checklist for code with side effects that must survive a crash or restart.
---
# Crash and restart

For every action with a side effect (sending a request, writing a file,
starting a process, charging money):
- What is saved before the action, and what after it?
- If the program dies between the two, what does a restart do?
- Could a restart do the same action twice? If yes, that is a `high` issue
  unless the action is safe to repeat.
- "I don't know if it happened" must stay unknown, never become "it did not
  happen".
- Recovery must use what was saved, not what is in memory.
Write a test that stops at each saved step and restarts.
