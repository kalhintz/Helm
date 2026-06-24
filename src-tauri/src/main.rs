// Helm — native Tauri shell. Terminal backend uses portable-pty (ConPTY on
// Windows), so there is no Electron and no external sidecar process.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod agent_watch;
mod hook_server;

use std::collections::HashMap;
use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::Mutex;

/// Windows CREATE_NO_WINDOW — keeps helper processes (git/netstat) from flashing a
/// console window.
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

struct PtyInstance {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

#[derive(Default)]
struct PtyState {
    map: Mutex<HashMap<u32, PtyInstance>>,
    next: Mutex<u32>,
}

#[derive(Clone, Serialize)]
struct DataPayload {
    b64: String,
}

fn resolve_shell(shell: &str) -> String {
    #[cfg(windows)]
    {
        match shell {
            "" | "powershell" => "powershell.exe".into(),
            "pwsh" => "pwsh.exe".into(),
            "cmd" => "cmd.exe".into(),
            "wsl" => "wsl.exe".into(),
            other => other.into(),
        }
    }
    #[cfg(not(windows))]
    {
        let login = || std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        match shell {
            // Windows shell names sent by the frontend fall back to the login shell.
            "" | "default" | "powershell" | "pwsh" | "cmd" | "wsl" => login(),
            "bash" => "/bin/bash".into(),
            "zsh" => "/bin/zsh".into(),
            "sh" => "/bin/sh".into(),
            other => other.into(),
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn pty_spawn(
    app: AppHandle,
    state: State<PtyState>,
    shell: String,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    _workspace_id: Option<String>,
    _surface_id: Option<String>,
    agent: Option<String>,
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let cwd_for_watch = cwd.clone().unwrap_or_default();
    let mut cmd = CommandBuilder::new(resolve_shell(&shell));
    if let Some(dir) = cwd {
        if !dir.is_empty() {
            cmd.cwd(dir);
        }
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let id = {
        let mut n = state.next.lock().unwrap();
        *n += 1;
        *n
    };
    state
        .map
        .lock()
        .unwrap()
        .insert(id, PtyInstance { master: pair.master, writer });

    // pty output -> frontend (per-pty event, base64-framed for xterm.js).
    let app_data = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let evt = format!("pty-data:{id}");
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                    if app_data.emit(&evt, DataPayload { b64 }).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = app_data.emit(&format!("pty-exit:{id}"), ());
    });

    // reap child so it doesn't linger as a zombie
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    // Surface the agent's live progress (todos / tools / token usage) by tailing
    // its session log — emits `agent-progress:{id}` to the UI.
    if let Some(a) = agent {
        agent_watch::start(app.clone(), id, a, cwd_for_watch);
    }

    Ok(id)
}

#[tauri::command]
fn pty_write(state: State<PtyState>, id: u32, data: String) -> Result<(), String> {
    if let Some(inst) = state.map.lock().unwrap().get_mut(&id) {
        inst.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
        let _ = inst.writer.flush();
    }
    Ok(())
}

#[tauri::command]
fn pty_resize(state: State<PtyState>, id: u32, cols: u16, rows: u16) -> Result<(), String> {
    if let Some(inst) = state.map.lock().unwrap().get(&id) {
        inst.master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn pty_kill(state: State<PtyState>, id: u32) {
    state.map.lock().unwrap().remove(&id);
}

#[tauri::command]
fn app_home() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
}

#[tauri::command]
fn app_selftest() -> bool {
    std::env::var("HELM_SELFTEST").map(|v| v == "1").unwrap_or(false)
}

/// Listening TCP ports on this machine (for sidebar port slots).
/// Parses `netstat -ano` output — works on all Windows versions with no extra deps.
#[tauri::command]
fn listening_ports() -> Vec<u16> {
    #[cfg(windows)]
    {
        let out = match std::process::Command::new("netstat").args(["-ano"]).creation_flags(NO_WINDOW).output() {
            Ok(o) => o,
            Err(_) => return vec![],
        };
        if !out.status.success() {
            return vec![];
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut ports: Vec<u16> = text
            .lines()
            .filter(|l| l.contains("LISTENING"))
            .filter_map(|l| {
                let cols: Vec<&str> = l.split_whitespace().collect();
                if cols.len() < 2 { return None; }
                let addr = cols[1];
                let port_str = addr.rsplit(':').next()?;
                port_str.parse::<u16>().ok()
            })
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports.retain(|&p| p >= 1024 && p <= 49151);
        ports
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Current git branch for a directory (for the sidebar row), or None.
#[tauri::command]
fn git_branch(cwd: String) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["-C", &cwd, "rev-parse", "--abbrev-ref", "HEAD"]);
    #[cfg(windows)]
    cmd.creation_flags(NO_WINDOW);
    let out = cmd.output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() && s != "HEAD" {
            return Some(s);
        }
    }
    None
}

#[derive(Clone, Serialize, Default)]
struct SystemStats {
    cpu: f64,
    mem: f64,
}

/// Background-refreshed cache. The system_stats command reads this instantly and
/// never does blocking work on the request path (that would stall the IPC runtime
/// and make terminal typing lag).
#[derive(Default)]
struct StatsCache(Mutex<SystemStats>);

#[tauri::command]
fn system_stats(stats: State<StatsCache>) -> SystemStats {
    stats.0.lock().unwrap().clone()
}

/// Begin tailing an agent's progress for an already-running pty. Called from the
/// frontend when it detects (by typed command or terminal title) that the user
/// launched claude/codex/opencode inside a plain shell session.
#[tauri::command]
fn start_agent_watch(app: AppHandle, id: u32, agent: String, cwd: String) {
    // route this cwd's hook events to this pty, and register the agent's hook so
    // it pushes events to us instantly.
    hook_server::register_session(&app, id, &cwd);
    let port = app
        .try_state::<hook_server::HookHub>()
        .map(|h| *h.port.lock().unwrap())
        .unwrap_or(0);
    if port != 0 {
        if agent == "claude" {
            if let Some(fwd) = hook_server::forwarder_path().to_str() {
                hook_server::register_claude(&cwd, port, fwd);
            }
        } else if agent == "opencode" {
            // opencode auto-loads our plugin from its global plugin/ dir; this just
            // (re)writes it with the current receiver port. (Codex is intentionally
            // left on the watcher — auto-editing its global hooks.json + trust store
            // is too risky.)
            hook_server::register_opencode(port);
        }
    }
    agent_watch::start(app, id, agent, cwd);
}

// CPU/memory are read cross-platform via the `sysinfo` crate in setup()'s poller.

#[derive(Clone, Serialize, Default)]
struct UsageRow {
    label: String,
    /// utilization (0-1 fraction or 0-100; the UI normalizes)
    pct: f64,
    resets_at: String,
}
#[derive(Clone, Serialize, Default)]
struct UsageCard {
    account: String,
    plan: String,
    extra: bool,
    rows: Vec<UsageRow>,
}
#[derive(Default)]
struct UsageCache(Mutex<Vec<UsageCard>>);

#[tauri::command]
fn usage_cards(cache: State<UsageCache>) -> Vec<UsageCard> {
    cache.0.lock().unwrap().clone()
}

/// Dependency-free HTTP/1.1 GET against a localhost service → JSON body.
/// Reads a small JSON response from a local endpoint.
fn http_get_json(port: u16, path: &str) -> Option<Value> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_millis(700))).ok()?;
    s.set_write_timeout(Some(std::time::Duration::from_secs(3))).ok()?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).ok()?;
    // Read until the JSON body parses — don't wait for EOF (a keep-alive peer
    // would otherwise block until the read timeout on every request).
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..40 {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(v) = parse_http_json(&buf) {
                    return Some(v);
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if let Some(v) = parse_http_json(&buf) {
                    return Some(v);
                }
            }
            Err(_) => break,
        }
    }
    parse_http_json(&buf)
}

fn parse_http_json(buf: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(buf);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let body = if head.to_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body)
    } else {
        body.to_string()
    };
    serde_json::from_str::<Value>(body.trim()).ok()
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(nl) = rest.find("\r\n") {
        let size = usize::from_str_radix(rest[..nl].trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = nl + 2;
        if start + size > rest.len() {
            out.push_str(&rest[start..]);
            break;
        }
        out.push_str(&rest[start..start + size]);
        rest = rest[start + size..].strip_prefix("\r\n").unwrap_or(&rest[start + size..]);
    }
    out
}

/// Local port for the optional usage endpoint, from `HELM_USAGE_PORT`. Unset or
/// invalid → the USAGE card is disabled (no port baked in, no integration assumed).
fn usage_port() -> Option<u16> {
    std::env::var("HELM_USAGE_PORT").ok()?.trim().parse().ok()
}

/// Claude usage card from a local usage endpoint. None on unreachable / rate-limit so
/// the caller keeps the last good value.
fn read_usage_cards(port: u16) -> Option<Vec<UsageCard>> {
    let profile = http_get_json(port, "/api/oauth/profile")?;
    let account = profile["account"]["display_name"]
        .as_str()
        .or_else(|| profile["account"]["email"].as_str())
        .or_else(|| profile["account"]["name"].as_str())?
        .to_string();
    let plan = profile["organization"]["organization_type"]
        .as_str()
        .unwrap_or("claude")
        .to_string();
    let extra = profile["organization"]["has_extra_usage_enabled"]
        .as_bool()
        .unwrap_or(false);

    let mut rows = Vec::new();
    if let Some(usage) = http_get_json(port, "/api/oauth/usage") {
        let mk = |key: &str, label: &str| -> Option<UsageRow> {
            let node = &usage[key];
            if !node.is_object() {
                return None;
            }
            Some(UsageRow {
                label: label.into(),
                pct: node["utilization"].as_f64().unwrap_or(0.0),
                resets_at: node["resets_at"].as_str().unwrap_or("").to_string(),
            })
        };
        if let Some(r) = mk("five_hour", "5h") {
            rows.push(r);
        }
        if let Some(r) = mk("seven_day", "7d") {
            rows.push(r);
        }
        // 7d-Sonnet sibling — exact key unconfirmed; render only if it exists
        for k in ["seven_day_sonnet", "seven_day_oauth_apps", "seven_day_opus"] {
            if let Some(r) = mk(k, "7d Sonnet") {
                rows.push(r);
                break;
            }
        }
    }

    Some(vec![UsageCard { account, plan, extra, rows }])
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(PtyState::default())
        .manage(hook_server::HookHub::default())
        .setup(|app| {
            // Native-hook receiver: agents POST lifecycle events here for instant
            // progress/tasks/completion (see hook_server). Falls back silently to
            // the transcript watchers when an agent has no hook registered.
            {
                let port = hook_server::start(app.handle().clone());
                if let Some(hub) = app.try_state::<hook_server::HookHub>() {
                    *hub.port.lock().unwrap() = port;
                }
                hook_server::write_forwarder(port);
            }
            // System-stats cache refreshed off the request path (sysinfo, cross-
            // platform) so the command never does blocking work inside the IPC loop.
            app.manage(StatsCache::default());
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    let mut sys = sysinfo::System::new();
                    loop {
                        sys.refresh_cpu_usage();
                        sys.refresh_memory();
                        let total = sys.total_memory();
                        let s = SystemStats {
                            cpu: sys.global_cpu_usage() as f64,
                            mem: if total > 0 { (sys.used_memory() as f64 / total as f64) * 100.0 } else { 0.0 },
                        };
                        if let Some(c) = h.try_state::<StatsCache>() {
                            *c.0.lock().unwrap() = s;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2000));
                    }
                });
            }

            // Account usage for the USAGE card, polled from an optional local
            // endpoint (set HELM_USAGE_PORT to enable). Polled at 60s — the
            // upstream rate-limits hard; on failure we keep the last good value.
            app.manage(UsageCache::default());
            if let Some(port) = usage_port() {
                let h = app.handle().clone();
                std::thread::spawn(move || loop {
                    if let Some(cards) = read_usage_cards(port) {
                        if let Some(c) = h.try_state::<UsageCache>() {
                            *c.0.lock().unwrap() = cards;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_secs(60));
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            app_home,
            app_selftest,
            git_branch,
            listening_ports,
            system_stats,
            start_agent_watch,
            usage_cards
        ])
        .run(tauri::generate_context!())
        .expect("error while running helm");
}
