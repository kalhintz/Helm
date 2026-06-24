/* =====================================================================
   cmux dashboard — single cohesive frontend module
   store + topbar/header + left rail + center terminals + right rail
   + ConPTY wiring + main()
   Public store API (single source of truth):
     App.newSession(opts) -> session
     App.selectSession(id)
     App.closeSession(id)
     App.showSession(id)        (center mount/show)
     App.renderLeft()
     App.renderRight()
   ===================================================================== */
(function () {
  "use strict";

  const TAURI = window.__TAURI__ || {};
  const invoke = (TAURI.core && TAURI.core.invoke)
    ? TAURI.core.invoke
    : async () => { throw new Error("no tauri"); };
  const listen = (TAURI.event && TAURI.event.listen)
    ? TAURI.event.listen
    : async () => () => {};

  // Clipboard via the Tauri clipboard-manager plugin — WebView2's built-in
  // xterm textarea paste is unreliable, so route paste/copy through the plugin.
  async function clipReadText() {
    // navigator.clipboard reads the real OS clipboard (incl. text copied in other
    // apps) and works in WebView2 under a keydown gesture; the Tauri plugin is the
    // fallback. Whichever returns text first wins.
    try { const s = await navigator.clipboard.readText(); if (s) return s; } catch (_) {}
    try {
      const v = await invoke("plugin:clipboard-manager|read_text", { label: null });
      const s = typeof v === "string" ? v : (v && v.plainText && v.plainText.text) || "";
      if (s) return s;
    } catch (_) {}
    return "";
  }
  async function clipWriteText(text) {
    try { await navigator.clipboard.writeText(text); return; } catch (_) {}
    try { await invoke("plugin:clipboard-manager|write_text", { data: { plainText: { label: null, text } } }); } catch (_) {}
  }
  // brief "복사됨" affordance on a copy button.
  function flashCopied(btn) {
    const o = btn.textContent;
    btn.textContent = "✓";
    btn.classList.add("is-copied");
    setTimeout(() => { btn.textContent = o; btn.classList.remove("is-copied"); }, 900);
  }
  // copy an image via the async Clipboard API (ClipboardItem); returns true on
  // success. Callers fall back to copying the data-URI text on false.
  async function clipWriteImage(img) {
    if (!img || !img.data_uri) return false;
    try {
      const res = await fetch(img.data_uri);
      const blob = await res.blob();
      if (window.ClipboardItem && navigator.clipboard && navigator.clipboard.write) {
        await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
        return true;
      }
    } catch (_) {}
    return false;
  }

  const $  = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const el = (tag, cls, txt) => { const e = document.createElement(tag); if (cls) e.className = cls; if (txt != null) e.textContent = txt; return e; };
  const escapeHtml = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  function b64ToBytes(b64) {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  function fmtUptime(startMs) {
    let s = Math.max(0, Math.floor((Date.now() - startMs) / 1000));
    const h = Math.floor(s / 3600); s -= h * 3600;
    const m = Math.floor(s / 60); s -= m * 60;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m ${s}s`;
    return `${s}s`;
  }
  function fmtClock(ts) {
    const d = new Date(ts), p = (n) => String(n).padStart(2, "0");
    return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  }
  function projectName(cwd) {
    if (!cwd) return "untitled";
    return String(cwd).replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "untitled";
  }

  const ACCENTS = ["green", "blue", "orange", "purple", "red"];
  const ACCENT_HEX = { green: "#3fb950", blue: "#4493f8", orange: "#d29922", purple: "#a371f7", red: "#f85149", muted: "#8b949e" };

  /* AI coding agents this multiplexer harmonizes with. Each runs inside a real
     ConPTY shell; `launch` is the CLI typed into the shell after it starts.
     The app then surfaces the agent's title (OSC 0/2) and attention requests
     (OSC 9/777/99) — that is the "harmonized" integration, agent-agnostic. */
  const AGENTS = {
    claude:   { label: "Claude Code", shell: "powershell", launch: "claude",   accent: "orange", badge: "claude",   icon: "CC", resume: "--continue" },
    codex:    { label: "Codex",       shell: "powershell", launch: "codex",     accent: "blue",   badge: "codex",    icon: "CX" },
    opencode: { label: "opencode",    shell: "powershell", launch: "opencode",  accent: "purple", badge: "opencode", icon: "OC" },
    pwsh:     { label: "PowerShell",  shell: "powershell", launch: null,        accent: "green",  badge: "pwsh",     icon: "PS" },
    cmd:      { label: "Command Prompt", shell: "cmd",     launch: null,        accent: "muted",  badge: "cmd",      icon: ">_" },
    wsl:      { label: "WSL",         shell: "wsl",        launch: null,        accent: "blue",   badge: "wsl",      icon: "WL" },
  };
  const agentIcon = (key) => (AGENTS[key] && AGENTS[key].icon) || AGENTS.pwsh.icon;
  const fmtAge = (ms) => {
    const s = Math.max(0, Math.floor(ms / 1000)), h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
    return h ? `${h}h ${m}m` : m ? `${m}m ${s % 60}s` : `${s % 60}s`;
  };
  const FILE_TOOLS = new Set(["Edit", "Write", "Read", "NotebookEdit", "MultiEdit", "edit", "read", "write"]);
  const AGENT_ORDER = ["claude", "codex", "opencode", "pwsh", "cmd", "wsl"];
  // Quick-change controls above the composer — each click injects the agent's own
  // slash command so its native picker (model / reasoning effort / agent) opens.
  // A control is either { cmd } (sends a slash command, opening the agent's own
  // picker) or { menu:[{label,cmd}] } (a Helm dropdown — pick an item to change
  // directly). Claude's /model takes the name as an argument, so we list models
  // and switch in one click; codex/opencode have no direct setter, so those open
  // the native picker.
  const AGENT_CMDS = {
    claude: [
      { label: "모델", menu: [
        { label: "Default", cmd: "/model default" },
        { label: "Opus", cmd: "/model opus" },
        { label: "Opus Plan", cmd: "/model opusplan" },
        { label: "Sonnet", cmd: "/model sonnet" },
        { label: "Haiku", cmd: "/model haiku" },
      ] },
      { label: "에이전트", cmd: "/agents" },
    ],
    codex: [{ label: "모델·추론", cmd: "/model" }, { label: "승인", cmd: "/approvals" }],
    // opencode controls are built dynamically from its HTTP API — see
    // buildOpencodeControls(); this entry is intentionally empty.
    opencode: [],
  };

  /* ---- opencode model/agent switching via its HTTP API (the real switch) ----
     opencode has no "set model" endpoint, but its prompt route
     POST /session/{id}/message carries first-class model+agent fields that it
     persists. So picking a model/agent here records a pending selection (shown in
     the chip) that the composer's NEXT send applies via opencode_send — really
     switching the live session, not injecting a slash command. */
  async function buildOpencodeControls(session) {
    if (settings.opencode && settings.opencode.apiSwitch === false) return [];
    let port = session._ocPort || 0;
    if (!port && session.ptyId != null) {
      try { port = await invoke("opencode_port_for", { ptyId: session.ptyId }); } catch (_) { port = 0; }
    }
    if (!port) return [];
    session._ocPort = port;
    let models = [], agents = [];
    try {
      [models, agents] = await Promise.all([
        invoke("opencode_models", { port }).catch(() => []),
        invoke("opencode_agents", { port }).catch(() => []),
      ]);
    } catch (_) {}
    const controls = [];
    if (models.length) {
      controls.push({ label: "모델", menu: models.map((m) => ({
        label: m.name || m.id,
        action: () => selectOcModel(session, m.id, m.providerID || m.provider || ""),
      })) });
    }
    if (agents.length) {
      controls.push({ label: "에이전트", menu: agents.map((a) => {
        const id = a.name || a.id || "";
        return { label: id.replace(/^​/, ""), action: () => selectOcAgent(session, id) };
      }) });
    }
    // native picker affordance (opens opencode's own model picker in the TUI)
    controls.push({ label: "네이티브 ▸", action: () => openOcNativeModels(session) });
    return controls;
  }
  function selectOcModel(session, id, provider) {
    session.ocPendingModel = id;
    session.ocPendingProvider = provider || "";
    session.currentModel = id;            // optimistic; re-syncs from agent-progress
    pushTimeline(session, "모델 선택: " + id + " (다음 전송 시 적용)", "purple");
    if (store.activeId === session.id) { renderQuickControls(session).catch(() => {}); App.renderRight(); }
  }
  function selectOcAgent(session, id) {
    session.ocPendingAgent = id;
    const clean = (id || "").replace(/^​/, "");
    session.currentMode = clean;          // optimistic
    pushTimeline(session, "에이전트 선택: " + clean + " (다음 전송 시 적용)", "purple");
    if (store.activeId === session.id) { renderQuickControls(session).catch(() => {}); App.renderRight(); }
  }
  async function openOcNativeModels(session) {
    const port = session._ocPort || 0;
    if (port) { try { await invoke("opencode_open_models", { port }); return; } catch (_) {} }
    sendCmd("/models"); // fallback if the API isn't reachable
  }
  const statusLabel = (st) => ({ spawning: "시작 중", running: "실행 중", active: "실행 중", attention: "입력 대기", idle: "대기", error: "오류", exited: "종료" }[st] || st);

  /* ================================================================
     XTERM theme
     ================================================================ */
  const XTERM_THEME = {
    background: "#0d0e11", foreground: "#e6edf3", cursor: "#4493f8",
    cursorAccent: "#0d0e11", selectionBackground: "rgba(68,147,248,0.25)",
    black: "#161b22", red: "#f85149", green: "#3fb950", yellow: "#d29922",
    blue: "#4493f8", magenta: "#a371f7", cyan: "#39c5cf", white: "#b1bac4",
    brightBlack: "#6e7681", brightRed: "#ff7b72", brightGreen: "#56d364",
    brightYellow: "#e3b341", brightBlue: "#79c0ff", brightMagenta: "#d2a8ff",
    brightCyan: "#56d8e4", brightWhite: "#f0f6fc",
  };

  /* ================================================================
     STORE — single source of truth
     Session shape:
       { id, ptyId, title, shell, cwd, status, accent, tags[],
         startedAt, msgCount, pinned,
         term, fit, slot, unlisten[], resizeObserver, timeline[], todos[] }
     status: "spawning" | "running" | "active" | "idle" | "error" | "exited"
     ================================================================ */
  const store = {
    home: "~",
    connection: "connecting",
    view: "dashboard",
    sessions: [],
    activeId: null,
    seq: 0,
    notifications: [],
    lastCwd: "",
  };

  /* ================================================================
     SETTINGS (persisted to localStorage, applied live)
     ================================================================ */
  const SETTINGS_KEY = "helm.settings";
  const DEFAULT_SETTINGS = {
    fontSize: 12.5, cursorBlink: true, defaultAgent: "ask", statsInterval: 2000,
    restoreSessions: true,
    show: { progress: true, todos: true, tools: true, usage: true, timeline: true },
    opencode: {
      notifyTurnDone: true,   // toast when an opencode turn completes
      notifyAwaiting: true,   // toast when opencode awaits prompt input
      showConversation: true, // render the DB-sourced conversation in 대화 view
      apiSwitch: true,        // expose API-backed model/agent switching
    },
  };
  const SESSIONS_KEY = "helm.sessions";
  function saveSessionState() {
    try {
      const sessions = store.sessions.map((s) => ({
        agent: s.agent, cwd: s.cwd, title: s.title,
        titleLocked: !!s.titleLocked, pinned: !!s.pinned, convView: !!s.convView,
      }));
      const activeIndex = store.sessions.findIndex((s) => s.id === store.activeId);
      localStorage.setItem(SESSIONS_KEY, JSON.stringify({ sessions, activeIndex }));
    } catch (_) {}
  }
  function loadSessionState() {
    try { return JSON.parse(localStorage.getItem(SESSIONS_KEY) || "null"); } catch (_) { return null; }
  }
  let settings = loadSettings();
  function loadSettings() {
    let s;
    try { s = JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}"); } catch (_) { s = {}; }
    const merged = Object.assign({}, DEFAULT_SETTINGS, s);
    merged.show = Object.assign({}, DEFAULT_SETTINGS.show, s.show || {});
    merged.opencode = Object.assign({}, DEFAULT_SETTINGS.opencode, s.opencode || {});
    return merged;
  }
  function saveSettings() { try { localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings)); } catch (_) {} }
  function applySettings() {
    store.sessions.forEach((s) => {
      if (s.term) {
        try {
          s.term.options.fontSize = settings.fontSize;
          s.term.options.cursorBlink = settings.cursorBlink;
          if (s.fit) s.fit.fit();
        } catch (_) {}
      }
    });
    const sh = settings.show;
    document.body.classList.toggle("hide-progress", !sh.progress);
    document.body.classList.toggle("hide-todos", !sh.todos);
    document.body.classList.toggle("hide-tools", !sh.tools);
    document.body.classList.toggle("hide-usage", !sh.usage);
    document.body.classList.toggle("hide-timeline", !sh.timeline);
    startStatsPolling();
  }
  function openSettings() {
    const m = $("#settings-modal"); if (!m) return;
    const f = $("#set-font"), fv = $("#set-font-val"), cur = $("#set-cursor"), ag = $("#set-agent"), st = $("#set-stats");
    if (f) f.value = settings.fontSize;
    if (fv) fv.textContent = settings.fontSize;
    if (cur) cur.checked = !!settings.cursorBlink;
    if (ag) ag.value = settings.defaultAgent;
    if (st) st.value = String(settings.statsInterval);
    $$(".set-show").forEach((cb) => { cb.checked = !!settings.show[cb.dataset.sec]; });
    const rs = $("#set-restore"); if (rs) rs.checked = !!settings.restoreSessions;
    const ococ = settings.opencode || {};
    const ocSet = (id, v) => { const cb = $("#" + id); if (cb) cb.checked = !!v; };
    ocSet("set-oc-turndone", ococ.notifyTurnDone);
    ocSet("set-oc-awaiting", ococ.notifyAwaiting);
    ocSet("set-oc-conv", ococ.showConversation);
    ocSet("set-oc-apiswitch", ococ.apiSwitch);
    m.hidden = false;
  }
  function closeSettings() { const m = $("#settings-modal"); if (m) m.hidden = true; }
  function wireSettings() {
    const f = $("#set-font"), fv = $("#set-font-val"), cur = $("#set-cursor"), ag = $("#set-agent"), st = $("#set-stats");
    if (f) f.addEventListener("input", () => { settings.fontSize = parseFloat(f.value); if (fv) fv.textContent = f.value; saveSettings(); applySettings(); });
    if (cur) cur.addEventListener("change", () => { settings.cursorBlink = cur.checked; saveSettings(); applySettings(); });
    if (ag) ag.addEventListener("change", () => { settings.defaultAgent = ag.value; saveSettings(); });
    if (st) st.addEventListener("change", () => { settings.statsInterval = parseInt(st.value, 10) || 2000; saveSettings(); applySettings(); });
    $$(".set-show").forEach((cb) => cb.addEventListener("change", () => { settings.show[cb.dataset.sec] = cb.checked; saveSettings(); applySettings(); }));
    const rs = $("#set-restore");
    if (rs) rs.addEventListener("change", () => { settings.restoreSessions = rs.checked; saveSettings(); });
    const ocBind = (id, key) => {
      const cb = $("#" + id);
      if (cb) cb.addEventListener("change", () => {
        settings.opencode = settings.opencode || {};
        settings.opencode[key] = cb.checked;
        saveSettings();
        // re-render so the apiSwitch toggle hides/shows the controls immediately
        const s = activeSession();
        if (s && s.agent === "opencode") renderQuickControls(s).catch(() => {});
      });
    };
    ocBind("set-oc-turndone", "notifyTurnDone");
    ocBind("set-oc-awaiting", "notifyAwaiting");
    ocBind("set-oc-conv", "showConversation");
    ocBind("set-oc-apiswitch", "apiSwitch");
    const close = $("#set-close"), save = $("#set-save"), reset = $("#set-reset"), modal = $("#settings-modal");
    if (close) close.addEventListener("click", closeSettings);
    if (save) save.addEventListener("click", closeSettings);
    if (reset) reset.addEventListener("click", () => { settings = JSON.parse(JSON.stringify(DEFAULT_SETTINGS)); saveSettings(); applySettings(); openSettings(); });
    if (modal) modal.addEventListener("click", (e) => { if (e.target === modal) closeSettings(); });
    document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeSettings(); });
  }

  function sessionById(id) { return store.sessions.find((s) => s.id === id) || null; }
  function activeSession() { return sessionById(store.activeId); }
  function runningCount() { return store.sessions.filter((s) => s.status === "running" || s.status === "active").length; }

  /* ================================================================
     CENTER — terminal mount/show/hide per session
     ================================================================ */
  const termHost   = () => $("#cx-term-host");
  const emptyState = () => $("#cx-empty-state");

  function mountTerminal(session) {
    if (session.term) return; // already mounted

    const slot = el("div", "cx-term-slot");
    slot.dataset.sid = session.id;
    termHost().appendChild(slot);
    session.slot = slot;

    const monoFont = getComputedStyle(document.documentElement).getPropertyValue("--mono-font").trim() || "Consolas, monospace";
    const term = new Terminal({
      fontFamily: monoFont, fontSize: settings.fontSize, lineHeight: 1.12,
      cursorBlink: settings.cursorBlink, allowProposedApi: true, theme: XTERM_THEME, scrollback: 20000,
    });
    const fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    try { term.loadAddon(new WebLinksAddon.WebLinksAddon()); } catch (_) {}
    session.term = term;
    session.fit = fit;

    // open into slot (must be displayable for sizing)
    slot.style.display = "block";
    term.open(slot);
    // GPU renderer: the DOM renderer repaints on the CPU via rAF and lags echo
    // on WebView2; WebGL draws glyphs on the GPU so typed chars appear at once.
    // Disposes on context loss so xterm falls back to the DOM renderer cleanly.
    try {
      const webgl = new WebglAddon.WebglAddon();
      webgl.onContextLoss(() => { try { webgl.dispose(); } catch (_) {} });
      term.loadAddon(webgl);
    } catch (_) {}
    try { fit.fit(); } catch (_) {}
    slot.style.display = "";

    term.onData((d) => {
      if (session.ptyId != null) invoke("pty_write", { id: session.ptyId, data: d });
      // sniff the typed command line so a hand-launched agent re-identifies the session
      for (const ch of d) {
        if (ch === "\r" || ch === "\n") {
          const m = /^\s*(claude|codex|opencode)\b/i.exec(session._line || "");
          if (m) detectAgent(session, m[1].toLowerCase());
          session._line = "";
        } else if (ch === "" || ch === "\b") {
          session._line = (session._line || "").slice(0, -1);
        } else if (ch >= " ") {
          session._line = (session._line || "") + ch;
        }
      }
    });
    term.onResize(({ cols, rows }) => { if (session.ptyId != null) invoke("pty_resize", { id: session.ptyId, cols, rows }); });

    // Ctrl+V / Cmd+V / Shift+Insert paste; Ctrl/Cmd+C copy when there's a
    // selection (Windows-Terminal style) and clears it so the next Ctrl+C
    // interrupts. With no selection, Ctrl+C falls through to the agent.
    // term.paste honors bracketed-paste, so a multi-line path lands as one chunk.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      const isV = e.key === "v" || e.key === "V";
      const isC = e.key === "c" || e.key === "C";
      if ((e.ctrlKey && isV) || (e.metaKey && isV) || (e.shiftKey && e.key === "Insert")) {
        // An image on the clipboard (opencode/Claude image-paste) wins: Helm saves
        // it to a temp PNG and pastes the path so the agent attaches it. Otherwise
        // fall back to a normal text paste.
        invoke("paste_clipboard_image")
          .then((p) => { if (p) term.paste(p); else return clipReadText().then((t) => { if (t) term.paste(t); }); })
          .catch(() => clipReadText().then((t) => { if (t) term.paste(t); }));
        return false;
      }
      if (isC && (e.ctrlKey || e.metaKey) && term.hasSelection()) {
        clipWriteText(term.getSelection());
        try { term.clearSelection(); } catch (_) {}
        return false;
      }
      return true;
    });

    // Harmonize with whatever runs inside (claude/codex/opencode/shell):
    //   OSC 0/2  -> terminal title  -> auto-name the session
    //   OSC 9/777/99/1337 (bell/notify) -> agent needs attention -> ring + badge
    try {
      for (const code of [0, 2]) {
        term.parser.registerOscHandler(code, (data) => {
          const t = (data || "").trim();
          // Accept agent-set titles (claude/codex/opencode emit meaningful strings)
          // but ignore a plain shell reporting its own exe path / cwd.
          const looksLikePath = /\.exe\b/i.test(t) || /[\\/]/.test(t) || /^관리자:/.test(t);
          if (t && !looksLikePath && !session.titleLocked) {
            session.title = t;
            App.renderLeft();
            if (store.activeId === session.id) {
              const tt = $("#cx-title-text"); if (tt) tt.textContent = t;
              const ph = $("#ph-title"); if (ph) ph.textContent = t;
            }
          }
          const dk = detectAgentFromText(t);
          if (dk) detectAgent(session, dk);
          return false;
        });
      }
      for (const code of [9, 777, 99, 1337]) {
        term.parser.registerOscHandler(code, () => { markAttention(session); return true; });
      }
    } catch (_) {}

    const ro = new ResizeObserver(() => {
      if (slot.classList.contains("active")) {
        try { fit.fit(); if (session.ptyId != null) invoke("pty_resize", { id: session.ptyId, cols: term.cols, rows: term.rows }); } catch (_) {}
      }
    });
    ro.observe(slot);
    session.resizeObserver = ro;
  }

  async function spawnPty(session) {
    const cols = (session.term && session.term.cols) || 120;
    const rows = (session.term && session.term.rows) || 32;
    try {
      const ptyId = await invoke("pty_spawn", {
        shell: session.shell, cwd: session.cwd, cols, rows,
        workspaceId: session.id, surfaceId: session.id,
        agent: session.agent,
      });
      session.ptyId = ptyId;
      session.status = "running";
      pushTimeline(session, "셸 시작됨", "green");

      const unData = await listen(`pty-data:${ptyId}`, (e) => {
        const bytes = b64ToBytes(e.payload.b64);
        if (session.term) session.term.write(bytes);
        session.lastOutput = Date.now();
        // an agent's full-screen TUI restores the main screen buffer on exit
        // (\x1b[?1049l / \x1b[?47l) — when that lands and an agent was running,
        // the shell is back, so drop the agent label and stop its watcher.
        if (session.launch && session._watch && altScreenExited(bytes)) {
          revertToShell(session);
        }
      });
      const unExit = await listen(`pty-exit:${ptyId}`, () => {
        session.status = "exited";
        session.ptyId = null;
        if (session.term) session.term.write("\r\n\x1b[2m[프로세스 종료]\x1b[0m\r\n");
        pushTimeline(session, "프로세스 종료", "red");
        App.renderLeft();
        App.renderRight();
        App.renderHeaderChips();
      });
      const unProg = await listen(`agent-progress:${ptyId}`, (e) => applyProgress(session, e.payload));
      const unCMsg = await listen(`conv-msg:${ptyId}`, (e) => onConvMsg(session, e.payload));
      const unCTool = await listen(`conv-tool:${ptyId}`, (e) => onConvTool(session, e.payload));
      const unCReset = await listen(`conv-reset:${ptyId}`, () => { session.transcript = []; if (session.convView) renderConv(session); });
      // opencode turn-complete edge (fires even when this tab is active).
      const unTurn = await listen(`agent-turn-done:${ptyId}`, (e) => onTurnDone(session, e.payload));
      session.unlisten.push(unData, unExit, unProg, unCMsg, unCTool, unCReset, unTurn);
    } catch (err) {
      session.status = "error";
      if (session.term) session.term.write(`\r\n\x1b[31m[pty 오류: ${err}]\x1b[0m\r\n`);
      pushTimeline(session, "spawn 실패", "red");
      console.error("[App] pty_spawn failed", err);
    }
  }

  /* ---- conversation view (center "대화" pane, read-only transcript) ---- */
  function mountConversation(session) {
    if (session.convSlot) return;
    const slot = el("div", "cx-conv-slot");
    slot.dataset.sid = session.id;
    termHost().appendChild(slot);
    session.convSlot = slot;
    renderConv(session);
  }
  function convText(s) {
    const esc = escapeHtml(s || "");
    return esc
      .replace(/```(\w*)\n?([\s\S]*?)```/g, (_m, _lang, code) => `<pre class="conv-code">${code.replace(/\n+$/, "")}</pre>`)
      .replace(/`([^`\n]+)`/g, '<code class="conv-inline">$1</code>')
      .replace(/\n/g, "<br>");
  }
  function buildConvTool(tc) {
    const row = el("div", "conv-tool is-" + (tc.status || "running"));
    row.dataset.tid = tc.id || "";
    row.appendChild(el("span", "conv-tool-name", tc.name || "tool"));
    if (tc.summary) row.appendChild(el("span", "conv-tool-sum", tc.summary));
    row.appendChild(el("span", "conv-tool-st", tc.status === "completed" ? "✓" : tc.status === "error" ? "✕" : "…"));
    if (tc.result) {
      const d = el("details", "conv-tool-result");
      d.appendChild(el("summary", null, "결과"));
      const wrap = el("div", "conv-code-wrap");
      const cBtn = el("button", "conv-code-copy", "⧉");
      cBtn.title = "코드 복사";
      cBtn.addEventListener("click", (e) => { e.stopPropagation(); e.preventDefault(); clipWriteText(tc.result); flashCopied(cBtn); });
      const pre = el("pre", "conv-code"); pre.textContent = tc.result;
      wrap.appendChild(cBtn); wrap.appendChild(pre);
      d.appendChild(wrap);
      row.appendChild(d);
    }
    return row;
  }
  function buildConvMsg(m) {
    const art = el("article", "conv-msg conv-role-" + (m.role || "system"));
    art.dataset.mid = m.id || "";
    const head = el("div", "conv-head");
    head.appendChild(el("span", "conv-role-badge", (m.role || "").toUpperCase()));
    if (m.usage && m.usage.max) head.appendChild(el("span", "conv-usage", fmtTokens(m.usage.used) + " / " + fmtTokens(m.usage.max)));
    const copyBtn = el("button", "conv-copy-btn", "⧉");
    copyBtn.title = "메시지 복사";
    copyBtn.addEventListener("click", (e) => { e.stopPropagation(); clipWriteText(m.text || ""); flashCopied(copyBtn); });
    head.appendChild(copyBtn);
    art.appendChild(head);
    if (m.thinking) {
      const d = el("details", "conv-thinking");
      d.appendChild(el("summary", null, "추론"));
      const body = el("div", "conv-think-body"); body.innerHTML = convText(m.thinking);
      d.appendChild(body);
      art.appendChild(d);
    }
    if (m.text) { const t = el("div", "conv-text"); t.innerHTML = convText(m.text); art.appendChild(t); }
    if (m.images && m.images.length) {
      const gal = el("div", "conv-img-gallery");
      m.images.forEach((img, idx) => {
        if (img.data_uri) {
          const t = el("img", "conv-img-thumb");
          t.src = img.data_uri; t.alt = img.alt || "image"; t.loading = "lazy";
          t.addEventListener("click", () => openImageLightbox(m.images, idx));
          gal.appendChild(t);
        } else {
          gal.appendChild(el("span", "conv-img-missing", img.alt || "이미지"));
        }
      });
      art.appendChild(gal);
    }
    if (m.tool_calls && m.tool_calls.length) {
      const tools = el("div", "conv-tools");
      m.tool_calls.forEach((tc) => tools.appendChild(buildConvTool(tc)));
      art.appendChild(tools);
    }
    return art;
  }
  let _lbKey = null;
  function openImageLightbox(images, idx) {
    const imgs = (images || []).filter((x) => x.data_uri);
    if (!imgs.length) return;
    let i = Math.max(0, Math.min(idx, imgs.length - 1));
    let lb = $("#image-lightbox");
    if (!lb) {
      lb = el("div", "modal-overlay image-lightbox"); lb.id = "image-lightbox";
      lb.addEventListener("click", (e) => { if (e.target === lb) closeImageLightbox(); });
      document.body.appendChild(lb);
    }
    function paint() {
      const cur = imgs[i];
      lb.innerHTML = "";
      const box = el("div", "lb-box");
      const head = el("div", "lb-head");
      head.appendChild(el("span", "lb-count", (i + 1) + " / " + imgs.length));
      const btns = el("div", "lb-btns");
      const copy = el("button", "lb-btn", "복사");
      copy.addEventListener("click", async () => {
        const ok = await clipWriteImage(cur);
        if (!ok) clipWriteText(cur.data_uri);
        copy.textContent = ok ? "복사됨" : "URI 복사됨";
        setTimeout(() => { copy.textContent = "복사"; }, 1000);
      });
      btns.appendChild(copy);
      if (imgs.length > 1) {
        const prev = el("button", "lb-btn", "‹"); prev.addEventListener("click", () => { i = (i - 1 + imgs.length) % imgs.length; paint(); });
        const next = el("button", "lb-btn", "›"); next.addEventListener("click", () => { i = (i + 1) % imgs.length; paint(); });
        btns.appendChild(prev); btns.appendChild(next);
      }
      const x = el("button", "modal-x", "✕"); x.addEventListener("click", closeImageLightbox);
      btns.appendChild(x); head.appendChild(btns); box.appendChild(head);
      const body = el("div", "lb-body");
      const im = el("img", "lb-img"); im.src = cur.data_uri; im.alt = cur.alt || "image";
      body.appendChild(im); box.appendChild(body); lb.appendChild(box);
    }
    _lbKey = (e) => {
      if (e.key === "Escape") closeImageLightbox();
      else if (e.key === "ArrowLeft" && imgs.length > 1) { i = (i - 1 + imgs.length) % imgs.length; paint(); }
      else if (e.key === "ArrowRight" && imgs.length > 1) { i = (i + 1) % imgs.length; paint(); }
    };
    document.addEventListener("keydown", _lbKey);
    lb.hidden = false; paint();
  }
  function closeImageLightbox() {
    const lb = $("#image-lightbox"); if (lb) lb.hidden = true;
    if (_lbKey) { document.removeEventListener("keydown", _lbKey); _lbKey = null; }
  }
  function renderConv(session) {
    const slot = session.convSlot;
    if (!slot) return;
    const atBottom = slot.scrollHeight - slot.scrollTop - slot.clientHeight < 80;
    slot.innerHTML = "";
    if (session.agent === "opencode" && settings.opencode && settings.opencode.showConversation === false) {
      slot.appendChild(el("div", "conv-degraded-note", "opencode 대화 표시가 설정에서 꺼져 있습니다"));
      return;
    }
    if (!session.transcript.length) {
      slot.appendChild(el("div", "conv-empty", "대화 기록이 여기에 표시됩니다"));
      return;
    }
    session.transcript.forEach((m) => slot.appendChild(buildConvMsg(m)));
    if (atBottom) slot.scrollTop = slot.scrollHeight;
  }
  function onConvMsg(session, m) {
    if (!m) return;
    const idx = session.transcript.findIndex((x) => x.id === m.id);
    if (idx >= 0) session.transcript[idx] = m; else session.transcript.push(m);
    if (session.transcript.length > 300) session.transcript.shift();
    session.msgCount = session.transcript.length;
    if (store.activeId === session.id) { const n = $("#cx-msg-n"); if (n) n.textContent = String(session.msgCount); }
    if (session.convSlot && session.convView) renderConv(session);
  }
  function onConvTool(session, tc) {
    if (!tc) return;
    for (const msg of session.transcript) {
      if (msg.tool_calls) {
        const t = msg.tool_calls.find((x) => x.id === tc.id);
        if (t) { t.status = tc.status; t.result = tc.result; if (tc.name && !t.name) t.name = tc.name; break; }
      }
    }
    if (session.convSlot && session.convView) renderConv(session);
  }
  /* opencode turn complete (backend edge-trigger). Fires regardless of which tab
     is focused — the user explicitly asked to be notified when a turn finishes. */
  function onTurnDone(session, payload) {
    if (!session) return;
    if (settings.opencode && settings.opencode.notifyTurnDone === false) return;
    const mode = (payload && (payload.title || payload.model)) || session.agentLabel || "opencode";
    pushNotification({ title: session.title, body: mode + " — 턴 완료" });
    if (store.activeId !== session.id) markAttention(session);
  }

  function showSession(id) {
    store.sessions.forEach((s) => { if (s.slot) s.slot.classList.remove("active"); if (s.convSlot) s.convSlot.classList.remove("active"); });
    const session = sessionById(id);
    const titleText = $("#cx-title-text");
    const msgN = $("#cx-msg-n");
    const subLabel = $("#cx-subheader-label");
    const fileChips = $("#cx-file-chips");
    const ph = $("#ph-title");

    if (!session) {
      emptyState().classList.remove("hidden");
      titleText.textContent = "세션 없음";
      msgN.textContent = "0";
      if (ph) ph.textContent = "진행 상황";
      return;
    }

    if (session.convView) {
      mountConversation(session);
      if (session.convSlot) session.convSlot.classList.add("active");
      renderConv(session);
    } else if (session.slot) {
      session.slot.classList.add("active");
    }
    emptyState().classList.add("hidden");
    const tog = $("#cx-view-toggle");
    if (tog) tog.querySelectorAll(".cvt").forEach((b) => b.classList.toggle("active", b.dataset.cv === (session.convView ? "conv" : "term")));

    titleText.textContent = session.title;
    msgN.textContent = String(session.msgCount || 0);
    if (ph) ph.textContent = session.title;

    subLabel.textContent = session.shell + " · " + projectName(session.cwd);
    renderFileChips(session);

    // pin button state
    const pinBtn = $("#cx-btn-pin");
    if (pinBtn) pinBtn.style.color = session.pinned ? "var(--blue)" : "";

    // composer agent chip reflects the active session's agent
    const agentLbl = $("#cx-agent-label");
    if (agentLbl) agentLbl.textContent = session.agentLabel || session.shell || "셸";
    const agentDot = $("#cx-agent-dot");
    if (agentDot) agentDot.style.background = ACCENT_HEX[session.accent] || ACCENT_HEX.green;
    renderQuickControls(session).catch(() => {});
    renderComposerCtx(session);

    requestAnimationFrame(() => {
      try {
        if (session.fit) session.fit.fit();
        if (session.ptyId != null && session.term) invoke("pty_resize", { id: session.ptyId, cols: session.term.cols, rows: session.term.rows });
        if (session.term) session.term.focus();
      } catch (_) {}
    });
  }

  /* ================================================================
     LIFECYCLE
     ================================================================ */
  async function newSession(opts) {
    opts = opts || {};
    const agentKey = (opts.agent && AGENTS[opts.agent]) ? opts.agent : "pwsh";
    const agent = AGENTS[agentKey];
    const id = "s" + (++store.seq);
    const cwd = opts.cwd || store.home;
    const session = {
      id, ptyId: null,
      agent: agentKey, agentLabel: agent.label,
      title: opts.title || (agent.launch ? agent.label : projectName(cwd)),
      shell: agent.shell, launch: agent.launch,
      cwd, status: "spawning", accent: agent.accent,
      tags: opts.tags || (agent.launch ? [agent.label] : []),
      branch: null,
      startedAt: Date.now(), msgCount: 0, pinned: false,
      term: null, fit: null, slot: null, unlisten: [],
      resizeObserver: null, lastOutput: 0,
      progress: null, _toolKeys: null, toolStats: {}, _line: "", _watch: null,
      convSlot: null, convView: false, transcript: [], files: [],
      timeline: [{ text: "세션 생성됨", color: "blue", time: Date.now(), dur: null }],
    };
    store.sessions.push(session);
    App.renderLeft();
    App.renderHeaderChips();

    mountTerminal(session);
    await spawnPty(session);

    // launch the agent CLI inside the freshly-started shell
    if (agent.launch && session.ptyId != null) {
      const resuming = !!(opts.resume && agent.resume);
      let cmd = resuming ? (agent.launch + " " + agent.resume) : agent.launch;
      // opencode: Helm owns the HTTP API port so it can drive model/agent
      // switching. Allocate a free port and have the bare TUI serve the API on it.
      if (agentKey === "opencode") {
        try {
          const port = await invoke("opencode_alloc_port", { ptyId: session.ptyId });
          if (port) { session._ocPort = port; cmd += " --port " + port + " --hostname 127.0.0.1"; }
        } catch (_) {}
      }
      const launchCmd = cmd;
      setTimeout(() => { if (session.ptyId != null) invoke("pty_write", { id: session.ptyId, data: launchCmd + "\r" }); }, 350);
      pushTimeline(session, agent.label + (resuming ? " 이어서" : " 실행"), agent.accent);
    }

    // Helm launched a known agent itself (no hand-typed command for the sniffer to
    // catch), so start its log watcher explicitly — otherwise the progress and
    // conversation panels stay empty.
    if (agent.launch) detectAgent(session, agentKey);

    // resolve git branch for the cwd (real integration)
    invoke("git_branch", { cwd }).then((b) => {
      if (b) { session.branch = b; App.renderLeft(); if (store.activeId === session.id) App.renderRight(); }
    }).catch(() => {});

    App.renderLeft();
    App.renderRight();
    App.renderHeaderChips();

    if (store.activeId == null) selectSession(id);
    return session;
  }

  function selectSession(id) {
    const s = sessionById(id);
    if (!s) return;
    store.activeId = id;
    clearAttention(s);
    App.renderLeft();
    showSession(id);
    App.renderRight();
    App.renderHeaderChips();
  }

  function closeSession(id) {
    const idx = store.sessions.findIndex((s) => s.id === id);
    if (idx < 0) return;
    const s = store.sessions[idx];
    s.unlisten.forEach((u) => { try { u(); } catch (_) {} });
    s.unlisten = [];
    if (s.ptyId != null) invoke("pty_kill", { id: s.ptyId }).catch(() => {});
    if (s.resizeObserver) try { s.resizeObserver.disconnect(); } catch (_) {}
    if (s.term) try { s.term.dispose(); } catch (_) {}
    if (s.slot) s.slot.remove();
    if (s.convSlot) s.convSlot.remove();

    store.sessions.splice(idx, 1);
    if (store.activeId === id) {
      const next = store.sessions[Math.max(0, idx - 1)];
      store.activeId = next ? next.id : null;
    }
    App.renderLeft();
    showSession(store.activeId);
    App.renderRight();
    App.renderHeaderChips();
  }

  function renameSession(id, title) {
    const s = sessionById(id);
    if (s) { s.title = title; s.titleLocked = true; App.renderLeft(); showSession(store.activeId); App.renderHeaderChips(); }
  }
  function togglePin(id) {
    const s = sessionById(id);
    if (s) { s.pinned = !s.pinned; App.renderLeft(); showSession(store.activeId); }
  }

  function pushTimeline(session, text, color, dur) {
    session.timeline = session.timeline || [];
    session.timeline.push({ text, color: color || "blue", time: Date.now(), dur: dur || null });
    if (session.timeline.length > 50) session.timeline.shift();
    if (store.activeId === session.id) App.renderRight();
  }
  function markAttention(session) {
    if (!session || store.activeId === session.id) return;
    if (session.status !== "attention") {
      session.status = "attention";
      pushTimeline(session, "입력 대기 (주의 필요)", "orange");
      pushNotification({ title: session.title, body: "에이전트가 입력을 기다립니다" });
      App.renderLeft();
      App.renderHeaderChips();
    }
  }
  function clearAttention(session) {
    if (session && session.status === "attention") {
      session.status = session.ptyId != null ? "running" : "exited";
      App.renderLeft();
      App.renderHeaderChips();
    }
  }

  /* ---- agent progress (fed by backend `agent-progress:{ptyId}` events) ---- */
  const AGENT_ST = {
    working: ["작업 중", "is-working"], waiting: ["입력 대기", "is-waiting"],
    done: ["완료", "is-done"], error: ["오류", "is-error"], idle: ["대기", ""],
  };
  function fmtTokens(n) {
    n = n || 0;
    if (n >= 1000) return (n / 1000).toFixed(n >= 100000 ? 0 : 1) + "k";
    return String(n);
  }
  const TOOL_ICONS = {
    Edit: "✎", MultiEdit: "✎", Write: "✚", Read: "▤", Bash: "❯", Grep: "⌕",
    Glob: "⌕", WebFetch: "🌐", webfetch: "🌐", Task: "⛓", TodoWrite: "☑",
    edit: "✎", read: "▤", write: "✚", bash: "❯", grep: "⌕", list: "▤",
    patch: "✎", update_plan: "☑",
  };
  function toolIcon(n) {
    if (TOOL_ICONS[n]) return TOOL_ICONS[n];
    if (typeof n === "string" && n.startsWith("mcp__")) return "⧉";
    return "◆";
  }
  function toolText(t) {
    const name = t.name || "tool";
    return t.summary ? `${name} · ${t.summary}` : name;
  }
  function applyProgress(session, p) {
    if (!session || !p) return;
    const prog = session.progress || (session.progress = { status: "idle", activity: "", todos: [], context: null });
    if (p.status) prog.status = p.status;
    if (p.activity != null) prog.activity = p.activity;
    if (Array.isArray(p.todos)) prog.todos = p.todos;
    if (p.context) prog.context = p.context;
    // granular sub-step (additive; skip-if-empty on the backend means these
    // arrive only when present — clear them otherwise so a stale chip drops).
    prog.current_tool = p.current_tool || null;
    prog.active_todo_index = (typeof p.active_todo_index === "number") ? p.active_todo_index : null;
    prog.step_display = p.step_display || "";
    // opencode mode/model/provider/sid (DB-sourced) — drives the chip + switch API
    if (p.mode != null && p.mode !== "") session.currentMode = p.mode;
    if (p.model != null && p.model !== "") session.currentModel = p.model;
    if (p.provider != null && p.provider !== "") session.currentProvider = p.provider;
    if (p.sid) session.ocSessionId = p.sid;
    if (Array.isArray(p.tools) && p.tools.length) {
      const seen = session._toolKeys || (session._toolKeys = new Set());
      session.toolStats = session.toolStats || {};
      p.tools.forEach((t) => {
        const key = (t.ts || "") + "|" + (t.name || "") + "|" + (t.summary || "");
        if (!seen.has(key)) {
          seen.add(key);
          pushTimeline(session, toolText(t), "purple");
          const nm = t.name || "tool";
          session.toolStats[nm] = (session.toolStats[nm] || 0) + 1;
          if (FILE_TOOLS.has(nm)) {
            const base = (t.summary || "").replace(/\\/g, "/").split("/").pop();
            if (base && !/\s/.test(base) && base.includes(".")) {
              session.files = session.files || [];
              if (!session.files.includes(base)) { session.files.push(base); if (session.files.length > 12) session.files.shift(); }
            }
          }
        }
      });
    }
    prog.updatedAt = Date.now();
    if (p.status === "waiting" && (!settings.opencode || session.agent !== "opencode" || settings.opencode.notifyAwaiting !== false)) markAttention(session);
    App.renderLeft();
    if (store.activeId === session.id) {
      App.renderRight();
      renderComposerCtx(session);
      renderFileChips(session);
      if (session.agent === "opencode") renderQuickControls(session).catch(() => {});
    }
    if (store.view === "todos") renderTodosView();
  }

  /* Re-identify a session by what is actually running inside it (the user may
     launch claude/codex/opencode by hand in a plain shell). Updates icon /
     label / accent and starts that agent's progress watcher on the backend. */
  function detectAgent(session, key) {
    if (!session || !AGENTS[key]) return;
    const a = AGENTS[key];
    if (session.agent !== key) {
      session.agent = key;
      session.agentLabel = a.label;
      session.accent = a.accent;
      if (a.launch) session.launch = a.launch;
      session.tags = [a.label];
      App.renderLeft();
      if (store.activeId === session.id) { showSession(session.id); App.renderRight(); }
    }
    // Codex's TUI repaints aggressively; a blinking cursor on top reads as the
    // whole pane flickering, so pin the cursor steady for codex sessions.
    if (key === "codex" && session.term) {
      try { session.term.options.cursorBlink = false; } catch (_) {}
    }
    // Start the log watcher even when the agent label was already set (restored or
    // Helm-launched session) — the old early-return left progress/conversation empty.
    if (session.ptyId != null && session._watch !== key) {
      session._watch = key;
      invoke("start_agent_watch", { id: session.ptyId, agent: key, cwd: session.cwd }).catch(() => {});
    }
  }
  function altScreenExited(bytes) {
    let s = "";
    for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return s.includes("\x1b[?1049l") || s.includes("\x1b[?47l");
  }
  // An agent CLI was Ctrl+C'd / quit and the bare shell is back: drop the agent
  // label, stop its watcher, and clear its progress so the rail re-syncs.
  function revertToShell(session) {
    if (!session || !session.launch) return;
    const shellKey = session.shell === "cmd" ? "cmd" : session.shell === "wsl" ? "wsl" : "pwsh";
    const a = AGENTS[shellKey];
    session.agent = shellKey;
    session.agentLabel = a.label;
    session.accent = a.accent;
    session.launch = null;
    session._watch = null;
    session.progress = null;
    session.tags = [];
    pushTimeline(session, a.label + " (에이전트 종료)", "blue");
    App.renderLeft();
    if (store.activeId === session.id) { showSession(session.id); App.renderRight(); }
  }
  function detectAgentFromText(s) {
    const t = (s || "").toLowerCase();
    if (t.includes("opencode")) return "opencode";
    if (t.includes("claude")) return "claude";
    if (t.includes("codex")) return "codex";
    return null;
  }
  function renderFileChips(session) {
    const fileChips = $("#cx-file-chips");
    if (!fileChips || !session) return;
    if (session.files && session.files.length) {
      fileChips.innerHTML = session.files.slice(-6)
        .map((f, i) => (i ? '<span class="cx-chip-sep">·</span>' : "") + `<a class="cx-file-chip">${escapeHtml(f)}</a>`)
        .join("");
    } else {
      fileChips.innerHTML = `<a class="cx-file-chip">${escapeHtml(session.cwd || "")}</a>`;
    }
  }
  function sendCmd(cmd) {
    const s = activeSession();
    if (s && s.ptyId != null) {
      invoke("pty_write", { id: s.ptyId, data: cmd + "\r" });
      if (s.term) { try { s.term.focus(); } catch (_) {} }
    }
  }
  function openQuickMenu(anchor, items) {
    document.querySelectorAll(".cx-quick-menu").forEach((m) => m.remove());
    const menu = el("div", "cx-quick-menu");
    items.forEach((it) => {
      const mi = el("button", "cx-quick-mi", it.label);
      mi.addEventListener("click", () => {
        menu.remove();
        if (it.action) it.action();      // real switch (opencode API)
        else if (it.cmd) sendCmd(it.cmd); // slash command (claude/codex)
      });
      menu.appendChild(mi);
    });
    document.body.appendChild(menu);
    const r = anchor.getBoundingClientRect();
    menu.style.left = r.left + "px";
    menu.style.bottom = (window.innerHeight - r.top + 5) + "px";
    const close = (e) => { if (!menu.contains(e.target) && e.target !== anchor) { menu.remove(); document.removeEventListener("mousedown", close); } };
    setTimeout(() => document.addEventListener("mousedown", close), 0);
  }
  async function renderQuickControls(session) {
    const wrap = $("#cx-quick");
    if (!wrap) return;
    // opencode builds its controls from the live HTTP API (async); everyone else
    // uses the static slash-command map.
    let cmds;
    if (session && session.agent === "opencode") {
      try { cmds = await buildOpencodeControls(session); } catch (_) { cmds = []; }
      // a later render may already have replaced this content — guard against it
      if (!wrap.isConnected) return;
    } else {
      cmds = (session && AGENT_CMDS[session.agent]) || [];
    }
    wrap.innerHTML = "";
    (cmds || []).forEach((c) => {
      const b = el("button", "cx-quick-btn", c.menu ? (c.label + " ▾") : c.label);
      if (c.menu) {
        b.addEventListener("click", (e) => { e.stopPropagation(); openQuickMenu(b, c.menu); });
      } else if (c.action) {
        b.addEventListener("click", (e) => { e.stopPropagation(); c.action(); });
      } else {
        b.title = c.cmd + " 보내기";
        b.addEventListener("click", () => sendCmd(c.cmd));
      }
      wrap.appendChild(b);
    });
    // current mode + model chip (opencode only)
    if (session && session.agent === "opencode" && (session.currentMode || session.currentModel)) {
      const chip = el("span", "cx-mode-chip");
      if (session.currentMode) chip.appendChild(el("span", "cx-mode-chip-mode", session.currentMode));
      if (session.currentModel) {
        if (session.currentMode) chip.appendChild(el("span", "cx-mode-chip-sep", "·"));
        chip.appendChild(el("span", "cx-mode-chip-model", session.currentModel));
      }
      if (session.ocPendingModel || session.ocPendingAgent) chip.appendChild(el("span", "cx-mode-chip-pending", "● 대기"));
      wrap.appendChild(chip);
    }
  }

  function renderComposerCtx(s) {
    const wrap = $("#cx-ctx"), nums = $("#cx-ctx-nums"), fill = $("#cx-ctx-fill");
    if (!wrap) return;
    const c = s && s.progress && s.progress.context;
    if (c && c.max) {
      wrap.hidden = false;
      const pct = Math.min(100, Math.round((c.used / c.max) * 100));
      if (nums) nums.textContent = fmtTokens(c.used) + " / " + fmtTokens(c.max) + " · " + pct + "%";
      if (fill) fill.style.width = pct + "%";
    } else {
      wrap.hidden = true;
    }
  }

  /* ================================================================
     LEFT RAIL render
     ================================================================ */
  let _activeOnly = false;
  let _seg = "project";

  function buildRow(s) {
    const row = el("div", "lr-row" + (s.id === store.activeId ? " selected" : "") + (s.status === "attention" ? " attention" : "") + (s.progress && s.progress.status === "working" ? " agent-working" : ""));
    row.dataset.id = s.id;
    row.dataset.status = s.status;
    row.setAttribute("role", "option");
    row.addEventListener("click", () => selectSession(s.id));

    const accent = el("div", "lr-row-accent");
    accent.style.background = ACCENT_HEX[s.accent] || ACCENT_HEX.blue;
    row.appendChild(accent);

    const main = el("div", "lr-row-main");
    const top = el("div", "lr-row-top");
    const dot = el("span", "lr-dot");
    dot.dataset.status = s.status;
    top.appendChild(dot);
    const title = el("span", "lr-title", s.title);
    title.title = s.title;
    top.appendChild(title);
    const uptime = el("span", "lr-uptime", fmtUptime(s.startedAt));
    uptime.dataset.started = String(s.startedAt);
    top.appendChild(uptime);
    main.appendChild(top);

    const bottom = el("div", "lr-row-bottom");
    bottom.appendChild(el("span", "lr-agent lr-agent-" + (s.agent || "pwsh"), s.agentLabel || s.shell || "셸"));
    if (s.branch) {
      const br = el("span", "lr-branch", "⎇ " + s.branch);
      br.title = s.branch;
      bottom.appendChild(br);
    }
    bottom.appendChild(el("span", "lr-status lr-status-" + s.status, statusLabel(s.status)));
    (s.tags || []).slice(0, 2).forEach((t) => bottom.appendChild(el("span", "lr-tag", t)));
    main.appendChild(bottom);

    const act = el("div", "lr-activity");
    act.dataset.st = (s.progress && s.progress.status) || s.status;
    main.appendChild(act);

    row.appendChild(main);
    return row;
  }

  function renderTabs() {
    const bar = $("#cx-tabs");
    if (!bar) return;
    bar.innerHTML = "";
    store.sessions.forEach((s) => {
      const tab = el("div", "cx-tab" + (s.id === store.activeId ? " active" : "") + (s.status === "attention" ? " attention" : ""));
      tab.dataset.id = s.id;
      tab.title = (s.agentLabel || s.shell || "") + " — " + s.title;
      tab.appendChild(el("span", "cx-tab-ic ag-" + (s.agent || "pwsh"), agentIcon(s.agent)));
      tab.appendChild(el("span", "cx-tab-title", s.title));
      const dot = el("span", "cx-tab-dot");
      dot.dataset.st = (s.progress && s.progress.status) || s.status;
      tab.appendChild(dot);
      const x = el("button", "cx-tab-x", "×");
      x.title = "닫기";
      x.addEventListener("click", (e) => { e.stopPropagation(); closeSession(s.id); });
      tab.appendChild(x);
      tab.addEventListener("click", () => selectSession(s.id));
      bar.appendChild(tab);
    });
    const add = el("button", "cx-tab-add", "+");
    add.title = "새 세션";
    add.addEventListener("click", () => openNew(add));
    bar.appendChild(add);
  }

  function renderLeft() {
    renderTabs();
    const list = $("#lr-session-list");
    if (!list) return;
    const visible = _activeOnly
      ? store.sessions.filter((s) => s.status === "running" || s.status === "active")
      : store.sessions;
    list.innerHTML = "";
    if (!visible.length) {
      list.appendChild(el("div", "lr-empty", "세션 없음"));
      return;
    }
    if (_seg === "project") {
      const groups = {};
      visible.forEach((s) => { const k = projectName(s.cwd); (groups[k] = groups[k] || []).push(s); });
      Object.entries(groups).forEach(([name, arr]) => {
        const h = el("div", "lr-group-label");
        h.appendChild(el("span", "lr-group-name", name));
        h.appendChild(el("span", "lr-group-count", String(arr.length)));
        list.appendChild(h);
        arr.forEach((s) => list.appendChild(buildRow(s)));
      });
    } else {
      visible.forEach((s) => list.appendChild(buildRow(s)));
    }
  }

  /* ================================================================
     RIGHT RAIL render
     ================================================================ */
  function barColorClass(p) { return p >= 80 ? "rr-bar-red" : p >= 50 ? "rr-bar-orange" : "rr-bar-green"; }
  function valColorClass(p) { return p >= 80 ? "rr-red" : p >= 50 ? "rr-orange" : "rr-green"; }

  function renderSystemCard(stats) {
    const cpu = Math.min(100, Math.max(0, stats.cpu || 0));
    const mem = Math.min(100, Math.max(0, stats.mem || 0));
    const set = (barId, valId, pct, label) => {
      const bar = $("#" + barId), val = $("#" + valId);
      if (!bar) return;
      bar.style.width = pct + "%";
      val.textContent = label != null ? label : pct.toFixed(0) + "%";
    };
    const barCpu = $("#rr-bar-cpu"), valCpu = $("#rr-val-cpu");
    if (barCpu) {
      barCpu.style.width = cpu + "%";
      barCpu.className = "rr-bar " + barColorClass(cpu);
      valCpu.textContent = cpu.toFixed(0) + "%";
      valCpu.className = "rr-usage-val " + valColorClass(cpu);
    }
    const barMem = $("#rr-bar-mem"), valMem = $("#rr-val-mem");
    if (barMem) {
      barMem.style.width = mem + "%";
      barMem.className = "rr-bar " + barColorClass(mem);
      valMem.textContent = mem.toFixed(0) + "%";
      valMem.className = "rr-usage-val " + valColorClass(mem);
    }
    // session uptime (active session), capped to 4h for the bar
    const s = activeSession();
    const barSess = $("#rr-bar-sess"), valSess = $("#rr-val-sess");
    if (barSess) {
      if (s) {
        const up = Date.now() - s.startedAt;
        barSess.style.width = Math.min(100, (up / (4 * 3600 * 1000)) * 100) + "%";
        valSess.textContent = fmtUptime(s.startedAt);
      } else {
        barSess.style.width = "0%";
        valSess.textContent = "—";
      }
    }
  }

  function renderRight() {
    const s = activeSession();

    // active-session info card
    const setTxt = (id, v) => { const e = $("#" + id); if (e) e.textContent = v; };
    if (s) {
      setTxt("rr-sess-agent", s.agentLabel || s.shell || "셸");
      const stEl = $("#rr-sess-status");
      if (stEl) { stEl.textContent = statusLabel(s.status); stEl.className = "rr-info-val rr-st-" + s.status; }
      setTxt("rr-sess-cwd", s.cwd || "—");
      const cwdEl = $("#rr-sess-cwd"); if (cwdEl) cwdEl.title = s.cwd || "";
      setTxt("rr-sess-branch", s.branch ? "⎇ " + s.branch : "—");
      setTxt("rr-sess-uptime", fmtUptime(s.startedAt));
    } else {
      ["rr-sess-agent", "rr-sess-cwd", "rr-sess-branch", "rr-sess-uptime"].forEach((id) => setTxt(id, "—"));
      const stEl = $("#rr-sess-status"); if (stEl) { stEl.textContent = "—"; stEl.className = "rr-info-val"; }
    }

    // opencode mode/model row (hidden for other agents)
    const modeRow = $("#rr-sess-mode-row");
    if (modeRow) {
      const showMode = s && s.agent === "opencode" && (s.currentMode || s.currentModel);
      modeRow.hidden = !showMode;
      if (showMode) {
        setTxt("rr-sess-mode", (s.currentMode || "—") + (s.currentModel ? " · " + s.currentModel : ""));
      }
    }

    // agent progress: status pill + activity
    const prog = s && s.progress;
    const pill = $("#rr-prog-status");
    if (pill) {
      const m = (prog && AGENT_ST[prog.status]) || null;
      pill.textContent = m ? m[0] : "—";
      pill.className = "rr-prog-pill" + (m && m[1] ? " " + m[1] : "");
    }
    const act = $("#rr-prog-activity");
    if (act) act.textContent = (prog && prog.activity) || (s ? "활동 대기 중" : "세션 없음");

    // granular current sub-step (tool + target + step counter)
    const sub = $("#rr-prog-sub");
    if (sub) {
      const ct = prog && prog.current_tool;
      const step = prog && prog.step_display;
      if (ct || step) {
        sub.hidden = false;
        sub.innerHTML = "";
        if (ct) {
          const stMark = ct.status === "completed" ? "✓" : ct.status === "error" ? "✕" : "▸";
          const chip = el("span", "rr-substep-tool is-" + (ct.status || "running"));
          chip.appendChild(el("span", "rr-substep-ic", toolIcon(ct.name)));
          chip.appendChild(el("span", "rr-substep-name", ct.name));
          if (ct.target) chip.appendChild(el("span", "rr-substep-tgt", ct.target));
          chip.appendChild(el("span", "rr-substep-st", stMark));
          sub.appendChild(chip);
        }
        if (step) sub.appendChild(el("span", "rr-substep-step", "단계 " + step));
      } else {
        sub.hidden = true;
      }
    }

    // context meter (real token usage when the agent reports it)
    const ctxWrap = $("#rr-ctx-wrap"), ctxBar = $("#rr-ctx-bar"), ctxNums = $("#rr-ctx-nums");
    if (ctxWrap) {
      const c = prog && prog.context;
      if (c && c.max) {
        ctxWrap.hidden = false;
        const pct = Math.min(100, Math.round((c.used / c.max) * 100));
        if (ctxBar) { ctxBar.style.width = pct + "%"; ctxBar.className = "rr-bar " + barColorClass(pct); }
        if (ctxNums) ctxNums.textContent = fmtTokens(c.used) + " / " + fmtTokens(c.max) + " · " + pct + "%";
      } else {
        ctxWrap.hidden = true;
      }
    }

    // todos (real, e.g. claude TodoWrite)
    const todoList = $("#rr-todo-list"), todoScore = $("#rr-todo-score");
    if (todoList) {
      const todos = (prog && prog.todos) || [];
      todoList.innerHTML = "";
      if (!todos.length) {
        todoList.appendChild(el("div", "rr-empty", "—"));
        if (todoScore) todoScore.textContent = "";
      } else {
        const done = todos.filter((t) => t.status === "completed").length;
        if (todoScore) todoScore.textContent = done + "/" + todos.length;
        const activeIdx = (prog && typeof prog.active_todo_index === "number") ? prog.active_todo_index : -1;
        todos.forEach((t, i) => {
          const item = el("div", "rr-todo-item is-" + (t.status || "pending"));
          if (i === activeIdx) item.classList.add("is-active");
          const mark = t.status === "completed" ? "✓" : t.status === "in_progress" ? "▸" : "○";
          item.appendChild(el("span", "rr-todo-check", mark));
          item.appendChild(el("span", "rr-todo-text", t.text || ""));
          todoList.appendChild(item);
        });
      }
    }

    // 도구 / 플러그인 used by the active session's agent
    const toolsBox = $("#rr-tools");
    if (toolsBox) {
      const stats = (s && s.toolStats) || null;
      const entries = stats ? Object.entries(stats).sort((a, b) => b[1] - a[1]) : [];
      toolsBox.innerHTML = "";
      if (!entries.length) {
        toolsBox.appendChild(el("div", "rr-empty", "—"));
      } else {
        entries.slice(0, 24).forEach(([name, count]) => {
          const isMcp = name.startsWith("mcp__");
          const chip = el("span", "rr-tool" + (isMcp ? " is-mcp" : ""));
          const label = isMcp ? "⧉ " + name.replace(/^mcp__/, "").replace(/__/g, "·") : name;
          chip.appendChild(el("span", "rr-tool-name", label));
          if (count > 1) chip.appendChild(el("span", "rr-tool-n", String(count)));
          toolsBox.appendChild(chip);
        });
      }
    }

    // timeline
    const tl = $("#rr-timeline");
    if (tl) {
      const entries = (s && s.timeline) || [];
      tl.innerHTML = "";
      if (!entries.length) {
        tl.appendChild(el("div", "rr-empty", "활동 없음"));
      } else {
        entries.slice().reverse().forEach((e) => {
          const row = el("div", "rr-tl-item");
          row.appendChild(el("span", "rr-tl-dot rr-tl-dot-" + (e.color || "blue")));
          row.appendChild(el("span", "rr-tl-text", e.text));
          const meta = el("div", "rr-tl-meta");
          meta.appendChild(el("span", "rr-tl-time", fmtClock(e.time)));
          if (e.dur != null) meta.appendChild(el("span", "rr-tl-dur", e.dur));
          row.appendChild(meta);
          tl.appendChild(row);
        });
      }
    }
  }

  /* ================================================================
     TOP BAR + PAGE HEADER
     ================================================================ */
  function setConnection(v) {
    store.connection = v;
    const pill = $("#conn-pill");
    if (pill) {
      pill.dataset.state = v;
      const lbl = pill.querySelector(".conn-label");
      if (lbl) lbl.textContent = v === "connected" ? "Connected" : v === "error" ? "Disconnected" : "Connecting";
    }
  }

  // Light the wifi indicator while one or more phones are linked over the LAN.
  function setMobileClients(count) {
    const n = Math.max(0, count | 0);
    const wifi = $("#tb-wifi");
    const mob = $("#tb-mobile");
    if (wifi) {
      wifi.hidden = n === 0;
      wifi.title = n === 0 ? "" : (n === 1 ? "모바일 연결됨" : `모바일 ${n}대 연결됨`);
      const nb = $("#tb-wifi-n");
      if (nb) nb.textContent = n > 1 ? String(n) : "";
    }
    if (mob) mob.classList.toggle("linked", n > 0);
  }

  function renderHeaderChips() {
    const sessions = store.sessions;
    const set = (sel, v) => { const e = $(sel); if (e) e.textContent = String(v); };
    set("#chip-active-n", runningCount());
    set("#chip-loaded-n", sessions.length);

    const counts = {};
    for (const s of sessions) { const k = projectName(s.cwd); counts[k] = (counts[k] || 0) + 1; }
    const parts = Object.entries(counts).map(([k, n]) => `${k} ${n}`);
    const chip = $("#chip-projects");
    if (chip) { chip.textContent = parts.join(" · "); chip.style.display = parts.length ? "" : "none"; }
  }

  function pushNotification(n) {
    store.notifications.unshift(Object.assign({ ts: Date.now(), read: false }, n));
    renderUnread();
  }
  function renderUnread() {
    const unread = store.notifications.filter((n) => !n.read).length;
    const dot = $("#tb-unread");
    if (dot) { dot.hidden = unread === 0; dot.textContent = unread > 9 ? "9+" : String(unread); }
    const list = $("#bell-list");
    if (list) {
      if (!store.notifications.length) {
        list.innerHTML = '<div class="dd-empty">에이전트 승인 및 알림이 여기에 표시됩니다.</div>';
      } else {
        list.innerHTML = "";
        store.notifications.slice(0, 30).forEach((it) => {
          const row = el("div", "dd-item");
          row.innerHTML = `<div class="dd-item-title">${escapeHtml(it.title || "알림")}</div>` +
            (it.body ? `<div class="dd-item-body u-muted">${escapeHtml(it.body)}</div>` : "");
          list.appendChild(row);
        });
      }
    }
  }

  function renderTodosView() {
    const list = $("#tv-list"), score = $("#tv-score");
    if (!list) return;
    list.innerHTML = "";
    // "현재 활성" board: one card per running agent session (claude/codex/opencode)
    // showing what it's doing — title, status + elapsed, agent/folder, context bar.
    const agents = store.sessions.filter((s) => AGENTS[s.agent] && AGENTS[s.agent].launch);
    if (score) score.textContent = agents.length ? (agents.length + "개 세션") : "";
    if (!agents.length) {
      const empty = el("div", "tv-empty");
      empty.appendChild(el("div", "tv-empty-ic", "✓"));
      empty.appendChild(el("div", "tv-empty-title", "활성 에이전트 세션이 없습니다"));
      empty.appendChild(el("div", "tv-empty-sub", "claude · codex · opencode 세션을 시작하면 진행 중인 작업이 여기에 한눈에 모입니다."));
      list.appendChild(empty);
      return;
    }

    const fmtTok = (n) => n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "k" : String(n);
    const statusText = (st) => ({ working: "작업 중", waiting: "입력 대기", attention: "입력 대기", idle: "대기 중", done: "완료됨", error: "오류", spawning: "시작 중", running: "실행 중" }[st] || st || "대기 중");

    agents.forEach((s) => {
      const prog = s.progress || {};
      const st = prog.status || s.status || "idle";
      const ctx = prog.context;

      const card = el("div", "tv-card is-" + st);
      card.addEventListener("click", () => { selectSession(s.id); gotoSessionsView(); });

      const head = el("div", "tv-card-head");
      head.appendChild(el("span", "tv-dot is-" + st));
      head.appendChild(el("span", "tv-card-title", s.title || projectName(s.cwd)));
      const right = el("div", "tv-card-right");
      right.appendChild(el("span", "tv-status is-" + st, statusText(st)));
      const elSpan = el("span", "tv-elapsed", "⏱ " + fmtAge(Date.now() - (s.startedAt || Date.now())));
      elSpan.dataset.started = s.startedAt || Date.now();
      right.appendChild(elSpan);
      head.appendChild(right);
      card.appendChild(head);

      const meta = el("div", "tv-meta");
      meta.appendChild(el("span", "tv-meta-agent", s.agentLabel || s.agent));
      meta.appendChild(el("span", "tv-meta-sep", "·"));
      meta.appendChild(el("span", "tv-meta-folder", "📁 " + projectName(s.cwd)));
      if (s.branch) { meta.appendChild(el("span", "tv-meta-sep", "·")); meta.appendChild(el("span", "tv-meta-branch", s.branch)); }
      card.appendChild(meta);

      // opencode mode (e.g. "Sisyphus - Ultraworker") + model chip
      if (s.agent === "opencode" && (s.currentMode || s.currentModel)) {
        const mr = el("div", "tv-mode");
        mr.appendChild(el("span", "tv-mode-label", "모드"));
        mr.appendChild(el("span", "tv-mode-val", s.currentMode || "—"));
        if (s.currentModel) {
          mr.appendChild(el("span", "tv-mode-sep", "·"));
          mr.appendChild(el("span", "tv-mode-model", s.currentModel));
        }
        card.appendChild(mr);
      }

      const act = (prog.activity || "").trim();
      if (act) {
        const actRow = el("div", "tv-activity");
        actRow.appendChild(el("span", "tv-activity-mark is-" + st, st === "working" ? "▸" : "•"));
        actRow.appendChild(el("span", "tv-activity-text", act));
        card.appendChild(actRow);
      }

      // granular sub-step: current tool + step counter
      if (prog.current_tool || prog.step_display) {
        const subRow = el("div", "tv-substep");
        if (prog.current_tool) {
          subRow.appendChild(el("span", "tv-substep-ic", toolIcon(prog.current_tool.name)));
          subRow.appendChild(el("span", "tv-substep-name", prog.current_tool.name));
          if (prog.current_tool.target) subRow.appendChild(el("span", "tv-substep-tgt", prog.current_tool.target));
        }
        if (prog.step_display) subRow.appendChild(el("span", "tv-substep-step", "단계 " + prog.step_display));
        card.appendChild(subRow);
      }

      const ctxRow = el("div", "tv-ctx");
      ctxRow.appendChild(el("span", "tv-ctx-label", "컨텍스트"));
      const bar = el("div", "tv-ctx-bar");
      const fill = el("div", "tv-ctx-fill is-" + st);
      fill.style.width = (ctx ? Math.min(100, ctx.pct) : 0) + "%";
      bar.appendChild(fill);
      ctxRow.appendChild(bar);
      ctxRow.appendChild(el("span", "tv-ctx-num", ctx ? (fmtTok(ctx.used) + " / " + fmtTok(ctx.max)) : "—"));
      card.appendChild(ctxRow);

      list.appendChild(card);
    });
  }
  function switchView(view) {
    const tv = $("#todos-view");
    if (tv) tv.hidden = (view !== "todos");
    if (view === "todos") renderTodosView();
  }
  function gotoSessionsView() {
    store.view = "dashboard";
    $$("#tb-nav .tb-tab").forEach((t) => t.classList.toggle("active", t.dataset.view === "dashboard"));
    switchView("dashboard");
  }

  function wireTopbar() {
    $$("#tb-nav .tb-tab").forEach((tab) => {
      tab.addEventListener("click", () => {
        $$("#tb-nav .tb-tab").forEach((t) => t.classList.toggle("active", t === tab));
        store.view = tab.dataset.view;
        switchView(store.view);
      });
    });

    const bell = $("#tb-bell"), dd = $("#bell-dropdown");
    if (bell && dd) {
      bell.addEventListener("click", (e) => {
        e.stopPropagation();
        const open = dd.hidden;
        dd.hidden = !open;
        bell.setAttribute("aria-expanded", String(open));
        if (open) { store.notifications.forEach((n) => (n.read = true)); renderUnread(); }
      });
      document.addEventListener("click", (e) => {
        if (!dd.hidden && !dd.contains(e.target) && e.target !== bell) {
          dd.hidden = true; bell.setAttribute("aria-expanded", "false");
        }
      });
    }

    const gear = $("#tb-gear");
    if (gear) gear.addEventListener("click", openSettings);

    const mob = $("#tb-mobile");
    if (mob) mob.addEventListener("click", async () => {
      try {
        const info = await invoke("mobile_info");
        const u = $("#mob-url"); if (u) u.value = info.url || "";
        const set = (id, v) => { const e = $("#" + id); if (e) e.textContent = v; };
        set("mob-ip", info.lan_ip || "—");
        set("mob-http", info.http_port || "—");
        set("mob-ws", info.ws_port || "—");
      } catch (_) {}
      const m = $("#mobile-modal"); if (m) m.hidden = false;
    });
    const mClose = $("#mob-close");
    if (mClose) mClose.addEventListener("click", () => { const m = $("#mobile-modal"); if (m) m.hidden = true; });
    const mCopy = $("#mob-copy");
    if (mCopy) mCopy.addEventListener("click", () => { const u = $("#mob-url"); if (u) { u.select(); clipWriteText(u.value); } });
    const mModal = $("#mobile-modal");
    if (mModal) mModal.addEventListener("click", (e) => { if (e.target === mModal) mModal.hidden = true; });

    try {
      const getWin = TAURI.window && TAURI.window.getCurrentWindow;
      if (getWin) {
        const win = getWin();
        const min = $("#wc-min"), max = $("#wc-max"), close = $("#wc-close");
        if (min) min.addEventListener("click", () => win.minimize());
        if (max) max.addEventListener("click", () => win.toggleMaximize());
        if (close) close.addEventListener("click", () => win.close());
      }
    } catch (_) {}

    // ---- mobile drawer toggles (no-op on desktop; buttons hidden via CSS) ----
    (function mobileDrawers() {
      const nav = document.getElementById("tb-nav");
      const right = document.querySelector(".tb-right");
      const body = document.body;
      const closeAll = () => body.classList.remove("mnav-open", "mstat-open");

      // hamburger -> left rail (session list), placed before the 세션/작업 tabs
      if (nav && !document.getElementById("m-nav-btn")) {
        const b = document.createElement("button");
        b.id = "m-nav-btn";
        b.className = "m-drawer-btn";
        b.type = "button";
        b.title = "세션 목록";
        b.setAttribute("aria-label", "세션 목록 열기");
        b.innerHTML = "&#9776;"; // ☰
        b.addEventListener("click", (e) => {
          e.stopPropagation();
          const open = body.classList.contains("mnav-open");
          closeAll();
          if (!open) body.classList.add("mnav-open");
        });
        nav.parentNode.insertBefore(b, nav);
      }

      // status icon -> right rail (세션 상태), placed first in .tb-right
      if (right && !document.getElementById("m-stat-btn")) {
        const b = document.createElement("button");
        b.id = "m-stat-btn";
        b.className = "m-drawer-btn";
        b.type = "button";
        b.title = "세션 상태";
        b.setAttribute("aria-label", "세션 상태 열기");
        b.innerHTML = "&#128202;"; // 📊
        b.addEventListener("click", (e) => {
          e.stopPropagation();
          const open = body.classList.contains("mstat-open");
          closeAll();
          if (!open) body.classList.add("mstat-open");
        });
        right.insertBefore(b, right.firstChild);
      }

      // tap the scrim, or pick a session, to close the drawer
      const shell = document.getElementById("app-shell");
      if (shell) shell.addEventListener("click", (e) => {
        if (body.classList.contains("mnav-open") || body.classList.contains("mstat-open")) {
          const inLeft = e.target.closest("#left-rail");
          const inRight = e.target.closest("#right-rail");
          if (!inLeft && !inRight) closeAll();                    // tapped scrim/center
          else if (inLeft && e.target.closest(".lr-row")) closeAll(); // picked a session
        }
      });
      document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeAll(); });
    })();
  }

  /* ================================================================
     LEFT/CENTER/COMPOSER wiring
     ================================================================ */
  function defaultNewCwd() {
    const act = activeSession();
    return store.lastCwd || (act && act.cwd) || store.home;
  }
  function openNew(anchorBtn) {
    if (settings.defaultAgent && settings.defaultAgent !== "ask") {
      const cwd = defaultNewCwd();
      store.lastCwd = cwd;
      newSession({ agent: settings.defaultAgent, cwd });
    } else {
      openAgentMenu(anchorBtn);
    }
  }

  function openAgentMenu(anchorBtn) {
    document.querySelectorAll(".ns-menu").forEach((m) => m.remove());
    const menu = el("div", "ns-menu");

    // working-folder input (paste/edit the path the new session starts in)
    const cwdRow = el("div", "ns-cwd-row");
    const cwdInput = el("input", "ns-cwd");
    cwdInput.type = "text";
    cwdInput.value = defaultNewCwd();
    cwdInput.placeholder = "작업 폴더 경로";
    cwdInput.spellcheck = false;
    cwdRow.appendChild(el("span", "ns-cwd-label", "폴더"));
    cwdRow.appendChild(cwdInput);
    const browseBtn = el("button", "ns-browse", "찾아보기");
    browseBtn.addEventListener("click", async (ev) => {
      ev.preventDefault();
      try {
        const dlg = TAURI.dialog;
        if (!dlg || !dlg.open) { cwdInput.focus(); return; }
        const sel = await dlg.open({ directory: true, defaultPath: cwdInput.value || store.home });
        if (sel) cwdInput.value = sel;
      } catch (_) {}
    });
    cwdRow.appendChild(browseBtn);
    menu.appendChild(cwdRow);

    const pick = (key) => {
      menu.remove();
      const cwd = (cwdInput.value || "").trim() || store.home;
      store.lastCwd = cwd;
      newSession({ agent: key, cwd });
    };
    cwdInput.addEventListener("keydown", (e) => { if (e.key === "Enter") pick("pwsh"); });

    AGENT_ORDER.forEach((key) => {
      const a = AGENTS[key];
      const item = el("button", "ns-item");
      const dot = el("span", "ns-dot");
      dot.style.background = ACCENT_HEX[a.accent] || ACCENT_HEX.green;
      item.appendChild(dot);
      item.appendChild(el("span", "ns-item-label", a.label));
      item.addEventListener("click", () => pick(key));
      menu.appendChild(item);
    });

    document.body.appendChild(menu);
    const r = anchorBtn.getBoundingClientRect();
    menu.style.left = r.left + "px";
    menu.style.top = (r.bottom + 4) + "px";
    menu.style.minWidth = Math.max(300, r.width) + "px";
    cwdInput.focus();
    cwdInput.select();
    const close = (e) => { if (!menu.contains(e.target) && e.target !== anchorBtn) { menu.remove(); document.removeEventListener("mousedown", close); } };
    setTimeout(() => document.addEventListener("mousedown", close), 0);
  }

  function wireLeft() {
    const newBtn = $("#lr-new-session");
    if (newBtn) newBtn.addEventListener("click", () => openNew(newBtn));

    $$(".lr-seg").forEach((btn) => {
      btn.addEventListener("click", () => {
        $$(".lr-seg").forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        _seg = btn.dataset.seg || "project";
        renderLeft();
      });
    });
    const cb = $("#lr-active-only");
    if (cb) cb.addEventListener("change", () => { _activeOnly = cb.checked; renderLeft(); });
  }

  async function composerSend() {
    const input = $("#cx-input");
    const s = activeSession();
    if (!input || !s || s.ptyId == null) return;
    const text = input.value;
    if (!text.length) return;

    // opencode real switch: when a model/agent is pending AND we have an API port
    // + bound session id, send through opencode_send so the selection is applied
    // and persisted on this turn. Falls back to the PTY on any failure.
    const ocSwitching = s.agent === "opencode"
      && (s.ocPendingModel || s.ocPendingAgent)
      && s._ocPort && s.ocSessionId
      && !(settings.opencode && settings.opencode.apiSwitch === false);
    if (ocSwitching) {
      let ok = false;
      try {
        ok = await invoke("opencode_send", {
          port: s._ocPort,
          sessionId: s.ocSessionId,
          text,
          modelId: s.ocPendingModel || null,
          provider: s.ocPendingProvider || null,
          agent: s.ocPendingAgent || null,
        });
      } catch (_) { ok = false; }
      if (ok) {
        if (s.ocPendingModel) s.currentModel = s.ocPendingModel;
        if (s.ocPendingAgent) s.currentMode = (s.ocPendingAgent || "").replace(/^​/, "");
        s.ocPendingModel = null; s.ocPendingAgent = null; s.ocPendingProvider = null;
        s.msgCount = (s.msgCount || 0) + 1;
        const msgN2 = $("#cx-msg-n"); if (msgN2) msgN2.textContent = String(s.msgCount);
        pushTimeline(s, "메시지 전송 (모델/에이전트 전환 적용)", "purple");
        input.value = ""; input.style.height = "";
        renderQuickControls(s).catch(() => {});
        return;
      }
      // fall through to PTY send + native picker if the API switch failed
      pushTimeline(s, "API 전환 실패 — 터미널로 전송", "orange");
    }

    invoke("pty_write", { id: s.ptyId, data: text + "\r" });
    s.msgCount = (s.msgCount || 0) + 1;
    const msgN = $("#cx-msg-n");
    if (msgN) msgN.textContent = String(s.msgCount);
    pushTimeline(s, "메시지 전송", "blue");
    input.value = "";
    input.style.height = "";
  }

  function wireCenter() {
    const tog = $("#cx-view-toggle");
    if (tog) tog.querySelectorAll(".cvt").forEach((b) => b.addEventListener("click", () => {
      const s = activeSession(); if (!s) return;
      s.convView = (b.dataset.cv === "conv");
      saveSessionState();
      showSession(s.id);
    }));
    const input = $("#cx-input");
    if (input) {
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); composerSend(); }
      });
      input.addEventListener("input", () => {
        input.style.height = "auto";
        input.style.height = Math.min(input.scrollHeight, 140) + "px";
      });
    }
    const sendBtn = $("#cx-send-btn");
    if (sendBtn) sendBtn.addEventListener("click", composerSend);

    const editBtn = $("#cx-btn-edit");
    if (editBtn) editBtn.addEventListener("click", () => {
      const s = activeSession(); if (!s) return;
      const t = prompt("세션 제목 수정:", s.title);
      if (t !== null && t.trim()) renameSession(s.id, t.trim());
    });
    const pinBtn = $("#cx-btn-pin");
    if (pinBtn) pinBtn.addEventListener("click", () => { const s = activeSession(); if (s) togglePin(s.id); });
    const delBtn = $("#cx-btn-delete");
    if (delBtn) delBtn.addEventListener("click", () => {
      const s = activeSession(); if (!s) return;
      if (confirm(`세션 "${s.title}" 을(를) 삭제하시겠습니까?`)) closeSession(s.id);
    });
  }

  function wireHeader() {
    const back = $("#ph-back");
    if (back) back.addEventListener("click", () => { store.activeId = null; renderLeft(); showSession(null); renderRight(); });
  }

  /* ================================================================
     TICKERS + POLLING
     ================================================================ */
  function startUptimeTicker() {
    setInterval(() => {
      const list = $("#lr-session-list");
      if (list) {
        list.querySelectorAll(".lr-uptime[data-started]").forEach((node) => {
          const started = parseInt(node.dataset.started, 10);
          if (!isNaN(started)) node.textContent = fmtUptime(started);
        });
      }
      const s = activeSession();
      const valSess = $("#rr-val-sess");
      if (s && valSess) valSess.textContent = fmtUptime(s.startedAt);
      // live elapsed in the tasks board
      $$(".tv-elapsed[data-started]").forEach((node) => {
        const st = parseInt(node.dataset.started, 10);
        if (!isNaN(st)) node.textContent = "⏱ " + fmtAge(Date.now() - st);
      });
    }, 1000);
  }

  async function pollSystemStats() {
    try {
      const stats = await invoke("system_stats");
      renderSystemCard(stats);
    } catch (err) {
      renderSystemCard({ cpu: 0, mem: 0 });
    }
  }

  /* USAGE account cards from a local usage endpoint. */
  function fmtResets(iso) {
    const t = Date.parse(iso);
    if (isNaN(t)) return "";
    let s = Math.max(0, Math.floor((t - Date.now()) / 1000));
    const h = Math.floor(s / 3600); s -= h * 3600;
    const m = Math.floor(s / 60);
    return "resets " + (h > 0 ? h + "h " : "") + m + "m";
  }
  function renderUsageCards(cards) {
    const box = $("#rr-usage-cards");
    if (!box) return;
    box.innerHTML = "";
    (cards || []).forEach((card) => {
      const c = el("div", "rr-card");
      const head = el("div", "rr-card-head");
      head.appendChild(el("span", "rr-acct-label", card.account || "account"));
      if (card.plan) head.appendChild(el("span", "rr-plan-badge", card.plan));
      c.appendChild(head);
      (card.rows || []).forEach((r) => {
        let pct = r.pct || 0;
        if (pct <= 1) pct = pct * 100;
        pct = Math.round(pct);
        const row = el("div", "rr-usage-row");
        row.appendChild(el("span", "rr-usage-key", r.label));
        const wrap = el("div", "rr-bar-wrap");
        const bar = el("div", "rr-bar " + barColorClass(pct));
        bar.style.width = Math.min(100, pct) + "%";
        wrap.appendChild(bar);
        row.appendChild(wrap);
        const reset = r.resets_at ? " · " + fmtResets(r.resets_at) : "";
        row.appendChild(el("span", "rr-usage-val " + valColorClass(pct), pct + "%" + reset));
        c.appendChild(row);
      });
      if (card.extra) c.appendChild(el("div", "rr-ph-note", "추가 사용 가능"));
      box.appendChild(c);
    });
  }
  async function pollUsage() {
    try { renderUsageCards(await invoke("usage_cards")); } catch (_) {}
  }
  function startUsagePolling() {
    pollUsage();
    // poll the backend CACHE often (cheap); the backend itself refreshes from the
    // rate-limited upstream only every 60s.
    setInterval(pollUsage, 5000);
  }
  let _statsTimer = null;
  function startStatsPolling() {
    pollSystemStats();
    if (_statsTimer) clearInterval(_statsTimer);
    _statsTimer = setInterval(pollSystemStats, settings.statsInterval || 2000);
  }

  /* ================================================================
     Public surface
     ================================================================ */
  const App = {
    store,
    newSession, selectSession, closeSession, showSession,
    renameSession, togglePin,
    renderLeft, renderRight, renderTabs, renderHeaderChips,
    setConnection, pushNotification,
    sessionById, activeSession,
    applyProgress,
    fmtUptime, fmtClock,
  };
  window.App = App;

  /* ================================================================
     MAIN
     ================================================================ */
  // App-level keyboard shortcuts. Registered on the capture phase so we can claim
  // a chord before xterm's textarea sees it, but we only ever swallow the chords
  // we own — plain Ctrl+<letter> stays with the terminal (it's readline's), so we
  // live in the Ctrl+Shift+<letter> / Ctrl+<digit> / Ctrl+Tab space instead.
  function wireShortcuts() {
    const indexOfActive = () => store.sessions.findIndex((s) => s.id === store.activeId);
    const jump = (i) => { const s = store.sessions[i]; if (s) selectSession(s.id); };
    const cycle = (dir) => {
      const n = store.sessions.length; if (!n) return;
      const cur = indexOfActive();
      jump((((cur < 0 ? 0 : cur) + dir) % n + n) % n);
    };
    const fontStep = (d) => {
      settings.fontSize = Math.min(32, Math.max(8, Math.round((settings.fontSize + d) * 2) / 2));
      saveSettings(); applySettings();
    };
    const toggleConv = () => { const s = activeSession(); if (!s) return; s.convView = !s.convView; saveSessionState(); showSession(s.id); };
    const clearScrollback = () => { const s = activeSession(); if (s && s.term) { try { s.term.clear(); } catch (_) {} } };

    document.addEventListener("keydown", (e) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const k = e.key, shift = e.shiftKey;
      let handled = true;

      if (k === "Tab") cycle(shift ? -1 : 1);
      else if (!shift && k >= "1" && k <= "8") jump(parseInt(k, 10) - 1);
      else if (!shift && k === "9") jump(store.sessions.length - 1);
      else if (shift && (k === "T" || k === "t")) openNew($("#lr-new-session"));
      else if (shift && (k === "W" || k === "w")) { if (store.activeId != null) closeSession(store.activeId); }
      else if (shift && (k === "M" || k === "m")) toggleConv();
      else if (shift && (k === "K" || k === "k")) clearScrollback();
      else if (shift && (k === "U" || k === "u")) { const b = $("#tb-bell"); if (b) b.click(); }
      else if (!shift && k === ",") openSettings();
      else if (k === "=" || k === "+") fontStep(0.5);
      else if (k === "-" || k === "_") fontStep(-0.5);
      else if (!shift && k === "0") { settings.fontSize = DEFAULT_SETTINGS.fontSize; saveSettings(); applySettings(); }
      else handled = false;

      if (handled) { e.preventDefault(); e.stopPropagation(); }
    }, true);
  }

  async function main() {
    wireTopbar();
    wireShortcuts();
    wireHeader();
    wireLeft();
    wireCenter();
    wireSettings();
    setConnection("connecting");
    renderHeaderChips();
    renderUnread();
    renderTabs();
    renderRight();
    startUptimeTicker();
    applySettings();
    startUsagePolling();
    setMobileClients(0);
    try { listen("mobile-clients", (e) => setMobileClients((e.payload && e.payload.count) || 0)); } catch (_) {}

    let home = "";
    try { home = await invoke("app_home"); } catch (_) {}
    if (home) store.home = home;
    setConnection(home ? "connected" : "error");

    const saved = loadSessionState();
    if (settings.restoreSessions && saved && Array.isArray(saved.sessions) && saved.sessions.length) {
      // restore previous sessions: reopen shells at their cwd; agents resume
      for (const meta of saved.sessions) {
        const sess = await newSession({ agent: meta.agent || "pwsh", cwd: meta.cwd || store.home, title: meta.title, resume: true });
        if (sess) { if (meta.titleLocked) sess.titleLocked = true; if (meta.pinned) sess.pinned = true; if (meta.convView) sess.convView = true; }
      }
      renderLeft();
      const act = store.sessions[saved.activeIndex] || store.sessions[0];
      if (act) selectSession(act.id); else showSession(null);
    } else {
      const s1 = await newSession({ agent: "pwsh", cwd: store.home, title: projectName(store.home) });
      if (s1) selectSession(s1.id); else showSession(null);
    }

    // persist the session list so a restart / reboot can restore it
    setInterval(saveSessionState, 2000);
    window.addEventListener("beforeunload", saveSessionState);
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", main);
  else main();
})();
