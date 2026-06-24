# Changelog

All notable changes to Helm are documented here. The format loosely follows
[Keep a Changelog](https://keepachangelog.com); versions follow
[Semantic Versioning](https://semver.org).

## [0.4.1] — 2026-06-24

### Fixed

- Codex's task list and tools no longer disappear when its native hook becomes
  active. Those come only from Codex's rollout log (no hook event carries them),
  so for Codex the log watcher stays the source of truth while the hook just adds
  an earlier status nudge.

### Docs

- Rewrote the README (English + Korean) with a dedicated mobile-access guide and
  an updated agent-support matrix.

## [0.4.0] — 2026-06-24

Codex joins the native-hook pipeline (all three agents now push live state with
no polling), and the mobile UI gets a real phone layout.

### Added

- **Codex native hooks** — Codex now pushes its lifecycle events to Helm's
  localhost receiver through `~/.codex/hooks.json`, so its progress, activity, and
  completion appear instantly like Claude and opencode. Registration merges only
  the hook entries and leaves Codex's trust store untouched; Codex's own one-time
  trust prompt authorizes the new hook. This completes instant, poll-free tracking
  for all three agents.
- **Mobile phone layout** — on a phone the three-pane dashboard collapses to a
  single full-width column with the terminal/conversation as the focus; the
  session list and the session-status rail become slide-in drawers reached from a
  hamburger and a status button. Horizontal overflow is locked, the composer is
  zoom-safe on iOS, and modals become full-width sheets. The desktop layout is
  unchanged.

## [0.3.0] — 2026-06-24

Mobile access over the LAN, plus much more robust live agent tracking that no
longer depends on any plugin being installed.

### Added

- **Mobile (Phase A, LAN)** — a 📱 button shows a URL/QR for a phone on the same
  wifi. Helm serves the identical UI over HTTP and bridges it over a WebSocket:
  the phone drives sessions live (terminal, progress, tasks, conversation) with
  a per-launch pairing token. No new dependencies — a hand-rolled std HTTP + WS
  server, the same event stream the desktop uses.

### Changed

- **Robust agent tracking without any plugin** — Claude's transcript is now found
  by scanning all project dirs for the newest one whose recorded cwd matches the
  session (Claude often writes under a git-root slug, not the cwd slug), so
  progress/tasks/context work for vanilla Claude. OMC's HUD cache and the hook's
  exact transcript path are used as faster sources when present, never required.
- The task list re-syncs when an agent revises its plan (the change signature now
  tracks task text, not just status), and the right-rail task list scrolls.

### Fixed

- Codex's cursor no longer flickers while it works (its TUI repaints hard, so the
  cursor is pinned steady for Codex sessions).
- When an agent CLI exits back to the shell, the session drops the agent label and
  stops its watcher instead of staying stuck on the agent.

## [0.2.0] — 2026-06-24

Live agent state goes from log-tailing to native hooks, plus a redesigned tasks
board and a much richer composer.

### Added

- **Native-hook progress pipeline** — Helm runs a localhost receiver and gives
  each agent its own hook so progress / activity / **completion** push instantly
  with zero polling. Claude (project `.claude/settings.local.json`, never the
  global config) and opencode (an auto-loaded `~/.config/opencode/plugin/`
  bridge) are wired; Codex stays on the watcher.
- **Tasks board ("작업")** — reframed into a "현재 활성" overview: one card per
  running session with status, live activity, a per-second elapsed timer, and a
  context-usage bar. Codex `update_plan` and the OMC `TaskCreate`/`TaskUpdate`
  harness now feed the task list alongside Claude `TodoWrite`.
- **Keyboard shortcuts** — session nav (cycle / jump / new / close), Terminal ⇄
  Conversation, clear scrollback, font size, settings, notifications.
- **Composer quick controls** — model / agent / reasoning chips above the send
  button; Claude's model dropdown switches the model in one click.
- A session tab now shows an attention ring when its agent needs input.

### Changed

- Log watchers wake on filesystem events instead of polling, and bind to the
  exact transcript a hook reports — fixing Claude token-context tracking.
- Clipboard paste reads the real OS clipboard (text copied in other apps).

### Fixed

- Codex status was pinned to idle (wrong turn-event name) — it now shows live
  "working" + what it's doing.
- The agent watcher failed to start for restored / Helm-launched sessions, so
  progress / tasks / conversation stayed empty.
- `Ctrl`/`Cmd`+`C` copies the terminal selection (and still interrupts with no
  selection).

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

[0.4.1]: https://github.com/kalhintz/Helm/releases/tag/v0.4.1
[0.4.0]: https://github.com/kalhintz/Helm/releases/tag/v0.4.0
[0.3.0]: https://github.com/kalhintz/Helm/releases/tag/v0.3.0
[0.2.0]: https://github.com/kalhintz/Helm/releases/tag/v0.2.0
[0.1.0]: https://github.com/kalhintz/Helm/releases/tag/v0.1.0
