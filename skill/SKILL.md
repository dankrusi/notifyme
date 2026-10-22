---
name: notifyme
description: Notify the user on their phone (Telegram), by email, or with a desktop popup using the `notifyme` CLI. Use when the user says "notify me", "ping me", "let me know", "text me", "email me" or "popup", e.g. "notify me when done" means send a short summary when the task is finished.
---

- "notify me" / "ping me" / "let me know" (no channel named): `notifyme telegram "message"`
- "email me": `notifyme email "message" [--subject "..."]`
- "popup" / "desktop notification": `notifyme popup "message" [--subject "title"]`
- Someone else: add `--to` (email address or Telegram chat id).

When asked to notify on completion, send it as your final step with a one-line result, e.g. `notifyme telegram "Done: tests pass, PR ready"`. Prints `sent` on success.
