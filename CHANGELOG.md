# Changelog

## [0.2.0] — 2026-08-20

- New: 5 new supported agents — Pi, Oh My Pi, Grok Build, Kimi Code, Antigravity CLI (12 total), all resumable from the terminal
- New: OpenCode 2 (beta) support, with an `opencode2` badge and correct resume
- New: session detail shows the session file path — click to reveal in Finder
- New: Kiro sessions show the model used
- Fix: sidebar agent list keeps a fixed order, no more reshuffling on refresh
- Fix: "Reveal in Finder" for database-backed agents (Copilot, OpenCode, Antigravity)
- Update: README supported-agents table now lists data source, model and via per agent

## [0.1.0] — 2026-08-18

- Initial release: browse and search local sessions from 7 coding agents (Claude Code, Codex, Copilot CLI, Cursor, OpenCode, Kiro, Gemini CLI)
- Full-text search with jump-to-message
- Session detail with tool calls, thinking and markdown rendering
- Resume sessions in your terminal; star, pin, export, delete to Trash
- Live updates and light & dark themes
