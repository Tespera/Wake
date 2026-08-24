# Changelog

## [0.2.5] — 2026-08-25

- New: experimental Linux support — browse, search and resume sessions on Linux desktops; prebuilt arm64 packages (.deb and tar.gz) attached to the release
- New: on Linux, resume opens sessions in GNOME Terminal, Console, Konsole, Ghostty, kitty, Alacritty, WezTerm, Xfce Terminal or XTerm
- New: keyboard shortcuts follow the platform — ⌘ on macOS, Ctrl on Linux
- Fix: resume failure notices only say "copied to clipboard" when the copy really happened; otherwise the command is shown in the message

## [0.2.4] — 2026-08-24

- New: Session locations is now a full manager — every location, built-in or custom, can be edited, removed, or pointed at a different folder
- New: add custom session folders for any agent, like backups, synced copies, or non-standard installs
- New: Restore defaults brings all locations back to the built-in paths in one click
- Update: location rows open an edit form on click, with an agent picker and a folder browser
- Update: refined spacing, dialog styling and button alignment across the app
- Fix: the delete confirmation now shows real buttons — before, it could only be confirmed with the Enter key
- Fix: a session that exists in two locations no longer flips between copies; the newest copy wins
- Fix: deleted sessions stay deleted even when another location holds a copy of them
- Fix: sessions from a removed location leave the list right away

## [0.2.3] — 2026-08-24

- New: Session locations — a sidebar button listing every folder Wake reads, with per-location session counts; click a row to open it in Finder
- New: custom data locations are respected — `CODEX_HOME` for Codex, `XDG_DATA_HOME` for OpenCode
- Update: the refresh button moved to the sidebar footer
- Update: sidebar counts are now badges
- Fix: an agent installed while Wake is running now appears after a refresh, no relaunch needed

## [0.2.2] — 2026-08-22

- New: DeepSeek Harness (`dsh`) support — 13 agents total, resumable, with its compressed session logs read transparently
- Update: sidebar agent order
- Update: the Open In button now names the app it will open

## [0.2.1] — 2026-08-20

- Fix: resuming OpenCode sessions in your terminal now works (broken in the 0.2.0 build)

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
