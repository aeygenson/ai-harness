---
description: Checklist for code that reads, writes or trusts files and paths.
---
# Filesystem attacks

For code that trusts files or paths, try as applicable:
- a symlink as the final name, as a parent folder, as a nested parent;
- a hardlink to another file;
- a FIFO, device or socket instead of a regular file;
- relative paths, `..`, non-canonical paths;
- the file or a folder replaced while the code works with it, and put back;
- wrong owner, permissions or link count;
- unexpected extra files in a folder;
- a crash before and after an atomic rename; half-written temporary files.
Checking only the final name is not enough when a parent folder can be a link.
