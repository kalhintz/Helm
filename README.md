<div align="center">

# ⎈ Helm

**A lightweight, native, cross-platform dashboard-terminal that harmonizes with AI coding agent CLIs.**

Run Claude Code, Codex, or opencode inside Helm's terminals — and the dashboard
surfaces each session's live state (progress, tasks, tools, plugins, token
context) right beside the terminal. tmux/cmux for the agent era.

<sub>`Tauri (Rust)` · `WebView2 / WKWebView / WebKitGTK` · `ConPTY / PTY` · `xterm.js + WebGL` · no Electron</sub>

**English** · [한국어](README.ko.md)

<img src="docs/screenshot.png" alt="Helm screenshot" width="900">

</div>

---

## Why

Modern coding agents are CLIs with rich, structured output — task lists, tool
calls, token budgets, multi-step plans. A plain terminal flattens all of that
into a scrollback of text.

Helm runs the agent in a **real terminal** (so everything works exactly as it
does in your shell) **and**, in parallel, reconstructs that structure as a
**live dashboard** — without the agent knowing or caring. One window, many
agents, full situational awareness.

When an agent supports it, Helm registers a **native hook** so state pushes the
instant it changes — no polling. When it doesn't, Helm reads the agent's own
session logs on filesystem-event wake. Either way the dashboard stays live.

## What you get

A three-pane dashboard:

| Pane | Shows |
|---|---|
| **Left** | Every session, grouped by project, with a status dot and a live activity bar. |
| **Center** | Session tabs (per-agent icon) + the live terminal, with a **터미널 ⇄ 대화** (Terminal ⇄ Conversation) toggle that re-renders the transcript as message cards, and a composer with model / agent / reasoning quick controls. |
| **Right** | The active session's live state — progress, tasks, tools/plugins, token context — plus a per-session event timeline and live system CPU/MEM. |

## Features

| Feature | Description |
|---|---|
| **Instant live state** | Where the agent supports hooks, Helm registers one and state updates push the moment it changes — zero polling. Otherwise it tails the agent's logs on filesystem events. |
| **Agent harmonization** | Detects which agent is running (typed command + terminal title) and labels the session — even when launched by hand in a plain shell. |
| **Tasks board (작업)** | A "현재 활성" overview: one card per running session with status, live activity, a per-second elapsed timer, and a context-usage bar — plus the full task list per session. |
| **Conversation view** | Read-only render of the live transcript — user/assistant cards, collapsible reasoning, tool calls, code blocks. |
| **Composer quick controls** | Model / agent / reasoning chips above the send button; pick from a live list and it switches in place. |
| **Mobile access (LAN)** | Open Helm on your phone over the same wifi and drive sessions live — terminal, progress, tasks, conversation. See [Mobile](#mobile-access). |
| **Per-session working folder** | Choose the directory a new session starts in — paste a path or browse natively. |
| **Session auto-restore** | Sessions (working dir + agent) come back after a restart or reboot; Claude resumes with `--continue`. |
| **Clipboard** | Paste and copy in the terminal, including text copied from other apps (see shortcuts below). |
| **WebGL renderer** | Glyphs drawn on the GPU for snappy echo, with a clean fallback to the DOM renderer on context loss. |
| **Settings** | Terminal font, default agent, panel visibility, restore toggle. |

## Agent support

What Helm can surface depends on what each agent writes to disk or pushes
through a hook.

| Capability | Claude Code | Codex | opencode |
|---|:---:|:---:|:---:|
| Detection + labeling | ✓ | ✓ | ✓ |
| Status / activity | ✓ | ✓ | ✓ |
| Tasks (task list) | ✓ | ✓ | ✓ |
| Tools / plugins | ✓ | ✓ | ✓ (MCP/plugins) |
| Conversation view | ✓ | ✓ | — ¹ |
| Token context | ✓ | ✓ | — |
| Instant via native hook | ✓ | ✓ ² | ✓ |
| Source | `~/.claude` transcript + hook | `~/.codex` rollout log + hook | disk log + hook plugin |

¹ opencode keeps message bodies in a locked database, so Helm surfaces its
status, activity, file operations, and the configured MCP/plugin list rather
than full conversation cards.

² Codex's tasks and tools come from its rollout log (read on filesystem-event
wake, so already near-instant); its hook adds an earlier status / turn nudge.

> **Adding a new agent** is just writing one watcher (or hook handler) that emits
> Helm's normalized events — the UI renders any agent generically. See
> [How it works](#how-it-works).

## Mobile access

Helm can serve its full UI to a phone on the **same wifi** and bridge every
session live over a WebSocket — the same event stream the desktop uses.

**To connect:**

1. Click the **📱** button in the top bar. A dialog shows a **URL**, your LAN
   IP, and the two ports.
2. On your phone (same wifi), open that URL — or tap **Copy** and paste it into
   the phone's browser.
3. That's it. The phone now mirrors and drives your sessions live: terminal,
   progress, tasks, and conversation.

**On the phone layout**, the three panes collapse to a single full-width column
(the terminal/conversation is the focus). Two buttons in the top bar open the
side panels as drawers:

| Button | Opens |
|---|---|
| **☰** | The session list (left rail) |
| **📊** | The session-status rail (right) |

Tap the dimmed area, pick a session, or press `Esc` to close a drawer.

**Pairing & ports:**

| Detail | Value |
|---|---|
| Pairing | A fresh random token each launch is baked into the URL; the WebSocket rejects anything else. |
| HTTP port | `8787` — override with `HELM_HTTP_PORT` |
| WebSocket port | `8788` — override with `HELM_WS_PORT` |
| Scope | LAN only. No cloud, no relay — the phone talks straight to your machine. |

> If the phone can't load the page, your firewall is likely blocking the port —
> allow Helm (or those ports) on your private network. A cloud relay for access
> outside the LAN is on the [roadmap](#roadmap).

## Keyboard & clipboard

`Mod` = `Ctrl` on Windows/Linux, `Cmd` on macOS. App shortcuts deliberately avoid
plain `Ctrl`+`<letter>` so the terminal keeps its own bindings (readline, etc.).

**Sessions**

| Keys | Action |
|---|---|
| `Ctrl` + `Tab` / `Ctrl` + `Shift` + `Tab` | Next / previous session |
| `Mod` + `1`–`8` | Jump to session 1–8 |
| `Mod` + `9` | Jump to the last session |
| `Mod` + `Shift` + `T` | New session |
| `Mod` + `Shift` + `W` | Close the active session |

**View & terminal**

| Keys | Action |
|---|---|
| `Mod` + `Shift` + `M` | Toggle Terminal ⇄ Conversation |
| `Mod` + `Shift` + `K` | Clear the terminal scrollback |
| `Mod` + `Shift` + `U` | Notifications — open panel / jump to latest unread |
| `Mod` + `,` | Settings |
| `Mod` + `=` / `Mod` + `-` | Increase / decrease font size |
| `Mod` + `0` | Reset font size |

**Clipboard**

| Keys | Action |
|---|---|
| `Mod` + `V`, `Shift` + `Insert` | Paste into the terminal |
| `Mod` + `Shift` + `C`, `Cmd` + `C` | Copy the selection |
| `Ctrl` + `C` | Pass through to interrupt the agent |

Paste honors bracketed-paste mode, so a multi-line path lands as a single chunk
in TUIs like Claude Code instead of executing line by line.

## Requirements

A desktop OS with a system webview, plus the agent CLIs you want to drive
(`claude`, `codex`, `opencode`) on your `PATH`.

| OS | Webview | Extra build deps |
|---|---|---|
| **Windows** 10/11 | WebView2 (preinstalled on current Windows) | — |
| **macOS** | WKWebView (built in) | — |
| **Linux** | WebKitGTK | `webkit2gtk-4.1`, `libgtk-3-dev`, `librsvg2-dev`, `build-essential` |

## Build from source

```bash
cd src-tauri
cargo build --release        # needs the Rust toolchain + your OS's webview deps
```

The frontend is embedded into the binary at build time, so any change under
`ui/` requires a rebuild. The output is:

| OS | Binary |
|---|---|
| Windows | `src-tauri\target\release\helm.exe` (or `run.cmd`) |
| macOS / Linux | `./src-tauri/target/release/helm` |

A Windows / macOS / Linux matrix build runs in CI on every code change.

## Configuration

Most options live in the in-app **Settings** panel:

| Setting | Default |
|---|---|
| Terminal font | system monospace |
| Default agent | `claude` |
| Panel visibility | all panels on |
| Session restore | on |

Environment variables:

| Variable | Effect |
|---|---|
| `HELM_HTTP_PORT` | Port the mobile UI is served on (default `8787`). |
| `HELM_WS_PORT` | Port the mobile WebSocket bridge listens on (default `8788`). |
| `HELM_USAGE_PORT` | If set, Helm reads account-usage JSON from a local endpoint on that port and shows a usage panel. Unset → the panel is hidden and no request is made. |

## How it works

All parsing happens in the Rust backend, which emits a **normalized event
stream** (keyed by PTY id). The UI renders these generically — which is exactly
why the same data feeds the mobile client unchanged.

| Event | Payload | Purpose |
|---|---|---|
| `pty-data:{id}` | `{ b64 }` | Terminal output |
| `pty-exit:{id}` | — | Process exited |
| `agent-progress:{id}` | `{ status, activity, todos[], tools[], context }` | Right-rail state |
| `conv-msg:{id}` | `{ role, text, thinking?, tool_calls[], usage? }` | Conversation message |
| `conv-tool:{id}` | `{ name, status, result }` | Tool-call result |
| `conv-reset:{id}` | — | Clear the conversation |

Two paths feed `agent-progress`:

- **Native hooks** — Helm runs a localhost receiver and registers a hook for each
  agent (Claude's project hooks, opencode's auto-loaded plugin, Codex's
  `hooks.json`). Lifecycle events push the instant they happen. Registration is
  additive and never disturbs an agent's existing config or trust store.
- **Log watchers** — for anything a hook doesn't carry (token context, Codex's
  task list, conversation bodies), a per-agent watcher reads the session log and
  wakes on filesystem events.

The mobile bridge mirrors that same stream over a hand-rolled HTTP + WebSocket
server (no extra dependencies), and relays the phone's commands back to the
backend.

## Project layout

| Path | What |
|---|---|
| `src-tauri/src/main.rs` | PTY spawn + I/O, system stats, Tauri commands, background pollers |
| `src-tauri/src/agent_watch.rs` | Per-agent log watchers that normalize progress + conversation into events |
| `src-tauri/src/hook_server.rs` | Localhost hook receiver + per-agent hook registration |
| `src-tauri/src/mobile.rs` | LAN HTTP + WebSocket bridge for the phone client |
| `ui/` | Frontend (vanilla JS, no framework): `index.html`, `styles.css`, `app.js` |
| `ui/vendor/xterm` | Vendored xterm.js + WebGL/fit/web-links addons |

## Troubleshooting

| Symptom | Fix |
|---|---|
| Blank / white window | The webview cache went bad (usually after a hard kill). On Windows, delete `%LOCALAPPDATA%\com.helm.app` and relaunch. |
| Right pane stays empty | Launch the agent **inside** the session, and make sure the session's working folder is where the agent actually runs — the log lookup is keyed off that path. |
| Phone can't load the page | Same wifi? Firewall allowing the port on your private network? See [Mobile](#mobile-access). |
| Paste does nothing | Focus the terminal first, then use `Ctrl`/`Cmd`+`V` or `Shift`+`Insert`. |
| No usage panel | Expected unless `HELM_USAGE_PORT` is set (see [Configuration](#configuration)). |

## Roadmap

- More agents — each is just one watcher (or hook handler) emitting the normalized events.
- Mobile beyond the LAN — an optional cloud relay for access off your network.
- Richer mobile gestures and offline reconnect.

## License

[MIT](LICENSE) © kalhintz
