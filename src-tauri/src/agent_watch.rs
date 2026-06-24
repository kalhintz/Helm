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
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use notify::{RecursiveMode, Watcher};

/// Filesystem-change signal for a directory tree. The watch loops block on the
/// returned receiver so they react the instant an agent writes its log, instead
/// of waking on a fixed poll. The returned watcher must be kept alive (dropping
/// it stops the watch); a periodic recv_timeout still serves as a safety net.
fn dir_signal(dir: &Path) -> (Option<notify::RecommendedWatcher>, Receiver<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .ok()
    .and_then(|mut w| match w.watch(dir, RecursiveMode::Recursive) {
        Ok(_) => Some(w),
        Err(_) => None,
    });
    (watcher, rx)
}

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

use crate::mobile::emit_all;

#[derive(Clone, Serialize, Default)]
pub struct AgentProgress {
    /// working | waiting | done | idle | error
    pub status: String,
    pub activity: String,
    pub todos: Vec<TodoItem>,
    pub tools: Vec<ToolEvent>,
    pub context: Option<Context>,
    /// opencode-only: cleaned display mode/agent (e.g. "Sisyphus - Ultraworker").
    /// `skip` keeps the Claude/Codex payloads byte-identical.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// opencode-only: modelID (e.g. "claude-opus-4-8").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// opencode-only: providerID (e.g. "anthropic").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// opencode-only: bound session id, so the frontend can drive the switch API.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sid: String,
    /// Granular current sub-step (tool + target). Skipped when no live tool —
    /// keeps existing Claude/Codex payloads byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<CurrentTool>,
    /// Index into `todos` of the active (in_progress) task, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_todo_index: Option<usize>,
    /// Step counter, e.g. "4/8" or "4". Empty = omitted.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub step_display: String,
}

#[derive(Clone, Serialize)]
pub struct CurrentTool {
    pub name: String,
    pub target: String,
    /// running | completed | error
    pub status: String,
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
pub struct ConvImageBlock {
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
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
    /// Inline images (base64 data URIs). Skipped when empty so existing
    /// conv-msg payloads stay byte-identical.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ConvImageBlock>,
}

/// Cap on inlined base64 length (~3MB decoded). Larger images are skipped to
/// avoid bloating one IPC frame.
const MAX_INLINE_IMG_B64: usize = 4_000_000;

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
            images: vec![],
        };
        emit_all(app, &format!("conv-msg:{pty_id}"), &m);
    } else if typ == "user" {
        let content = &msg["content"];
        let mut images: Vec<ConvImageBlock> = Vec::new();
        let mut arr_text = String::new();
        if let Some(arr) = content.as_array() {
            let mut had_result = false;
            for item in arr {
                match item["type"].as_str() {
                    Some("tool_result") => {
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
                        emit_all(app, &format!("conv-tool:{pty_id}"), &tc);
                    }
                    Some("text") => {
                        if let Some(t) = item["text"].as_str() {
                            if !arr_text.is_empty() {
                                arr_text.push('\n');
                            }
                            arr_text.push_str(t);
                        }
                    }
                    Some("image") => {
                        if let Some(img) = claude_image_block(item) {
                            images.push(img);
                        }
                    }
                    _ => {}
                }
            }
            // a tool_result turn carries no user-visible body — but an image-only
            // paste arrives as an array too, so only bail when there's nothing to show.
            if had_result && arr_text.trim().is_empty() && images.is_empty() {
                return;
            }
        }
        let text = if !arr_text.is_empty() {
            arr_text
        } else {
            content.as_str().unwrap_or("").to_string()
        };
        if text.trim().is_empty() && images.is_empty() {
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
            images,
        };
        emit_all(app, &format!("conv-msg:{pty_id}"), &m);
    }
}

/// Build a `ConvImageBlock` from a Claude `{type:"image", source:{...}}` content
/// block. Only base64 sources are inlined; oversized images are skipped (None).
fn claude_image_block(item: &Value) -> Option<ConvImageBlock> {
    let src = &item["source"];
    if src["type"].as_str() != Some("base64") {
        return None;
    }
    let mime = src["media_type"].as_str().unwrap_or("image/png").to_string();
    let data = src["data"].as_str()?;
    if data.len() > MAX_INLINE_IMG_B64 {
        return None;
    }
    Some(ConvImageBlock {
        mime: mime.clone(),
        data_uri: Some(format!("data:{};base64,{}", mime, data)),
        alt: Some("스크린샷".into()),
    })
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
        if m + Duration::from_secs(86_400) < after {
            continue;
        }
        if best.as_ref().map_or(true, |(bm, _)| m > *bm) {
            best = Some((m, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Read Claude's token context from OMC's per-project HUD cache
/// (`<cwd>/.omc/state/hud-stdin-cache.json`). OMC already computes the exact
/// usage, so when it's present this is far more reliable than parsing the
/// transcript (which can live under an unexpected slug).
fn read_omc_context(cwd: &str) -> Option<Context> {
    let path = Path::new(cwd)
        .join(".omc")
        .join("state")
        .join("hud-stdin-cache.json");
    let txt = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(txt.trim_start_matches('\u{feff}')).ok()?;
    let cw = &v["context_window"];
    let max = cw["context_window_size"].as_u64().unwrap_or(0);
    if max == 0 {
        return None;
    }
    let cu = &cw["current_usage"];
    let used = cu["input_tokens"].as_u64().unwrap_or(0)
        + cu["output_tokens"].as_u64().unwrap_or(0)
        + cu["cache_creation_input_tokens"].as_u64().unwrap_or(0)
        + cu["cache_read_input_tokens"].as_u64().unwrap_or(0);
    let pct = cw["used_percentage"]
        .as_u64()
        .map(|p| p.min(100) as u8)
        .unwrap_or_else(|| ((used as f64 / max as f64) * 100.0).min(100.0) as u8);
    if used == 0 && pct == 0 {
        return None;
    }
    Some(Context { used, max, pct })
}

/// Find the active Claude transcript by scanning ALL project dirs for the newest
/// *.jsonl whose recorded `cwd` matches this session — robust for vanilla Claude
/// (no OMC, no hooks), since Claude may write under a git-root slug rather than
/// the cwd slug. Mirrors how codex rollouts are matched.
fn newest_claude_transcript(projects: &Path, after: SystemTime, cwd: &str) -> Option<PathBuf> {
    let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(projects) {
        for slug in rd.flatten() {
            let sd = slug.path();
            if !sd.is_dir() {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(&sd) {
                for e in files.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Ok(m) = e.metadata().and_then(|md| md.modified()) else {
                        continue;
                    };
                    if m + Duration::from_secs(86_400) < after {
                        continue;
                    }
                    candidates.push((m, p));
                }
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in candidates {
        let matched = first_lines(&p, 15)
            .iter()
            .any(|v| v["cwd"].as_str().map_or(false, |c| path_matches(c, cwd)));
        if matched {
            return Some(p);
        }
    }
    None
}

fn claude_watch(app: AppHandle, pty_id: u32, cwd: String) {
    let Some(home) = home_dir() else { return };
    let projects = home.join(".claude").join("projects");
    let dir = projects.join(slug_for_cwd(&cwd));
    let evt = format!("agent-progress:{pty_id}");
    let start = SystemTime::now();

    let mut cur_path: Option<PathBuf> = None;
    let mut offset: u64 = 0;
    let mut state = ClaudeState::default();
    let mut last_sig = String::new();
    let mut pending: HashMap<String, String> = HashMap::new();
    let (_fs_watch, fs_rx) = dir_signal(&projects);

    loop {
        // wake the instant claude writes its transcript; 1.2s is just a safety net
        let _ = fs_rx.recv_timeout(Duration::from_millis(1200));

        // Prefer the EXACT transcript path a Claude hook handed us; fall back to the
        // cwd->slug guess. Claude often writes under a git-root slug (not cwd), so the
        // guess can miss the file entirely — which is why token context went untracked.
        let selected = crate::hook_server::transcript_for(&app, pty_id)
            .map(PathBuf::from)
            .filter(|p| p.exists())
            // robust vanilla path: newest transcript anywhere whose cwd matches
            .or_else(|| newest_claude_transcript(&projects, start, &cwd))
            // last resort: the cwd->slug guess
            .or_else(|| if dir.exists() { newest_jsonl(&dir, start) } else { None });

        // (re)select the active transcript; on switch, restart from byte 0.
        if let Some(newest) = selected {
            if cur_path.as_ref() != Some(&newest) {
                cur_path = Some(newest);
                offset = 0;
                state = ClaudeState::default();
                pending.clear();
                emit_all(&app, &format!("conv-reset:{pty_id}"), ());
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

        let mut prog = state.to_progress(idle);
        // OMC writes an exact HUD cache (model/context/cost) per project — read it
        // for token context. Reliable regardless of which slug Claude wrote to and
        // needs no hook approval, so context tracks even before hooks fire.
        let omc_ctx = read_omc_context(&cwd);
        if omc_ctx.is_some() {
            prog.context = omc_ctx.clone();
        }
        let sig = format!(
            "{}|{}",
            state.signature(&prog),
            omc_ctx.as_ref().map(|c| c.pct).unwrap_or(255)
        );
        if sig != last_sig {
            last_sig = sig;
            // Once native hooks are live for this pty they own status/activity/todos;
            // the watcher then contributes only token context (and conversation).
            if crate::hook_server::hooks_active(&app, pty_id) {
                emit_all(&app, &evt, serde_json::json!({ "context": prog.context }));
            } else {
                emit_all(&app, &evt, &prog);
            }
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
    // granular: live status of the most recent tool_use, by tool_use_id.
    tool_id_status: HashMap<String, String>, // tool_use_id -> running|completed|error
    last_tool_id: String,                    // id of the most recent tool_use
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
                    // granular: flip the live tool status running -> completed/error.
                    let st = if item["is_error"].as_bool().unwrap_or(false) { "error" } else { "completed" };
                    self.tool_id_status.insert(tuid.to_string(), st.into());
                    if self.tool_id_status.len() > 100 {
                        // drop everything but the most recent tool's status.
                        let keep = self.last_tool_id.clone();
                        self.tool_id_status.retain(|k, _| k == &keep);
                    }
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
                        // granular: mark this tool live until its tool_result lands.
                        if let Some(id) = item["id"].as_str() {
                            self.last_tool_id = id.to_string();
                            self.tool_id_status.insert(id.to_string(), "running".into());
                        }
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
        let todos: Vec<TodoItem> = if !self.omc_tasks.is_empty() {
            self.omc_tasks
                .iter()
                .map(|(_, text, status)| TodoItem { text: text.clone(), status: status.clone() })
                .collect()
        } else {
            self.todos.clone()
        };

        let active_todo_index = first_in_progress(&todos);
        let current_tool = if status == "working" && !self.last_tool.is_empty() {
            let st = self
                .tool_id_status
                .get(&self.last_tool_id)
                .cloned()
                .unwrap_or_else(|| "running".into());
            Some(CurrentTool {
                name: self.last_tool.clone(),
                target: self.last_tool_summary.clone(),
                status: st,
            })
        } else {
            None
        };

        AgentProgress {
            status: status.into(),
            activity,
            todos,
            tools,
            context,
            current_tool,
            active_todo_index,
            ..Default::default()
        }
    }

    fn signature(&self, p: &AgentProgress) -> String {
        let done = p.todos.iter().filter(|t| t.status == "completed").count();
        let last_ts = self.tools.last().map(|t| t.ts.as_str()).unwrap_or("");
        // include the task TEXT, not just status, so a revised plan (same statuses,
        // different steps) re-emits and the rail re-syncs.
        let todo_sig: String = p.todos.iter().map(|t| format!("{}/{};", t.text, t.status)).collect();
        let cur = p
            .current_tool
            .as_ref()
            .map(|t| format!("{}/{}/{}", t.name, t.target, t.status))
            .unwrap_or_default();
        format!(
            "{}|{}|{}/{}|{}|{}|{}|{}|{}|{}",
            p.status,
            p.activity,
            done,
            p.todos.len(),
            self.tools.len(),
            last_ts,
            self.used_tokens,
            todo_sig,
            cur,
            p.active_todo_index.map(|i| i.to_string()).unwrap_or_default()
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

fn first_in_progress(todos: &[TodoItem]) -> Option<usize> {
    todos.iter().position(|t| t.status == "in_progress")
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
            if m + Duration::from_secs(86_400) < after {
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
                images: vec![],
            };
            emit_all(app, &format!("conv-msg:{pty_id}"), &m);
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
            emit_all(app, &format!("conv-tool:{pty_id}"), &tc);
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
                    images: vec![],
                };
                emit_all(app, &format!("conv-msg:{pty_id}"), &m);
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
                        images: vec![],
                    };
                    emit_all(app, &format!("conv-msg:{pty_id}"), &m);
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
    let (_fs_watch, fs_rx) = dir_signal(&root);

    loop {
        let _ = fs_rx.recv_timeout(Duration::from_millis(1200));
        if !root.exists() {
            continue;
        }
        if let Some(p) = newest_codex_rollout(&root, start, &cwd) {
            if cur.as_ref() != Some(&p) {
                cur = Some(p);
                offset = 0;
                state = CodexState::default();
                pending.clear();
                emit_all(&app, &format!("conv-reset:{pty_id}"), ());
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
            // Codex's tasks (from `update_plan`) and tools live only in its rollout
            // log — no Codex hook event carries them — so unlike Claude/opencode the
            // watcher stays authoritative here even when the hook is live. The
            // rollout log is read on filesystem-event wake, so this is already
            // instant; the Codex hook just adds an earlier status/turn nudge.
            emit_all(&app, &evt, &prog);
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
    cur_tool: Option<(String, String)>, // (name, summary) of the last function_call
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
                        // granular: remember the live tool for the right-rail sub-step.
                        self.cur_tool = Some((name.clone(), summary.clone()));
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
        let total = self.todos.len();
        let done = self.todos.iter().filter(|t| t.status == "completed").count();
        let step_display = if total > 0 { format!("{}/{}", done.min(total), total) } else { String::new() };
        let active_todo_index = first_in_progress(&self.todos);
        let current_tool = if status == "working" {
            self.cur_tool
                .as_ref()
                .map(|(n, s)| CurrentTool { name: n.clone(), target: s.clone(), status: "running".into() })
        } else {
            None
        };
        AgentProgress {
            status: status.into(),
            activity,
            todos: self.todos.clone(),
            tools,
            context,
            current_tool,
            active_todo_index,
            step_display,
            ..Default::default()
        }
    }
    fn signature(&self, p: &AgentProgress) -> String {
        let last_ts = self.tools.last().map(|t| t.ts.as_str()).unwrap_or("");
        // include task TEXT (not just status) so a revised codex plan re-syncs.
        let todo_sig: String = self.todos.iter().map(|t| format!("{}/{};", t.text, t.status)).collect();
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            p.status,
            p.activity,
            self.tools.len(),
            last_ts,
            self.used_tokens,
            self.todos.len(),
            todo_sig,
            p.step_display,
            p.active_todo_index.map(|i| i.to_string()).unwrap_or_default()
        )
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

/// Path to opencode's SQLite store (a multi-GB WAL DB). We open it READ-ONLY so a
/// concurrent opencode writer is never blocked.
fn opencode_db_path() -> Option<PathBuf> {
    let p = opencode_root()?.join("opencode.db");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

/// Open opencode.db read-only. WAL readers don't block the writer; never set
/// journal_mode on a read-only handle. busy_timeout keeps us from erroring out
/// if a checkpoint briefly holds a lock.
fn open_opencode_db(path: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(300));
    Some(conn)
}

/// opencode's mode/agent strings carry a leading zero-width space (U+200B); strip
/// it (and surrounding whitespace) so the chip reads "Sisyphus - Ultraworker".
fn clean_mode(s: &str) -> String {
    s.trim_start_matches('\u{200b}').trim().to_string()
}

/// Context window (max tokens) for a model. Prefer the live `/api/model`
/// `limit.context` at runtime; this is the offline fallback when the HTTP API
/// isn't reachable.
fn model_context_max(model_id: &str) -> u64 {
    match model_id {
        "claude-opus-4-8" | "claude-opus-4-7" | "claude-sonnet-4-6" => 1_000_000,
        "claude-haiku-4-5" => 200_000,
        "claude-fable-5" => 100_000,
        "gpt-5.5" | "gpt-5.5-pro" => 200_000,
        "gpt-5.4" | "gpt-5.4-mini" => 128_000,
        "gpt-5.3-codex" => 100_000,
        "gemini-3.1-pro-preview" | "gemini-3-flash-preview" => 1_000_000,
        _ => 200_000,
    }
}

/// Newest non-archived session whose directory matches `cwd` (normalize
/// backslash->slash, lowercase). Returns (session_id, directory). Always LIMIT —
/// the DB is huge.
fn db_bind_session(conn: &Connection, cwd: &str) -> Option<(String, String)> {
    let norm = cwd.replace('\\', "/").to_lowercase();
    conn.query_row(
        "SELECT id, directory FROM session
         WHERE LOWER(REPLACE(directory,'\\','/')) = ?1
           AND time_archived IS NULL
         ORDER BY time_updated DESC LIMIT 1",
        [norm],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .ok()
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

/// One-line summary for an opencode tool part's `state.input`
/// (filePath / command / pattern / description), mirroring summarize_input.
fn oc_tool_summary(input: &Value) -> String {
    for k in ["filePath", "file_path", "command", "pattern", "query", "description", "url", "path"] {
        if let Some(s) = input[k].as_str() {
            let s = s.trim().replace(['\n', '\r'], " ");
            if !s.is_empty() {
                // file-ish keys read better as a basename
                let v = if k.contains("ath") || k == "filePath" { base_name(&s) } else { s };
                return v.chars().take(60).collect();
            }
        }
    }
    String::new()
}

/// Best-effort image extraction from an opencode `file`/`image` part. opencode
/// commonly stores either a `data:` URL or a base64 field. If the shape isn't
/// clearly an inline image, return None (don't guess).
fn oc_image_block(p: &Value) -> Option<ConvImageBlock> {
    let mime = p["mime"]
        .as_str()
        .or_else(|| p["media_type"].as_str())
        .or_else(|| p["mimeType"].as_str())
        .unwrap_or("");
    // a data: URL carries its own mime; otherwise require an image/* mime.
    if let Some(u) = p["url"].as_str().filter(|u| u.starts_with("data:image/")) {
        if u.len() <= MAX_INLINE_IMG_B64 {
            let m = mime.is_empty().then(|| "image/png").unwrap_or(mime);
            return Some(ConvImageBlock {
                mime: m.to_string(),
                data_uri: Some(u.to_string()),
                alt: Some("이미지".into()),
            });
        }
        return None;
    }
    if !mime.starts_with("image/") {
        return None;
    }
    let b = p["base64"].as_str().or_else(|| p["data"].as_str())?;
    if b.len() > MAX_INLINE_IMG_B64 {
        return None;
    }
    Some(ConvImageBlock {
        mime: mime.to_string(),
        data_uri: Some(format!("data:{};base64,{}", mime, b)),
        alt: Some("이미지".into()),
    })
}

fn opencode_watch(app: AppHandle, pty_id: u32, cwd: String) {
    let Some(root) = opencode_root() else { return };
    let evt = format!("agent-progress:{pty_id}");

    // Open the SQLite store once, read-only. It owns conversation/todos/context/
    // mode/model/status; the logfmt tail stays as an instant file-op fallback.
    let db = opencode_db_path().and_then(|p| open_opencode_db(&p));

    let mut sid = String::new();
    let mut logpath: Option<PathBuf> = None;
    let mut offset: u64 = 0;
    let mut state = OpencodeState::default();
    let mut last_sig = String::new();
    let mut idle_polls: u32 = 0;
    let mut last_activity = String::new();
    let mut conv_seq: u64 = 0;
    let (_fs_watch, fs_rx) = dir_signal(&root);

    loop {
        // tighter cadence than the other watchers so conversation feels live.
        let _ = fs_rx.recv_timeout(Duration::from_millis(900));

        if sid.is_empty() {
            // Prefer the DB binder (no lock under WAL read); fall back to the JSON scan.
            sid = db
                .as_ref()
                .and_then(|c| db_bind_session(c, &cwd))
                .map(|(id, _dir)| id)
                .or_else(|| resolve_opencode_sid(&root, &cwd))
                .unwrap_or_default();
            if sid.is_empty() {
                continue;
            }
            state.sid = sid.clone();
            state.emitted_msgs.clear();
            emit_all(&app, &format!("conv-reset:{pty_id}"), ());
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

        let mut saw = false;
        if let Some(lp) = logpath.clone() {
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
        }

        // logfmt fast-path status (only used when the DB has no row yet — see
        // mapped_status). Keep updating it so the fallback stays meaningful.
        if state.canceled {
            state.status = "idle".into();
            state.canceled = false;
            idle_polls = 3;
        } else if saw {
            state.status = "working".into();
            idle_polls = 0;
        } else {
            idle_polls = idle_polls.saturating_add(1);
            if idle_polls >= 28 {
                state.status = "idle".into();
            }
        }

        // The DB is authoritative: mode/model/context/todos/status + conversation.
        // Conversation is always emitted from the DB; the frontend's
        // `showConversation` setting decides whether to render it.
        if let Some(conn) = db.as_ref() {
            state.db_sync(&app, conn, pty_id, true);
        }

        // Fallback conversation: only when the DB is absent — surface logfmt
        // activity as a system event so the 대화 view isn't empty.
        if db.is_none() && !state.activity.is_empty() && state.activity != last_activity {
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
                images: vec![],
            };
            emit_all(&app, &format!("conv-msg:{pty_id}"), &m);
        }

        let prog = state.to_progress();
        let sig = state.signature(&prog);
        if sig != last_sig {
            last_sig = sig;
            // The DB watcher is AUTHORITATIVE for opencode — it owns conversation,
            // tasks, context, mode/model AND status (none of which the helm-bridge
            // hook carries). So, like the Codex arm, always emit the full progress
            // even when the hook is live; the hook just adds an earlier nudge.
            emit_all(&app, &evt, &prog);
        }

        // ---- notification edge-trigger: turn complete (working -> awaiting) ----
        if state.prev_db_status == "working" && state.db_status == "awaiting_input" {
            let now = now_ms();
            if now.saturating_sub(state.last_turn_done_ms) >= 2000 {
                state.last_turn_done_ms = now;
                emit_all(
                    &app,
                    &format!("agent-turn-done:{pty_id}"),
                    serde_json::json!({ "title": state.mode, "model": state.model }),
                );
            }
        }
        state.prev_db_status = state.db_status.clone();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Default)]
struct OpencodeState {
    sid: String,
    // logfmt fast-path (instant file-op activity before the DB row lands):
    active_run: String,
    status: String,
    activity: String,
    model: String, // modelID (DB-authoritative; logfmt stream= as fallback)
    step: u64,
    canceled: bool,
    tools: Vec<ToolEvent>,
    tool_keys: HashSet<String>,
    // DB-sourced (authoritative for conversation / todos / context / mode):
    mode: String,
    provider: String,
    context: Option<Context>,
    todos: Vec<TodoItem>,
    emitted_msgs: HashSet<String>, // message ids already sent as a FINAL conv-msg
    db_status: String,             // working | awaiting_input | queued | idle
    prev_db_status: String,        // edge-trigger for turn-done notifications
    last_turn_done_ms: u64,
    cur_tool: Option<(String, String, String)>, // (name, summary, status) of newest tool part
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

    /// Pull the authoritative state for our bound session out of opencode.db:
    /// mode/model/provider, token context, todos, status — and emit conv-msg /
    /// conv-tool for the conversation view. Every query is LIMITed (huge DB).
    fn db_sync(&mut self, app: &AppHandle, conn: &Connection, pty_id: u32, emit_conv: bool) {
        if self.sid.is_empty() {
            return;
        }

        // ---- latest assistant message: mode / model / provider / status ----
        let latest_assistant: Option<Value> = conn
            .query_row(
                "SELECT data FROM message
                 WHERE session_id = ?1 AND json_extract(data,'$.role')='assistant'
                 ORDER BY time_created DESC LIMIT 1",
                [&self.sid],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());

        if let Some(d) = &latest_assistant {
            if let Some(m) = d["mode"].as_str().or_else(|| d["agent"].as_str()) {
                let cleaned = clean_mode(m);
                if !cleaned.is_empty() {
                    self.mode = cleaned;
                }
            }
            if let Some(m) = d["modelID"].as_str() {
                if !m.is_empty() {
                    self.model = m.to_string();
                }
            }
            if let Some(p) = d["providerID"].as_str() {
                if !p.is_empty() {
                    self.provider = p.to_string();
                }
            }
        }

        // ---- context: newest non-zero tokens.total across assistant messages ----
        // The very latest message can be mid-stream/aborted (tokens all zero), so
        // take the newest assistant whose tokens.total > 0.
        let used: Option<u64> = conn
            .query_row(
                "SELECT json_extract(data,'$.tokens.total') FROM message
                 WHERE session_id = ?1 AND json_extract(data,'$.role')='assistant'
                   AND COALESCE(json_extract(data,'$.tokens.total'),0) > 0
                 ORDER BY time_created DESC LIMIT 1",
                [&self.sid],
                |r| r.get::<_, Option<u64>>(0),
            )
            .ok()
            .flatten();
        if let Some(u) = used {
            if u > 0 {
                let max = model_context_max(&self.model);
                let pct = ((u as f64 / max as f64) * 100.0).round().min(100.0) as u8;
                self.context = Some(Context { used: u, max, pct });
            }
        }

        // ---- todos ----
        if let Ok(mut stmt) =
            conn.prepare("SELECT content, status FROM todo WHERE session_id = ?1 ORDER BY position")
        {
            if let Ok(rows) = stmt.query_map([&self.sid], |r| {
                Ok(TodoItem {
                    text: r.get::<_, String>(0)?,
                    status: r.get::<_, String>(1)?,
                })
            }) {
                let todos: Vec<TodoItem> = rows.flatten().collect();
                self.todos = todos;
            }
        }

        // ---- granular: newest tool part (name + summary + live status) ----
        self.cur_tool = None;
        if let Ok(row) = conn.query_row(
            "SELECT p.data FROM part p
             JOIN message m ON m.id = p.message_id
             WHERE m.session_id = ?1 AND json_extract(p.data,'$.type')='tool'
             ORDER BY p.time_created DESC LIMIT 1",
            [&self.sid],
            |r| r.get::<_, String>(0),
        ) {
            if let Ok(pv) = serde_json::from_str::<Value>(&row) {
                let st = &pv["state"];
                let status = match st["status"].as_str().unwrap_or("") {
                    "completed" => "completed",
                    "error" => "error",
                    _ => "running",
                };
                self.cur_tool = Some((
                    pv["tool"].as_str().unwrap_or("tool").to_string(),
                    oc_tool_summary(&st["input"]),
                    status.to_string(),
                ));
            }
        }

        // ---- status detection ----
        // latest message overall (user or assistant) decides who owes the next move.
        let latest: Option<(String, Value)> = conn
            .query_row(
                "SELECT json_extract(data,'$.role'), data FROM message
                 WHERE session_id = ?1 ORDER BY time_created DESC LIMIT 1",
                [&self.sid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
            .and_then(|(role, s)| serde_json::from_str::<Value>(&s).ok().map(|v| (role, v)));

        let mut db_status = "idle".to_string();
        if let Some((role, d)) = &latest {
            if role == "assistant" {
                let done = !d["finish"].is_null() || !d["error"].is_null();
                db_status = if done { "awaiting_input" } else { "working" }.to_string();
            } else if role == "user" {
                // assistant owes a reply
                db_status = "working".to_string();
            }
        }
        // queued input overrides — still busy, and suppresses the awaiting toast.
        let queued: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_input WHERE session_id = ?1 AND promoted_seq IS NULL",
                [&self.sid],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if queued > 0 {
            db_status = "queued".to_string();
        }
        self.db_status = db_status;

        // ---- conversation reconstruction (gated on the setting via emit_conv) ----
        if emit_conv {
            self.db_emit_conversation(app, conn, pty_id);
        }
    }

    /// Build ConvMessages for the newest ~40 messages and emit conv-msg for any
    /// not yet emitted as final. Streaming messages (finish==null) are emitted but
    /// NOT marked final, so the completed version later replaces them (frontend
    /// de-dupes by id).
    fn db_emit_conversation(&mut self, app: &AppHandle, conn: &Connection, pty_id: u32) {
        let mut stmt = match conn.prepare(
            "SELECT m.id, m.time_created, m.data, p.time_created, p.data
             FROM message m
             LEFT JOIN part p ON p.message_id = m.id
             WHERE m.id IN (
                 SELECT id FROM message WHERE session_id = ?1
                 ORDER BY time_created DESC LIMIT 40
             )
             ORDER BY m.time_created, p.time_created",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let rows = match stmt.query_map([&self.sid], |r| {
            Ok((
                r.get::<_, String>(0)?,            // message id
                r.get::<_, i64>(1)?,               // message ts
                r.get::<_, String>(2)?,            // message data
                r.get::<_, Option<String>>(4)?,    // part data (may be null)
            ))
        }) {
            Ok(r) => r,
            Err(_) => return,
        };

        // group parts under each message, preserving order
        let mut order: Vec<String> = Vec::new();
        let mut by_msg: HashMap<String, (i64, Value, Vec<Value>)> = HashMap::new();
        for row in rows.flatten() {
            let (mid, ts, mdata, pdata) = row;
            let entry = by_msg.entry(mid.clone()).or_insert_with(|| {
                order.push(mid.clone());
                let mv = serde_json::from_str::<Value>(&mdata).unwrap_or(Value::Null);
                (ts, mv, Vec::new())
            });
            if let Some(pd) = pdata {
                if let Ok(pv) = serde_json::from_str::<Value>(&pd) {
                    entry.2.push(pv);
                }
            }
        }

        for mid in order {
            let Some((ts, mdata, parts)) = by_msg.remove(&mid) else { continue };
            if mdata.is_null() {
                continue;
            }
            let role = mdata["role"].as_str().unwrap_or("assistant").to_string();
            let mut text = String::new();
            let mut thinking = String::new();
            let mut tool_calls: Vec<ConvToolCall> = Vec::new();
            let mut images: Vec<ConvImageBlock> = Vec::new();
            for p in &parts {
                match p["type"].as_str().unwrap_or("") {
                    "file" | "image" => {
                        if let Some(img) = oc_image_block(p) {
                            images.push(img);
                        }
                    }
                    "text" => {
                        if let Some(t) = p["text"].as_str() {
                            if !t.is_empty() {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(t);
                            }
                        }
                    }
                    "reasoning" => {
                        if let Some(t) = p["text"].as_str() {
                            if !t.is_empty() {
                                if !thinking.is_empty() {
                                    thinking.push('\n');
                                }
                                thinking.push_str(t);
                            }
                        }
                    }
                    "tool" => {
                        let st = &p["state"];
                        let status = match st["status"].as_str().unwrap_or("") {
                            "completed" => "completed",
                            "error" => "error",
                            _ => "running",
                        };
                        let result = st["output"]
                            .as_str()
                            .or_else(|| st["metadata"]["preview"].as_str())
                            .map(|s| truncate_str(s, 2000));
                        tool_calls.push(ConvToolCall {
                            id: p["callID"].as_str().unwrap_or("").to_string(),
                            name: p["tool"].as_str().unwrap_or("tool").to_string(),
                            summary: oc_tool_summary(&st["input"]),
                            status: status.to_string(),
                            result,
                        });
                    }
                    _ => {}
                }
            }
            if text.is_empty() && thinking.is_empty() && tool_calls.is_empty() && images.is_empty() {
                continue;
            }
            let usage = if role == "assistant" {
                mdata["tokens"]["total"].as_u64().filter(|t| *t > 0).map(|used| ConvUsage {
                    used,
                    max: model_context_max(mdata["modelID"].as_str().unwrap_or("")),
                })
            } else {
                None
            };
            let finished = !mdata["finish"].is_null() || !mdata["error"].is_null() || role == "user";
            // re-emit streaming messages every tick; only suppress finals we've sent.
            if finished && self.emitted_msgs.contains(&mid) {
                continue;
            }
            let m = ConvMessage {
                id: mid.clone(),
                role,
                ts: ts.to_string(),
                text,
                thinking: if thinking.is_empty() { None } else { Some(thinking) },
                tool_calls,
                usage,
                images,
            };
            emit_all(app, &format!("conv-msg:{pty_id}"), &m);
            if finished {
                self.emitted_msgs.insert(mid);
            }
        }
    }

    /// Map the DB-derived state to the frontend status vocabulary
    /// (working | waiting | idle). The watcher is authoritative for opencode.
    fn mapped_status(&self) -> &str {
        match self.db_status.as_str() {
            "working" | "queued" => "working",
            "awaiting_input" => "waiting",
            "idle" => "idle",
            // No DB row yet — fall back to the logfmt fast-path.
            _ => {
                if self.status.is_empty() {
                    "idle"
                } else {
                    self.status.as_str()
                }
            }
        }
    }

    fn to_progress(&self) -> AgentProgress {
        let status = self.mapped_status();
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
        let active_todo_index = first_in_progress(&self.todos);
        let current_tool = if status == "working" {
            self.cur_tool
                .as_ref()
                .map(|(n, s, st)| CurrentTool { name: n.clone(), target: s.clone(), status: st.clone() })
        } else {
            None
        };
        let step_display = if self.step > 0 { format!("{}", self.step) } else { String::new() };
        AgentProgress {
            status: status.into(),
            activity,
            todos: self.todos.clone(),
            tools,
            context: self.context.clone(),
            mode: self.mode.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            sid: self.sid.clone(),
            current_tool,
            active_todo_index,
            step_display,
        }
    }

    fn signature(&self, p: &AgentProgress) -> String {
        let todo_sig: String = self
            .todos
            .iter()
            .map(|t| format!("{}/{};", t.text, t.status))
            .collect();
        let ctx = self.context.as_ref().map(|c| c.used).unwrap_or(0);
        let cur = p
            .current_tool
            .as_ref()
            .map(|t| format!("{}/{}/{}", t.name, t.target, t.status))
            .unwrap_or_default();
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            p.status,
            p.activity,
            self.tools.len(),
            self.step,
            self.mode,
            self.model,
            ctx,
            todo_sig,
            self.emitted_msgs.len(),
            self.db_status,
            cur,
            p.active_todo_index.map(|i| i.to_string()).unwrap_or_default(),
        )
    }
}
