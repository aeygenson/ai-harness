---
description: Checklist for code that talks to another program over a protocol (JSON, HTTP, pipes).
---
# Protocol attacks

Try:
- malformed JSON, missing fields, extra fields, wrong types;
- a huge message, a flood on stderr;
- a reply with the wrong id, a duplicate reply, replies out of order;
- the other side closing the connection half-way, or never answering;
- an error reply where success was expected.
The code must reject what it does not understand instead of guessing, and
must not hang forever.
