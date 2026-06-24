// Agent progress watchers.
//
// Each AI coding CLI (claude / codex / opencode) runs inside one of our ConPTY
// terminals. Rather than scrape the terminal, we read the agent's own structured
// session log and normalize it into one `AgentProgress` shape, emitted to the
// frontend as `agent-progress:{pty_id}`. The frontend renders todos / tool
// timeline / token-usage from this single schema regardless of which agent it is.
//
// Keeping the parsing here (backend) means a future remote/mobile client can be
// fed the very same normalized stream.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize, Default)]
pub struct AgentProgress {
    /// working | waiting | done | idle | error
    pub status: String,
    pub activity: String,
    pub todos: Vec<TodoItem>,
    pub tools: Vec<ToolEvent>,
    pub context: Option<Context>,
}

#[derive(Clone, Serialize)]
pub struct TodoItem {
    pub text: String,
    /// pending | in_progress | completed
    pub status: String,
}

#[derive(Clone, Serialize)]
pub struct ToolEvent {
    pub ts: String,
    pub name: String,
    pub summary: String,
}

#[derive(Clone, Serialize)]
pub struct Context {
    pub used: u64,
    pub max: u64,
    pub pct: u8,
}

// ---- conversation view (center "대화" pane) ----
// Emitted separately from AgentProgress; the frontend renders these only when
// the user toggles a session to the conversation view. Claude/Codex carry full
// message bodies; opencode is degraded (no bodies on disk).
#[derive(Clone, Serialize)]
pub struct ConvToolCall {
    pub id: String,
    pub name: String,
    pub summary: String,
    /// running | completed | error
    pub status: String,
    pub result: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct ConvUsage {
    pub used: u64,
    pub max: u64,
}
#[derive(Clone, Serialize)]
pub struct ConvMessage {
    pub id: String,
    /// user | assistant | system
    pub role: String,
    pub ts: String,
    pub text: String,
    pub thinking: Option<String>,
    pub tool_calls: Vec<ConvToolCall>,
    pub usage: Option<ConvUsage>,
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

fn tool_result_text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(arr) = v.as_array() {
        let mut out = String::new();
        for it in arr {
            if let Some(t) = it["text"].as_str() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        return out;
    }
    String::new()
}

/// Parse one Claude transcript line into conversation events.
fn claude_conv_from_line(app: &AppHandle, pty_id: u32, v: &Value, pending: &mut HashMap<String, String>) {
    let typ = v["type"].as_str().unwrap_or("");
    let ts = v["timestamp"].as_str().unwrap_or("").to_string();
    let msg = &v["message"];

    if typ == "assistant" {
        let id = msg["id"].as_str().unwrap_or("").to_string();
        let mut text = String::new();
        let mut thinking: Option<String> = None;
        let mut tool_calls = Vec::new();
        if let Some(arr) = msg["content"].as_array() {
            for item in arr {
                match item["type"].as_str().unwrap_or("") {
                    "text" => {
                        if let Some(t) = item["text"].as_str() {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                    "thinking" => {
                        if let Some(t) = item["thinking"].as_str() {
                            thinking = Some(t.to_string());
                        }
                    }
                    "tool_use" => {
                        let tid = item["id"].as_str().unwrap_or("").to_string();
                        let name = item["name"].as_str().unwrap_or("tool").to_string();
                        if !tid.is_empty() {
                            pending.insert(tid.clone(), name.clone());
                        }
                        tool_calls.push(ConvToolCall {
                            id: tid,
                            name,
                            summary: summarize_input(&item["input"]),
                            status: "running".into(),
                            result: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        let usage = {
            let u = &msg["usage"];
            if u.is_object() {
                let g = |k: &str| u[k].as_u64().unwrap_or(0);
                let used = g("input_tokens")
                    + g("cache_read_input_tokens")
                    + g("cache_creation_input_tokens")
                    + g("output_tokens");
                let max = if used > 190_000 { 1_000_000 } else { 200_000 };
                Some(ConvUsage { used, max })
            } else {
                None
            }
        };
        if text.is_empty() && thinking.is_none() && tool_calls.is_empty() {
            return;
        }
        let m = ConvMessage {
            id: if id.is_empty() { ts.clone() } else { id },
            role: "assistant".into(),
            ts,
            text,
            thinking,
            tool_calls,
            usage,
        };
        let _ = app.emit(&format!("conv-msg:{pty_id}"), &m);
    } else if typ == "user" {
        let content = &msg["content"];
        if let Some(arr) = content.as_array() {
            let mut had_result = false;
            for item in arr {
                if item["type"].as_str() == Some("tool_result") {
                    had_result = true;
                    let tid = item["tool_use_id"].as_str().unwrap_or("").to_string();
                    let is_err = item["is_error"].as_bool().unwrap_or(false);
                    let name = pending.remove(&tid).unwrap_or_default();
                    let result = truncate_str(&tool_result_text(&item["content"]), 4000);
                    let tc = ConvToolCall {
                        id: tid,
                        name,
                        summary: String::new(),
                        status: if is_err { "error".into() } else { "completed".into() },
                        result: Some(result),
                    };
                    let _ = app.emit(&format!("conv-tool:{pty_id}"), &tc);
                }
            }
            if had_result {
                return;
            }
        }
        let text = content.as_str().unwrap_or("").to_string();
        if text.trim().is_empty() {
            return;
        }
        let m = ConvMessage {
            id: ts.clone(),
            role: "user".into(),
            ts,
            text,
            thinking: None,
            tool_calls: vec![],
            usage: None,
        };
        let _ = app.emit(&format!("conv-msg:{pty_id}"), &m);
    }
}

/// Spawn the right watcher for `agent`, bound to `pty_id` + its `cwd`.
/// No-op for plain shells (claude/codex/opencode only).
pub fn start(app: AppHandle, pty_id: u32, agent: String, cwd: String) {
    match agent.as_str() {
        "claude" => {
            std::thread::spawn(move || claude_watch(app, pty_id, cwd));
        }
        "codex" => {
            std::thread::spawn(move || codex_watch(app, pty_id, cwd));
        }
        "opencode" => {
            std::thread::spawn(move || opencode_watch(app, pty_id, cwd));
        }
        _ => {}
    }
}

/// Claude Code stores one JSONL transcript per session at
/// `%USERPROFILE%/.claude/projects/<slug>/<uuid>.jsonl`, where <slug> is the cwd
/// with every non-alphanumeric character replaced by '-'.
fn slug_for_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
}

/// Newest *.jsonl in `dir` whose mtime is at/after the watch start (so we bind to
/// the transcript of the run we just launched, not a stale prior session).
fn newest_jsonl(dir: &Path, after: SystemTime) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(m) = meta.modified() else { continue };
        // skip files that went stale before we started (allow 5s skew)
        if m + Duration::from_secs(5) < after {
            continue;
        }
        if best.as_ref().map_or(true, |(bm, _)| m > *bm) {
            best = Some((m, p));
        }
    }
    best.map(|(_, p)| p)
}

fn claude_watch(app: AppHandle, pty_id: u32, cwd: String) {
    let Some(home) = home_dir() else { return };
    let dir = home.join(".claude").join("projects").join(slug_for_cwd(&cwd));
    let evt = format!("agent-progress:{pty_id}");
    let start = SystemTime::now();

    let mut cur_path: Option<PathBuf> = None;
    let mut offset: u64 = 0;
    let mut state = ClaudeState::default();
    let mut last_sig = String::new();
    let mut pending: HashMap<String, String> = HashMap::new();

    loop {
        std::thread::sleep(Duration::from_millis(700));
        if !dir.exists() {
            continue;
        }

        // (re)select the active transcript; on switch, restart from byte 0.
        if let Some(newest) = newest_jsonl(&dir, start) {
            if cur_path.as_ref() != Some(&newest) {
                cur_path = Some(newest);
                offset = 0;
                state = ClaudeState::default();
                pending.clear();
                let _ = app.emit(&format!("conv-reset:{pty_id}"), ());
            }
        }
        let Some(path) = cur_path.clone() else { continue };

        let Ok(mut f) = File::open(&path) else { continue };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < offset {
            // file truncated / rotated — start over
            offset = 0;
            state = ClaudeState::default();
        }
        if len > offset {
            if f.seek(SeekFrom::Start(offset)).is_ok() {
                let mut buf = String::new();
                if f.read_to_string(&mut buf).is_ok() {
                    if let Some(idx) = buf.rfind('\n') {
                        let complete = &buf[..=idx];
                        for line in complete.split('\n') {
                            let t = line.trim();
                            if t.is_empty() {
                                continue;
                            }
                            if let Ok(v) = serde_json::from_str::<Value>(t) {
                                state.ingest(&v);
                                claude_conv_from_line(&app, pty_id, &v, &mut pending);
                            }
                        }
                        offset += complete.as_bytes().len() as u64;
                    }
                }
            }
        }

        // "working" while the transcript is actively growing; otherwise the last
        // turn boundary decides idle vs still-mid-loop.
        let idle = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|d| d.as_secs() >= 3)
            .unwrap_or(true);

        let prog = state.to_progress(idle);
        let sig = state.signature(&prog);
        if sig != last_sig {
            last_sig = sig;
            let _ = app.emit(&evt, &prog);
        }
    }
}

#[derive(Default)]
struct ClaudeState {
    todos: Vec<TodoItem>,
    tools: Vec<ToolEvent>,
    last_text: String,
    last_tool: String,
    last_tool_summary: String,
    last_stop: String,
    used_tokens: u64,
    // OMC-style task harness (TaskCreate/TaskUpdate) — an alternative to TodoWrite.
    // The id is only known from the TaskCreate tool_result ("Task #<id> created"),
    // so we hold the subject by tool_use_id until that result arrives.
    omc_pending: HashMap<String, String>,
    omc_tasks: Vec<(String, String, String)>, // (id, text, status)
}

impl ClaudeState {
    fn ingest(&mut self, v: &Value) {
        let typ = v["type"].as_str().unwrap_or("");
        if typ == "user" {
            // A TaskCreate's assigned id only appears in its tool_result
            // ("Task #<id> created…"), so resolve pending subjects here.
            if let Some(arr) = v["message"]["content"].as_array() {
                for item in arr {
                    if item["type"].as_str() != Some("tool_result") {
                        continue;
                    }
                    let tuid = item["tool_use_id"].as_str().unwrap_or("");
                    if let Some(subject) = self.omc_pending.get(tuid).cloned() {
                        if let Some(id) = parse_task_id(&tool_result_text(&item["content"])) {
                            self.omc_pending.remove(tuid);
                            self.omc_tasks.push((id, subject, "pending".into()));
                        }
                    }
                }
            }
            return;
        }
        if typ != "assistant" {
            return;
        }
        let msg = &v["message"];
        if let Some(sr) = msg["stop_reason"].as_str() {
            if !sr.is_empty() {
                self.last_stop = sr.to_string();
            }
        }

        let u = &msg["usage"];
        if u.is_object() {
            let g = |k: &str| u[k].as_u64().unwrap_or(0);
            let used = g("input_tokens")
                + g("cache_read_input_tokens")
                + g("cache_creation_input_tokens")
                + g("output_tokens");
            if used > 0 {
                self.used_tokens = used;
            }
        }

        let ts = v["timestamp"].as_str().unwrap_or("").to_string();
        if let Some(arr) = msg["content"].as_array() {
            for item in arr {
                match item["type"].as_str().unwrap_or("") {
                    "text" => {
                        if let Some(t) = item["text"].as_str() {
                            if !t.trim().is_empty() {
                                self.last_text = t.trim().to_string();
                            }
                        }
                    }
                    "tool_use" => {
                        let name = item["name"].as_str().unwrap_or("tool").to_string();
                        if name == "TodoWrite" {
                            if let Some(todos) = item["input"]["todos"].as_array() {
                                self.todos = todos
                                    .iter()
                                    .map(|t| TodoItem {
                                        text: t["content"]
                                            .as_str()
                                            .or_else(|| t["text"].as_str())
                                            .or_else(|| t["activeForm"].as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        status: t["status"].as_str().unwrap_or("pending").to_string(),
                                    })
                                    .collect();
                            }
                        } else if name == "TaskCreate" {
                            let subject = item["input"]["subject"]
                                .as_str()
                                .or_else(|| item["input"]["activeForm"].as_str())
                                .or_else(|| item["input"]["description"].as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some(tuid) = item["id"].as_str() {
                                if !subject.is_empty() {
                                    self.omc_pending.insert(tuid.to_string(), subject);
                                }
                            }
                        } else if name == "TaskUpdate" {
                            let id = item["input"]["taskId"]
                                .as_str()
                                .map(|s| s.to_string())
                                .or_else(|| item["input"]["taskId"].as_u64().map(|n| n.to_string()));
                            if let Some(id) = id {
                                let status = item["input"]["status"].as_str().unwrap_or("");
                                if status == "deleted" {
                                    self.omc_tasks.retain(|(tid, _, _)| tid != &id);
                                } else {
                                    for t in self.omc_tasks.iter_mut() {
                                        if t.0 == id {
                                            if !status.is_empty() {
                                                t.2 = status.to_string();
                                            }
                                            if let Some(subj) = item["input"]["subject"].as_str() {
                                                t.1 = subj.to_string();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let summary = summarize_input(&item["input"]);
                        self.last_tool = name.clone();
                        self.last_tool_summary = summary.clone();
                        self.tools.push(ToolEvent { ts: ts.clone(), name, summary });
                        if self.tools.len() > 100 {
                            let cut = self.tools.len() - 100;
                            self.tools.drain(0..cut);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn to_progress(&self, idle: bool) -> AgentProgress {
        let status = if !idle {
            "working"
        } else if self.last_stop == "tool_use" {
            "working"
        } else if self.last_stop == "end_turn" {
            "idle"
        } else {
            "idle"
        };

        let activity = if status == "working" && !self.last_tool.is_empty() {
            if self.last_tool_summary.is_empty() {
                self.last_tool.clone()
            } else {
                format!("{} · {}", self.last_tool, self.last_tool_summary)
            }
        } else if !self.last_text.is_empty() {
            self.last_text.chars().take(90).collect()
        } else {
            String::new()
        };

        let context = if self.used_tokens > 0 {
            // transcript doesn't expose the [1m] flag, so infer: anything past the
            // 200k standard window must be a 1M-context session.
            let max = if self.used_tokens > 190_000 { 1_000_000 } else { 200_000 };
            let pct = ((self.used_tokens as f64 / max as f64) * 100.0).min(100.0) as u8;
            Some(Context { used: self.used_tokens, max, pct })
        } else {
            None
        };

        let tools = self.tools.iter().rev().take(20).rev().cloned().collect();
        // Prefer the OMC task harness when present, otherwise the TodoWrite list.
        let todos = if !self.omc_tasks.is_empty() {
            self.omc_tasks
                .iter()
                .map(|(_, text, status)| TodoItem { text: text.clone(), status: status.clone() })
                .collect()
        } else {
            self.todos.clone()
        };

        AgentProgress {
            status: status.into(),
            activity,
            todos,
            tools,
            context,
        }
    }

    fn signature(&self, p: &AgentProgress) -> String {
        let done = p.todos.iter().filter(|t| t.status == "completed").count();
        let last_ts = self.tools.last().map(|t| t.ts.as_str()).unwrap_or("");
        let todo_sig: String = p.todos.iter().map(|t| t.status.chars().next().unwrap_or('?')).collect();
        format!(
            "{}|{}|{}/{}|{}|{}|{}|{}",
            p.status,
            p.activity,
            done,
            p.todos.len(),
            self.tools.len(),
            last_ts,
            self.used_tokens,
            todo_sig
        )
    }
}

/// Pull the numeric id out of a TaskCreate result like "Task #26 created…".
fn parse_task_id(text: &str) -> Option<String> {
    let idx = text.find("Task #")?;
    let digits: String = text[idx + "Task #".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

fn summarize_input(input: &Value) -> String {
    for k in [
        "description",
        "command",
        "file_path",
        "path",
        "pattern",
        "prompt",
        "query",
        "url",
        "old_string",
    ] {
        if let Some(s) = input[k].as_str() {
            let s = s.trim().replace(['\n', '\r'], " ");
            if !s.is_empty() {
                return s.chars().take(60).collect();
            }
        }
    }
    String::new()
}

// ===== shared path matching =====

fn norm_path(s: &str) -> String {
    s.replace('\\', "/").trim_end_matches('/').to_lowercase()
}
fn basename(s: &str) -> String {
    norm_path(s).rsplit('/').next().unwrap_or("").to_string()
}
/// Loose match between a session's reported directory and our launch cwd — path
/// shapes differ across agents (drive letters, slashes), so accept suffix or
/// basename equality.
fn path_matches(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (na, nb) = (norm_path(a), norm_path(b));
    na == nb || na.ends_with(&nb) || nb.ends_with(&na) || (!basename(a).is_empty() && basename(a) == basename(b))
}

// ===== Codex CLI =====
//
// Codex writes a rollout JSONL per session at
// %USERPROFILE%/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl. No reliable
// token usage or structured todos, so we surface status + activity + a
// tool/command timeline. Bound to a session by session_meta.payload.cwd.

fn first_lines(path: &Path, max: usize) -> Vec<Value> {
    use std::io::BufRead;
    let mut out = Vec::new();
    if let Ok(f) = File::open(path) {
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok).take(max) {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(t) {
                out.push(v);
            }
        }
    }
    out
}

fn newest_codex_rollout(root: &Path, after: SystemTime, cwd: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u8) {
        if depth > 4 {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out, depth + 1);
                } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |n| n.starts_with("rollout-"))
                {
                    out.push(p);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files, 0);
    let mut candidates: Vec<(SystemTime, PathBuf)> = files
        .into_iter()
        .filter_map(|p| {
            let m = std::fs::metadata(&p).and_then(|md| md.modified()).ok()?;
            if m + Duration::from_secs(5) < after {
                None
            } else {
                Some((m, p))
            }
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in candidates {
        let matched = first_lines(&p, 6).iter().any(|v| {
            v["type"].as_str() == Some("session_meta")
                && path_matches(v["payload"]["cwd"].as_str().unwrap_or(""), cwd)
        });
        if matched {
            return Some(p);
        }
    }
    None
}

fn codex_collect_text(p: &Value) -> String {
    let mut out = String::new();
    if let Some(arr) = p["content"].as_array() {
        for it in arr {
            if let Some(t) = it["text"].as_str() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
    } else if let Some(s) = p["text"].as_str() {
        out.push_str(s);
    }
    if out.is_empty() {
        if let Some(arr) = p["summary"].as_array() {
            for it in arr {
                if let Some(t) = it["text"].as_str() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
        }
    }
    out
}

/// Parse one Codex rollout line into conversation events.
fn codex_conv_from_line(app: &AppHandle, pty_id: u32, v: &Value, pending: &mut HashMap<String, String>) {
    if v["type"].as_str() != Some("response_item") {
        return;
    }
    let p = &v["payload"];
    let ts = v["timestamp"].as_str().unwrap_or("").to_string();
    match p["type"].as_str().unwrap_or("") {
        "function_call" => {
            let cid = p["call_id"].as_str().or_else(|| p["id"].as_str()).unwrap_or("").to_string();
            let name = p["name"].as_str().unwrap_or("call").to_string();
            if !cid.is_empty() {
                pending.insert(cid.clone(), name.clone());
            }
            let summary = p["arguments"]
                .as_str()
                .map(|s| s.replace(['\n', '\r'], " ").chars().take(80).collect::<String>())
                .unwrap_or_default();
            let m = ConvMessage {
                id: format!("{ts}-{cid}"),
                role: "assistant".into(),
                ts,
                text: String::new(),
                thinking: None,
                tool_calls: vec![ConvToolCall { id: cid, name, summary, status: "running".into(), result: None }],
                usage: None,
            };
            let _ = app.emit(&format!("conv-msg:{pty_id}"), &m);
        }
        "function_call_output" | "execution_result" => {
            let cid = p["call_id"].as_str().or_else(|| p["id"].as_str()).unwrap_or("").to_string();
            let name = pending.remove(&cid).unwrap_or_default();
            let out = p["output"].as_str().or_else(|| p["result"].as_str()).unwrap_or("");
            let tc = ConvToolCall {
                id: cid,
                name,
                summary: String::new(),
                status: "completed".into(),
                result: Some(truncate_str(out, 4000)),
            };
            let _ = app.emit(&format!("conv-tool:{pty_id}"), &tc);
        }
        "reasoning" => {
            let t = codex_collect_text(p);
            if !t.trim().is_empty() {
                let m = ConvMessage {
                    id: format!("{ts}-r"),
                    role: "assistant".into(),
                    ts,
                    text: String::new(),
                    thinking: Some(t),
                    tool_calls: vec![],
                    usage: None,
                };
                let _ = app.emit(&format!("conv-msg:{pty_id}"), &m);
            }
        }
        _ => {
            let role = p["role"].as_str().unwrap_or("");
            if role == "assistant" || role == "user" {
                let t = codex_collect_text(p);
                if !t.trim().is_empty() {
                    let m = ConvMessage {
                        id: format!("{ts}-{role}"),
                        role: role.into(),
                        ts,
                        text: t,
                        thinking: None,
                        tool_calls: vec![],
                        usage: None,
                    };
                    let _ = app.emit(&format!("conv-msg:{pty_id}"), &m);
                }
            }
        }
    }
}

fn codex_watch(app: AppHandle, pty_id: u32, cwd: String) {
    let Some(home) = home_dir() else { return };
    let root = home.join(".codex").join("sessions");
    let evt = format!("agent-progress:{pty_id}");
    let start = SystemTime::now();

    let mut cur: Option<PathBuf> = None;
    let mut offset: u64 = 0;
    let mut state = CodexState::default();
    let mut last_sig = String::new();
    let mut pending: HashMap<String, String> = HashMap::new();

    loop {
        std::thread::sleep(Duration::from_millis(800));
        if !root.exists() {
            continue;
        }
        if let Some(p) = newest_codex_rollout(&root, start, &cwd) {
            if cur.as_ref() != Some(&p) {
                cur = Some(p);
                offset = 0;
                state = CodexState::default();
                pending.clear();
                let _ = app.emit(&format!("conv-reset:{pty_id}"), ());
            }
        }
        let Some(path) = cur.clone() else { continue };
        let Ok(mut f) = File::open(&path) else { continue };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < offset {
            offset = 0;
            state = CodexState::default();
        }
        if len > offset && f.seek(SeekFrom::Start(offset)).is_ok() {
            let mut buf = String::new();
            if f.read_to_string(&mut buf).is_ok() {
                if let Some(idx) = buf.rfind('\n') {
                    let complete = &buf[..=idx];
                    for line in complete.split('\n') {
                        let t = line.trim();
                        if t.is_empty() {
                            continue;
                        }
                        if let Ok(v) = serde_json::from_str::<Value>(t) {
                            state.ingest(&v);
                            codex_conv_from_line(&app, pty_id, &v, &mut pending);
                        }
                    }
                    offset += complete.as_bytes().len() as u64;
                }
            }
        }
        // Only a safety net for abandoned turns — task_complete is the primary
        // signal that flips status to idle, so this can be generous and not trip
        // during normal multi-second reasoning gaps within a turn.
        let idle = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|d| d.as_secs() >= 45)
            .unwrap_or(true);
        let prog = state.to_progress(idle);
        let sig = state.signature(&prog);
        if sig != last_sig {
            last_sig = sig;
            let _ = app.emit(&evt, &prog);
        }
    }
}

#[derive(Default)]
struct CodexState {
    last_text: String,
    last_turn: String,
    tools: Vec<ToolEvent>,
    used_tokens: u64,
    todos: Vec<TodoItem>,
}
impl CodexState {
    fn ingest(&mut self, v: &Value) {
        let ts = v["timestamp"].as_str().unwrap_or("").to_string();
        match v["type"].as_str().unwrap_or("") {
            "event_msg" => {
                let pt = v["payload"]["type"].as_str().unwrap_or("");
                // Codex marks turn boundaries with task_started / task_complete.
                if pt == "task_started" || pt == "task_complete" {
                    self.last_turn = pt.to_string();
                } else if pt == "token_count" {
                    let info = &v["payload"]["info"];
                    let total = info["total_token_usage"]["total_tokens"]
                        .as_u64()
                        .or_else(|| info["total_tokens"].as_u64())
                        .or_else(|| v["payload"]["total_tokens"].as_u64());
                    if let Some(t) = total {
                        if t > 0 {
                            self.used_tokens = t;
                        }
                    }
                } else if pt == "agent_message" {
                    // Codex narrates what it's doing here — use it as live activity.
                    if let Some(m) = v["payload"]["message"].as_str() {
                        if !m.trim().is_empty() {
                            self.last_text = m.trim().to_string();
                        }
                    }
                }
            }
            "response_item" => {
                let p = &v["payload"];
                match p["type"].as_str().unwrap_or("") {
                    "function_call" => {
                        let name = p["name"].as_str().unwrap_or("call").to_string();
                        // Codex's plan tool mirrors Claude's todo list: plan[{step,status}].
                        if name == "update_plan" {
                            if let Some(plan) = p["arguments"]
                                .as_str()
                                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                                .and_then(|av| av["plan"].as_array().cloned())
                            {
                                self.todos = plan
                                    .iter()
                                    .map(|s| TodoItem {
                                        text: s["step"].as_str().unwrap_or("").to_string(),
                                        status: s["status"].as_str().unwrap_or("pending").to_string(),
                                    })
                                    .collect();
                            }
                        }
                        let summary = p["arguments"]
                            .as_str()
                            .map(|s| s.replace(['\n', '\r'], " ").chars().take(60).collect::<String>())
                            .unwrap_or_default();
                        self.push_tool(ts, name, summary);
                    }
                    "function_call_output" | "execution_result" => {
                        let out = p["output"].as_str().or_else(|| p["result"].as_str()).unwrap_or("");
                        let summary = out.lines().next().unwrap_or("").chars().take(60).collect::<String>();
                        self.push_tool(ts, "output".into(), summary);
                    }
                    _ => {
                        if p["role"].as_str() == Some("assistant") {
                            if let Some(arr) = p["content"].as_array() {
                                for it in arr {
                                    if let Some(tx) = it["text"].as_str() {
                                        if !tx.trim().is_empty() {
                                            self.last_text = tx.trim().to_string();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn push_tool(&mut self, ts: String, name: String, summary: String) {
        self.tools.push(ToolEvent { ts, name, summary });
        if self.tools.len() > 100 {
            let cut = self.tools.len() - 100;
            self.tools.drain(0..cut);
        }
    }
    fn to_progress(&self, idle: bool) -> AgentProgress {
        let status = if self.last_turn == "task_started" && !idle {
            "working"
        } else {
            "idle"
        };
        let activity = if !self.last_text.is_empty() {
            self.last_text.chars().take(90).collect()
        } else {
            String::new()
        };
        let context = if self.used_tokens > 0 {
            let max = if self.used_tokens > 190_000 { 1_000_000 } else { 200_000 };
            let pct = ((self.used_tokens as f64 / max as f64) * 100.0).min(100.0) as u8;
            Some(Context { used: self.used_tokens, max, pct })
        } else {
            None
        };
        let tools = self.tools.iter().rev().take(20).rev().cloned().collect();
        AgentProgress { status: status.into(), activity, todos: self.todos.clone(), tools, context }
    }
    fn signature(&self, p: &AgentProgress) -> String {
        let last_ts = self.tools.last().map(|t| t.ts.as_str()).unwrap_or("");
        let todo_sig: String = self.todos.iter().map(|t| t.status.chars().next().unwrap_or('?')).collect();
        format!("{}|{}|{}|{}|{}|{}|{}", p.status, p.activity, self.tools.len(), last_ts, self.used_tokens, self.todos.len(), todo_sig)
    }
}

// ===== opencode =====
//
// opencode exposes NO HTTP/SSE on Windows (stdio IPC) and its rich state lives
// in a multi-GB SQLite DB that is locked while it runs. So we tail its
// structured logfmt log instead. cwd -> sessionID is resolved from
// %USERPROFILE%/.local/share/opencode/storage/directory-readme/<sid>.json
// (its injectedPaths array holds the launch cwd). From the log we surface
// status, current activity (file / step / model) and a read/write/edit tool
// timeline. Token usage and todos are not available on disk while it runs.

fn opencode_root() -> Option<PathBuf> {
    let home = home_dir()?;
    let candidates = [
        home.join(".local").join("share").join("opencode"), // Linux / Windows / XDG
        home.join("Library").join("Application Support").join("opencode"), // macOS
    ];
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

/// Configured opencode plugins + MCP servers from ~/.config/opencode/opencode.json
/// — these are readable (unlike the locked session DB) so we can always show the
/// session's plugin/MCP set. Returns display names.
fn read_opencode_plugins() -> Vec<String> {
    let Some(home) = home_dir() else { return vec![] };
    let cfg = home.join(".config").join("opencode").join("opencode.json");
    let Ok(txt) = std::fs::read_to_string(&cfg) else { return vec![] };
    // PowerShell-written configs carry a UTF-8 BOM that serde_json rejects.
    let txt = txt.trim_start_matches('\u{feff}');
    let Ok(v) = serde_json::from_str::<Value>(txt) else { return vec![] };
    let mut out = Vec::new();
    if let Some(m) = v["mcp"].as_object() {
        for k in m.keys() {
            out.push(k.clone());
        }
    }
    if let Some(p) = v["plugin"].as_array() {
        for x in p {
            if let Some(s) = x.as_str() {
                out.push(s.split('@').next().unwrap_or(s).to_string());
            }
        }
    }
    out
}

/// opencode sessionID bound to `cwd` (newest by updatedAt).
fn resolve_opencode_sid(root: &Path, cwd: &str) -> Option<String> {
    let dir = root.join("storage").join("directory-readme");
    let mut best: Option<(u64, String)> = None;
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&p) else { continue };
        let txt = txt.trim_start_matches('\u{feff}');
        let Ok(v) = serde_json::from_str::<Value>(txt) else { continue };
        let matched = v["injectedPaths"]
            .as_array()
            .map(|a| a.iter().any(|x| path_matches(x.as_str().unwrap_or(""), cwd)))
            .unwrap_or(false);
        if !matched {
            continue;
        }
        let sid = v["sessionID"].as_str().unwrap_or("").to_string();
        if sid.is_empty() {
            continue;
        }
        let upd = v["updatedAt"].as_u64().unwrap_or(0);
        if best.as_ref().map_or(true, |(b, _)| upd > *b) {
            best = Some((upd, sid));
        }
    }
    best.map(|(_, s)| s)
}

fn newest_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let Ok(m) = e.metadata().and_then(|md| md.modified()) else { continue };
        if best.as_ref().map_or(true, |(bm, _)| m > *bm) {
            best = Some((m, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Extract a logfmt field: ` key=value` or ` key="quoted value"`.
fn lf(line: &str, key: &str) -> Option<String> {
    let start = if line.starts_with(&format!("{key}=")) {
        key.len() + 1
    } else {
        line.find(&format!(" {key}="))? + key.len() + 2
    };
    let rest = &line[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"').unwrap_or(stripped.len());
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find(' ').unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

fn base_name(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn opencode_watch(app: AppHandle, pty_id: u32, cwd: String) {
    let Some(root) = opencode_root() else { return };
    let evt = format!("agent-progress:{pty_id}");

    let mut sid = String::new();
    let mut logpath: Option<PathBuf> = None;
    let mut offset: u64 = 0;
    let mut state = OpencodeState::default();
    let mut last_sig = String::new();
    let mut idle_polls: u32 = 0;
    let mut last_activity = String::new();
    let mut conv_seq: u64 = 0;

    loop {
        std::thread::sleep(Duration::from_millis(700));

        if sid.is_empty() {
            sid = resolve_opencode_sid(&root, &cwd).unwrap_or_default();
            if sid.is_empty() {
                continue;
            }
            state.sid = sid.clone();
            // surface configured plugins/MCP as chips (purple, mcp__ prefix)
            for name in read_opencode_plugins() {
                state.push_tool(&format!("mcp__{name}"), "plugin");
            }
        }

        let logdir = root.join("log");
        if let Some(n) = newest_file(&logdir) {
            if logpath.as_ref() != Some(&n) {
                logpath = Some(n);
                offset = 0;
            }
        }
        let Some(lp) = logpath.clone() else { continue };

        let mut saw = false;
        if let Ok(mut f) = File::open(&lp) {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < offset {
                offset = 0;
            }
            if len > offset && f.seek(SeekFrom::Start(offset)).is_ok() {
                let mut buf = String::new();
                if f.read_to_string(&mut buf).is_ok() {
                    if let Some(idx) = buf.rfind('\n') {
                        let complete = &buf[..=idx];
                        for line in complete.split('\n') {
                            let t = line.trim();
                            if t.is_empty() {
                                continue;
                            }
                            if state.ingest(t) {
                                saw = true;
                            }
                        }
                        offset += complete.as_bytes().len() as u64;
                    }
                }
            }
        }

        if state.canceled {
            state.status = "idle".into();
            state.canceled = false;
            idle_polls = 3;
        } else if saw {
            state.status = "working".into();
            idle_polls = 0;
        } else {
            // opencode logs nothing while the model is generating, so keep a
            // generous grace window before declaring idle (else it flips to
            // 대기 mid-turn). ~20s at the 700ms poll cadence.
            idle_polls = idle_polls.saturating_add(1);
            if idle_polls >= 28 {
                state.status = "idle".into();
            }
        }

        // opencode exposes no message bodies — surface its activity as a system
        // event stream so the 대화 view shows what it is doing.
        if !state.activity.is_empty() && state.activity != last_activity {
            last_activity = state.activity.clone();
            conv_seq += 1;
            let m = ConvMessage {
                id: format!("oc-{conv_seq}"),
                role: "system".into(),
                ts: String::new(),
                text: last_activity.clone(),
                thinking: None,
                tool_calls: vec![],
                usage: None,
            };
            let _ = app.emit(&format!("conv-msg:{pty_id}"), &m);
        }

        let prog = state.to_progress();
        let sig = state.signature(&prog);
        if sig != last_sig {
            last_sig = sig;
            let _ = app.emit(&evt, &prog);
        }
    }
}

#[derive(Default)]
struct OpencodeState {
    sid: String,
    active_run: String,
    status: String,
    activity: String,
    model: String,
    step: u64,
    canceled: bool,
    tools: Vec<ToolEvent>,
    tool_keys: HashSet<String>,
}
impl OpencodeState {
    /// Returns true when the line advanced OUR session (drives working/idle).
    fn ingest(&mut self, line: &str) -> bool {
        let msg = lf(line, "message").unwrap_or_default();
        let line_sid = lf(line, "session.id");
        let run = lf(line, "run");

        if line_sid.as_deref() == Some(self.sid.as_str()) {
            if let Some(r) = &run {
                self.active_run = r.clone();
            }
            match msg.as_str() {
                "loop" => {
                    if let Some(s) = lf(line, "step").and_then(|s| s.parse::<u64>().ok()) {
                        self.step = s;
                    }
                }
                "stream" => {
                    if let Some(m) = lf(line, "modelID") {
                        self.model = m;
                    }
                }
                "cancel" => self.canceled = true,
                _ => {}
            }
            return true;
        }

        // File-op lines carry run= but no session.id — attribute them to our
        // session when the run matches the one last seen for our sid.
        if !self.active_run.is_empty() && run.as_deref() == Some(self.active_run.as_str()) {
            match msg.as_str() {
                "touching file" => {
                    if let Some(f) = lf(line, "file") {
                        let b = base_name(&f);
                        self.push_tool("edit", &b);
                        self.activity = format!("edit {b}");
                        return true;
                    }
                }
                "evaluated" => {
                    let perm = lf(line, "permission").unwrap_or_default();
                    if !perm.is_empty() {
                        if let Some(pat) = lf(line, "pattern") {
                            self.push_tool(&perm, &base_name(&pat));
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn push_tool(&mut self, name: &str, summary: &str) {
        let key = format!("{name}|{summary}");
        if self.tool_keys.contains(&key) {
            return;
        }
        self.tool_keys.insert(key);
        self.tools.push(ToolEvent {
            ts: String::new(),
            name: name.to_string(),
            summary: summary.to_string(),
        });
        if self.tools.len() > 100 {
            let cut = self.tools.len() - 100;
            self.tools.drain(0..cut);
        }
    }

    fn to_progress(&self) -> AgentProgress {
        let status = if self.status.is_empty() { "idle" } else { self.status.as_str() };
        let activity = if status == "working" {
            if !self.activity.is_empty() {
                self.activity.clone()
            } else if self.step > 0 && !self.model.is_empty() {
                format!("step {} · {}", self.step, self.model)
            } else if self.step > 0 {
                format!("step {}", self.step)
            } else if !self.model.is_empty() {
                self.model.clone()
            } else {
                "작업 중".into()
            }
        } else {
            String::new()
        };
        // always surface plugin/MCP chips (pushed once, early) + the most recent
        // file-op tools, so plugins are never squeezed out of the tail window.
        let mut tools: Vec<ToolEvent> =
            self.tools.iter().filter(|t| t.name.starts_with("mcp__")).cloned().collect();
        let others: Vec<&ToolEvent> =
            self.tools.iter().filter(|t| !t.name.starts_with("mcp__")).collect();
        let start = others.len().saturating_sub(20);
        tools.extend(others[start..].iter().map(|t| (*t).clone()));
        AgentProgress {
            status: status.into(),
            activity,
            todos: vec![],
            tools,
            context: None,
        }
    }

    fn signature(&self, p: &AgentProgress) -> String {
        format!("{}|{}|{}|{}", p.status, p.activity, self.tools.len(), self.step)
    }
}
