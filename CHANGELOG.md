# Changelog

All notable changes to Helm are documented here. The format loosely follows
[Keep a Changelog](https://keepachangelog.com); versions follow
[Semantic Versioning](https://semver.org).

## [0.1.0] — 2026-06-24

First public release. A native, cross-platform dashboard terminal that runs AI
coding agent CLIs inside real terminals and surfaces each session's live state
beside the terminal.

### Added

- **Three-pane dashboard** — sessions grouped by project (left), terminal with
  per-agent tabs (center), live agent state (right).
- **Agent harmonization** — detects which agent is running (typed command +
  terminal title) and tails its own logs:
  - **Claude Code** — full fidelity from `~/.claude` transcripts.
  - **Codex** — from `~/.codex` rollout logs.
  - **opencode** — status, activity, and file operations from the disk log,
    plus the configured MCP servers / plugins.
- **Conversation view** — read-only live transcript (user/assistant cards,
  collapsible reasoning, tool calls, code blocks) for Claude and Codex.
- **Tasks view (작업)** — the agent's task list with statuses, across all sessions.
- **Per-session working folder** — choose the directory a new session starts in,
  by path input or native folder browse.
- **Session auto-restore** — sessions (working dir + agent) come back after a
  restart or reboot; Claude resumes with `--continue`.
- **Clipboard** — Ctrl/Cmd+V and Shift+Insert to paste, Ctrl+Shift+C / Cmd+C to
  copy; `Ctrl+C` still interrupts the agent.
- **Keyboard shortcuts** — session navigation (cycle / jump / new / close),
  Terminal ⇄ Conversation toggle, clear scrollback, font size, settings, and
  notifications; chosen so the terminal keeps its own bindings.
- **Tab attention ring** — a session tab lights up when its agent needs input.
- **WebGL terminal renderer**, live system CPU/MEM, and settings for terminal
  font, default agent, panel visibility, and restore.

### Platforms

- **Windows** (WebView2), **macOS** (WKWebView), **Linux** (WebKitGTK), verified
  by a Windows/macOS/Linux matrix build in CI.

### Notes

- Built on Tauri (Rust) + the system webview + ConPTY/PTY. No Electron.
- MIT licensed.

[0.1.0]: https://github.com/kalhintz/Helm/releases/tag/v0.1.0
