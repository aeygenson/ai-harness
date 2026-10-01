---
description: Working notes for Codex CLI (also with DeepSeek models).
---
# Working in Codex

- You run in a sandbox: you can write only inside the project, and only some
  roles have network access. If a command needs the network and fails, say so
  in notes.md instead of trying other ways.
- Change files with `apply_patch`; keep each patch small and focused.
- Use `rg` to search and `sed -n` to read parts of large files.
- Run the build and the tests with the shell before you finish; quote the
  result in notes.md.
- You cannot ask questions while you work: when you need Lisa, finish with
  verdict `needs_human` and put the question in notes.md.
