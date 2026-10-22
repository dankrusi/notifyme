# notifyme

Tiny CLI for LLM agents (and humans) to send a notification by Telegram or email.

```
notifyme telegram "Build finished"            # to your phone
notifyme email "Report ready" --subject "CI"  # to your inbox
notifyme email "Hi" --to someone@example.com  # to others (comma-separate for several)
echo "long output" | notifyme telegram        # message from stdin
notifyme install                              # credentials, PATH, agent skills
```

Prints `sent` and exits 0 on success; errors go to stderr with exit 1.

## Install

```
cargo build --release
target/release/notifyme.exe install
```

`install` asks only for missing settings, finds your Telegram chat id automatically
(just message your bot when prompted), offers to copy itself to `~/.notifyme/bin` and
add that to PATH, and installs the agent instructions for any detected agents:

- Claude Code: `~/.claude/skills/notifyme/SKILL.md`
- Codex: `~/.codex/skills/notifyme/SKILL.md`
- Gemini CLI: block in `~/.gemini/GEMINI.md`

For anything else, paste [AGENTS.md](AGENTS.md) into its instructions. Then you can just
say "notify me when done" (Telegram) or "email me the summary" (email).

## Config

`~/.notifyme/.env` (or the file in `NOTIFYME_CONFIG`). Any key can be overridden by an
env var `NOTIFYME_<KEY>`.

| Key | |
|---|---|
| `TELEGRAM_BOT_API_KEY` | bot token from @BotFather |
| `TELEGRAM_TO` | default chat id (bots can't message phone numbers) |
| `EMAIL_TO` | default recipient(s) |
| `SMTP_HOST`, `SMTP_PORT` | port defaults to 465 |
| `SMTP_SECURITY` | `tls` (default on 465), `starttls` (default otherwise), `none` |
| `SMTP_USERNAME`, `SMTP_PASSWORD` | |
| `SMTP_FROM`, `SMTP_FROM_NAME` | from defaults to the username |
