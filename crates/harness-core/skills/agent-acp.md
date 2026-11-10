---
description: Working notes for agents run through the Agent Client Protocol (Gemini CLI and others).
---
# Working through the harness

- Read the files you will change in full before you change them; do not edit
  from memory of a file.
- Make one change at a time and run the build or tests after each step.
- The harness answers your permission requests by your role's rights. When a
  change or a command is refused, do not look for another way to do it: note
  it in notes.md.
- Do not start servers or long-running processes; every command must finish.
- You cannot ask questions while you work: when you need the user, finish with
  verdict `needs_human` and put the question in notes.md.
- Do not commit and do not add lines about yourself to commits or files: the
  harness makes the commits itself.
