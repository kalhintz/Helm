// Native agent hooks -> instant push.
//
// Each agent (Claude Code / Codex / opencode) is given a hook (or plugin) that
// POSTs its lifecycle events to this tiny localhost HTTP receiver the instant they
// fire — tool start/finish, task changes, turn completion. That makes progress /
// tasks / "done" update with zero log polling. The transcript watchers stay only
// for token/context + conversation history; once hooks are live for a session the
// watcher stops emitting status/activity/todos (it would only fight the hooks).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct HookHub {
    pub port: Mutex<u16>,
    cwd_to_pty: Mutex<HashMap<String, u32>>, // normalized cwd -> pty id
    active: Mutex<HashSet<u32>>,             // ptys receiving live hook events
}

fn norm(cwd: &str) -> String {
    cwd.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Bind to the chosen pty so hook events (which only carry cwd) can be routed.
pub fn register_session(app: &AppHandle, pty_id: u32, cwd: &str) {
    if let Some(hub) = app.try_state::<HookHub>() {
        hub.cwd_to_pty.lock().unwrap().insert(norm(cwd), pty_id);
    }
}

/// True once hooks have delivered at least one event for this pty — the watcher
/// then yields status/activity/todos to the (instant) hook stream.
pub fn hooks_active(app: &AppHandle, pty_id: u32) -> bool {
    app.try_state::<HookHub>()
        .map_or(false, |h| h.active.lock().unwrap().contains(&pty_id))
}

pub fn start(app: AppHandle) -> u16 {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = handle(&app, stream);
            });
        }
    });
    port
}

fn handle(app: &AppHandle, mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?; // request line (ignored)
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        let t = h.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some(v) = t.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    if let Ok(v) = serde_json::from_slice::<Value>(&body) {
        process(app, &v);
    }
    Ok(())
}

fn lookup(app: &AppHandle, cwd: &str) -> Option<u32> {
    let hub = app.try_state::<HookHub>()?;
    let pty = hub.cwd_to_pty.lock().unwrap().get(&norm(cwd)).copied();
    pty
}

fn mark_active(app: &AppHandle, pty: u32) {
    if let Some(hub) = app.try_state::<HookHub>() {
        hub.active.lock().unwrap().insert(pty);
    }
}

fn emit(app: &AppHandle, pty: u32, payload: Value) {
    let _ = app.emit(&format!("agent-progress:{pty}"), payload);
}

fn process(app: &AppHandle, v: &Value) {
    // opencode plugin events are {type, properties, directory}; Claude/Codex hooks
    // carry hook_event_name + cwd.
    if v.get("type").and_then(|t| t.as_str()).is_some() && v.get("hook_event_name").is_none() {
        process_opencode(app, v);
        return;
    }
    let ev = v["hook_event_name"]
        .as_str()
        .or_else(|| v["hookEventName"].as_str())
        .unwrap_or("");
    let cwd = v["cwd"].as_str().unwrap_or("");
    let pty = match lookup(app, cwd) {
        Some(p) => p,
        None => return,
    };
    mark_active(app, pty);
    match ev {
        "UserPromptSubmit" => emit(app, pty, json!({ "status": "working", "activity": "" })),
        "PreToolUse" => {
            let tool = v["tool_name"].as_str().unwrap_or("tool");
            let summ = summarize(&v["tool_input"]);
            let activity = if summ.is_empty() {
                tool.to_string()
            } else {
                format!("{tool} · {summ}")
            };
            emit(app, pty, json!({ "status": "working", "activity": activity }));
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let tool = v["tool_name"].as_str().unwrap_or("");
            if tool == "TaskCreate" || tool == "TaskUpdate" || tool == "TodoWrite" {
                if let Some(todos) = read_claude_tasks(v["session_id"].as_str().unwrap_or("")) {
                    emit(app, pty, json!({ "todos": todos }));
                }
            }
        }
        "Notification" => emit(app, pty, json!({ "status": "waiting" })),
        "Stop" => emit(app, pty, json!({ "status": "idle" })),
        _ => {}
    }
}

fn process_opencode(app: &AppHandle, v: &Value) {
    let dir = v["directory"].as_str().unwrap_or("");
    let pty = match lookup(app, dir) {
        Some(p) => p,
        None => return,
    };
    mark_active(app, pty);
    let p = &v["properties"];
    match v["type"].as_str().unwrap_or("") {
        "session.idle" => emit(app, pty, json!({ "status": "idle" })),
        "session.status" => {
            let st = p["status"]["type"].as_str().unwrap_or("");
            let mapped = if st == "busy" { "working" } else { "idle" };
            emit(app, pty, json!({ "status": mapped }));
        }
        "todo.updated" => {
            if let Some(arr) = p["todos"].as_array() {
                let todos: Vec<Value> = arr
                    .iter()
                    .map(|t| {
                        json!({
                            "text": t["content"].as_str().unwrap_or(""),
                            "status": t["status"].as_str().unwrap_or("pending"),
                        })
                    })
                    .collect();
                emit(app, pty, json!({ "todos": todos }));
            }
        }
        "message.part.updated" => {
            if let Some(txt) = p["part"]["text"].as_str() {
                if !txt.trim().is_empty() {
                    let a: String = txt.trim().chars().take(90).collect();
                    emit(app, pty, json!({ "status": "working", "activity": a }));
                }
            }
        }
        _ => {}
    }
}

/// Claude's authoritative task snapshot lives at ~/.claude/tasks/<sid>/<n>.json.
fn read_claude_tasks(sid: &str) -> Option<Vec<Value>> {
    if sid.is_empty() {
        return None;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let dir = std::path::Path::new(&home).join(".claude").join("tasks").join(sid);
    let mut items: Vec<(u64, Value)> = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem: u64 = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(u64::MAX);
        if let Ok(txt) = std::fs::read_to_string(&path) {
            if let Ok(t) = serde_json::from_str::<Value>(txt.trim_start_matches('\u{feff}')) {
                items.push((
                    stem,
                    json!({
                        "text": t["subject"].as_str().or_else(|| t["activeForm"].as_str()).unwrap_or(""),
                        "status": t["status"].as_str().unwrap_or("pending"),
                    }),
                ));
            }
        }
    }
    if items.is_empty() {
        return None;
    }
    items.sort_by_key(|(n, _)| *n);
    Some(items.into_iter().map(|(_, v)| v).collect())
}

fn helm_dir() -> std::path::PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    std::path::Path::new(&home).join(".helm")
}

pub fn forwarder_path() -> std::path::PathBuf {
    helm_dir().join(if cfg!(windows) {
        "helm-hook.cmd"
    } else {
        "helm-hook.sh"
    })
}

/// Write the tiny stdin->POST forwarder with the receiver port baked in. It always
/// exits 0 with the silent pass-through JSON so a hook never disturbs the agent.
pub fn write_forwarder(port: u16) {
    let _ = std::fs::create_dir_all(helm_dir());
    let path = forwarder_path();
    let body = if cfg!(windows) {
        format!(
            "@echo off\r\ncurl -s -m 2 -X POST \"http://127.0.0.1:{port}/hook\" -H \"content-type: application/json\" --data-binary @- >NUL 2>&1\r\necho {{\"continue\":true,\"suppressOutput\":true}}\r\nexit /b 0\r\n"
        )
    } else {
        format!(
            "#!/bin/sh\ncurl -s -m 2 -X POST \"http://127.0.0.1:{port}/hook\" -H \"content-type: application/json\" --data-binary @- >/dev/null 2>&1\necho '{{\"continue\":true,\"suppressOutput\":true}}'\nexit 0\n"
        )
    };
    let _ = std::fs::write(&path, body);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
}

fn summarize(input: &Value) -> String {
    for k in ["description", "command", "file_path", "path", "pattern", "subject"] {
        if let Some(s) = input[k].as_str() {
            if !s.is_empty() {
                return s.replace(['\n', '\r'], " ").chars().take(48).collect();
            }
        }
    }
    String::new()
}

/// Write the localhost forwarder + register Claude hooks for a project cwd. The
/// forwarder pipes each hook's stdin JSON to our receiver and always exits 0 so it
/// never disturbs the agent. We merge into the project's `.claude/settings.local.json`
/// (local-only) and never touch the user's global config.
pub fn register_claude(cwd: &str, _port: u16, forwarder: &str) {
    let settings_dir = std::path::Path::new(cwd).join(".claude");
    // NEVER touch the user's global ~/.claude — only genuinely project-local dirs.
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let global = std::path::Path::new(&home).join(".claude");
    if norm(&settings_dir.to_string_lossy()) == norm(&global.to_string_lossy()) {
        return;
    }
    if std::fs::create_dir_all(&settings_dir).is_err() {
        return;
    }
    let cmd = json!({ "type": "command", "command": format!("\"{forwarder}\""), "timeout": 5 });
    let group = json!({ "matcher": "*", "hooks": [cmd] });
    let events = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "Stop",
        "Notification",
        "SessionEnd",
    ];

    let path = settings_dir.join("settings.local.json");
    let mut root: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .filter(|v: &Value| v.is_object())
        .unwrap_or_else(|| json!({}));
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if let Some(h) = hooks.as_object_mut() {
        for ev in events {
            let arr = h.entry(ev).or_insert_with(|| json!([]));
            if let Some(list) = arr.as_array_mut() {
                // drop any prior Helm group, then append ours (idempotent; never
                // clobbers other hooks the user/OMC registered on this event).
                list.retain(|g| !group_mentions(g, forwarder));
                list.push(group.clone());
            } else {
                *arr = json!([group]);
            }
        }
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap_or_default());
}

fn group_mentions(group: &Value, forwarder: &str) -> bool {
    group["hooks"]
        .as_array()
        .map(|hs| {
            hs.iter().any(|h| {
                h["command"]
                    .as_str()
                    .map_or(false, |c| c.contains(forwarder))
            })
        })
        .unwrap_or(false)
}
