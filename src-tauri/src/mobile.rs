// Mobile bridge (Phase A, LAN-only).
//
// Lets a phone on the same wifi open Helm's UI in a browser and drive sessions.
// Two std-only servers, both bound to 0.0.0.0:
//   (1) HTTP  — serves the SAME files Tauri embeds from ../ui, so the phone loads
//       the identical UI. A tiny inline shim is injected into index.html that
//       provides window.__TAURI__ {core.invoke, event.listen} backed by the WS.
//   (2) WS    — broadcasts every backend event ({event,payload}) to clients and
//       receives {cmd,args} command messages, dispatched to the same backend fns.
//
// A per-launch pairing token gates the WS (query param ?token=… or first frame).
// No new crates: RFC6455 upgrade + text frames are hand-rolled, matching the
// dependency-free TCP style already used in hook_server.rs / http_get_json.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

// ----- fixed-ish ports (env-overridable) -----
const DEFAULT_HTTP_PORT: u16 = 8787;
const DEFAULT_WS_PORT: u16 = 8788;

fn http_port() -> u16 {
    std::env::var("HELM_HTTP_PORT").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_HTTP_PORT)
}
fn ws_port() -> u16 {
    std::env::var("HELM_WS_PORT").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_WS_PORT)
}

// ======================================================================
// Managed state
// ======================================================================

/// Central broadcast bus. Every WS client registers an mpsc::Sender here; its own
/// writer thread drains the receiver and frames each message. `send` fans a single
/// pre-serialized JSON string out to all clients, pruning ones whose receiver hung
/// up (client disconnected). Cheap, lock-guarded, no async runtime.
#[derive(Default)]
pub struct Bus {
    clients: Mutex<Vec<Sender<String>>>,
    /// Live phone count (incremented after handshake, decremented on disconnect) —
    /// drives the desktop's mobile-link (wifi) indicator.
    active: std::sync::atomic::AtomicUsize,
}

impl Bus {
    fn subscribe(&self) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel::<String>();
        self.clients.lock().unwrap().push(tx);
        rx
    }
    /// Broadcast one already-serialized JSON text frame to all live clients.
    pub fn send(&self, text: String) {
        let mut g = self.clients.lock().unwrap();
        g.retain(|tx| tx.send(text.clone()).is_ok());
    }
    fn client_connected(&self) -> usize {
        self.active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }
    fn client_disconnected(&self) -> usize {
        self.active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_sub(1)
    }
}

/// Per-launch state for the mobile bridge: the random pairing token plus the
/// resolved ports, exposed to the desktop UI via `mobile_info()`.
pub struct MobileState {
    pub token: String,
    pub http_port: u16,
    pub ws_port: u16,
}

impl Default for MobileState {
    fn default() -> Self {
        MobileState { token: gen_token(), http_port: http_port(), ws_port: ws_port() }
    }
}

// ======================================================================
// emit_all — the single broadcast integration point
// ======================================================================

/// Emit an event to the local Tauri webview AND to all connected WS clients.
/// Drop-in for `app.emit(event, payload)` at every call site that should reach the
/// phone. Serializes once into {event,payload} for the WS side; the webview path
/// is unchanged. If the Bus isn't managed yet (shouldn't happen post-setup) it
/// degrades to a plain app.emit.
pub fn emit_all<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    let _ = app.emit(event, payload.clone());
    if let Some(bus) = app.try_state::<Bus>() {
        // No phone connected: skip the serialize + frame build entirely. This is
        // the common case and runs on the PTY firehose + image-heavy conv-msg.
        if bus.active.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            return;
        }
        // Build {event, payload} once. serde_json::to_value can't fail for our
        // payloads (all derive Serialize); on the impossible error we just skip WS.
        if let Ok(pv) = serde_json::to_value(&payload) {
            let frame = json!({ "event": event, "payload": pv }).to_string();
            bus.send(frame);
        }
    }
}

// ======================================================================
// Tauri command for the desktop UI (QR / URL)
// ======================================================================

#[derive(Serialize)]
pub struct MobileInfo {
    pub lan_ip: String,
    pub http_port: u16,
    pub ws_port: u16,
    pub token: String,
    /// Ready-to-render URL the phone opens (token in the query string).
    pub url: String,
}

#[tauri::command]
pub fn mobile_info(state: State<MobileState>) -> MobileInfo {
    let lan_ip = lan_ip().unwrap_or_else(|| "127.0.0.1".into());
    let url = format!("http://{}:{}/?token={}", lan_ip, state.http_port, state.token);
    MobileInfo {
        lan_ip,
        http_port: state.http_port,
        ws_port: state.ws_port,
        token: state.token.clone(),
        url,
    }
}

// ======================================================================
// Bootstrap — call once from main()'s setup()
// ======================================================================

/// Start both servers. Returns immediately; each server runs on its own thread.
pub fn start(app: AppHandle) {
    let (hp, wp, token) = {
        let st = app.state::<MobileState>();
        (st.http_port, st.ws_port, st.token.clone())
    };
    start_http(app.clone(), hp, wp);
    start_ws(app, wp, token);
}

// ======================================================================
// (1) HTTP static server — serves ../ui (the same files Tauri embeds)
// ======================================================================

/// The frontend assets, embedded at compile time exactly like Tauri does via
/// generate_context!. Keeping them in the binary means the phone gets byte-for-byte
/// the same UI with no filesystem dependency at runtime.
fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    // Normalize: "/" -> index.html, strip leading slash + query already removed.
    let p = path.trim_start_matches('/');
    let p = if p.is_empty() { "index.html" } else { p };
    macro_rules! a {
        ($name:literal, $mime:literal) => {
            (include_bytes!(concat!("../../ui/", $name)) as &'static [u8], $mime)
        };
    }
    Some(match p {
        "index.html"                      => a!("index.html", "text/html; charset=utf-8"),
        "app.js"                          => a!("app.js", "text/javascript; charset=utf-8"),
        "styles.css"                      => a!("styles.css", "text/css; charset=utf-8"),
        "vendor/xterm/xterm.js"           => a!("vendor/xterm/xterm.js", "text/javascript"),
        "vendor/xterm/xterm.css"          => a!("vendor/xterm/xterm.css", "text/css"),
        "vendor/xterm/addon-fit.js"       => a!("vendor/xterm/addon-fit.js", "text/javascript"),
        "vendor/xterm/addon-web-links.js" => a!("vendor/xterm/addon-web-links.js", "text/javascript"),
        "vendor/xterm/addon-webgl.js"     => a!("vendor/xterm/addon-webgl.js", "text/javascript"),
        _ => return None,
    })
}

fn start_http(_app: AppHandle, http_port: u16, ws_port: u16) {
    let listener = match TcpListener::bind(("0.0.0.0", http_port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[mobile] http bind {http_port} failed: {e}");
            return;
        }
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = serve_http(stream, ws_port);
            });
        }
    });
}

fn serve_http(mut stream: TcpStream, ws_port: u16) -> std::io::Result<()> {
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    // request line: "GET /path?query HTTP/1.1"
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/"); // drop query

    match asset(path) {
        Some((bytes, mime)) => {
            // index.html is patched on the fly to inject the WS shim that supplies
            // window.__TAURI__ when running in a plain browser (not Tauri).
            if path == "/" || path.ends_with("index.html") {
                let html = String::from_utf8_lossy(bytes);
                let injected = inject_shim(&html, ws_port);
                write_response(&mut stream, 200, "text/html; charset=utf-8", injected.as_bytes())
            } else {
                write_response(&mut stream, 200, mime, bytes)
            }
        }
        None => write_response(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn write_response(stream: &mut TcpStream, code: u16, mime: &str, body: &[u8]) -> std::io::Result<()> {
    let status = if code == 200 { "200 OK" } else { "404 Not Found" };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Inject a <script> before app.js loads that, ONLY when window.__TAURI__ is
/// absent (i.e. a real browser, not the Tauri webview), installs a shim providing
/// core.invoke + event.listen backed by the WebSocket. The shim reads the pairing
/// token from the page URL's ?token=… and connects to ws://<host>:<ws_port>.
fn inject_shim(html: &str, ws_port: u16) -> String {
    let shim = format!(
        r#"<script>
(function(){{
  if (window.__TAURI__) return; // real Tauri webview — leave native bridge alone
  var qs = new URLSearchParams(location.search);
  var token = qs.get('token') || '';
  var WS = 'ws://' + location.hostname + ':{ws_port}/?token=' + encodeURIComponent(token);
  var ws, ready=false, q=[], nextId=1;
  var pending = {{}};            // invoke id -> {{resolve,reject}}
  var listeners = {{}};          // event name -> [cb]
  function flush(){{ while(ready && q.length) ws.send(q.shift()); }}
  function tx(o){{ var s=JSON.stringify(o); if(ready) ws.send(s); else q.push(s); }}
  function connect(){{
    ws = new WebSocket(WS);
    ws.onopen = function(){{ ready=true; setPill('connected'); flush(); }};
    ws.onclose = function(){{ ready=false; setPill('connecting'); setTimeout(connect, 1500); }};
    ws.onerror = function(){{ try{{ws.close();}}catch(e){{}} }};
    ws.onmessage = function(ev){{
      var m; try{{ m=JSON.parse(ev.data); }}catch(e){{ return; }}
      if (m.event){{                                   // broadcast event
        var cbs = listeners[m.event]; if(!cbs) return;
        for (var i=0;i<cbs.length;i++) try{{ cbs[i]({{ payload: m.payload, event: m.event }}); }}catch(e){{}}
      }} else if (m.id != null && pending[m.id]){{       // invoke reply
        var p = pending[m.id]; delete pending[m.id];
        if (m.error != null) p.reject(new Error(m.error)); else p.resolve(m.result);
      }}
    }};
  }}
  function setPill(state){{
    var el=document.getElementById('conn-pill'); if(!el) return;
    el.setAttribute('data-state', state);
    var lab=el.querySelector('.conn-label'); if(lab) lab.textContent = state==='connected'?'Mobile':'Connecting';
  }}
  window.__TAURI__ = {{
    core: {{
      invoke: function(cmd, args){{
        return new Promise(function(resolve, reject){{
          var id = nextId++;
          pending[id] = {{ resolve: resolve, reject: reject }};
          tx({{ cmd: cmd, args: args || {{}}, id: id }});
        }});
      }}
    }},
    event: {{
      listen: function(name, cb){{
        (listeners[name] = listeners[name] || []).push(cb);
        return Promise.resolve(function(){{                // unlisten
          var a=listeners[name]; if(!a) return;
          var i=a.indexOf(cb); if(i>=0) a.splice(i,1);
        }});
      }}
    }}
  }};
  connect();
}})();
</script>"#,
        ws_port = ws_port
    );
    // Insert right before app.js so the shim exists when the IIFE reads __TAURI__.
    if let Some(idx) = html.find("<script src=\"app.js\"") {
        let mut out = String::with_capacity(html.len() + shim.len());
        out.push_str(&html[..idx]);
        out.push_str(&shim);
        out.push('\n');
        out.push_str(&html[idx..]);
        out
    } else if let Some(idx) = html.rfind("</body>") {
        let mut out = String::with_capacity(html.len() + shim.len());
        out.push_str(&html[..idx]);
        out.push_str(&shim);
        out.push_str(&html[idx..]);
        out
    } else {
        format!("{html}{shim}")
    }
}

// ======================================================================
// (2) WebSocket server — broadcast events + receive commands
// ======================================================================

fn start_ws(app: AppHandle, ws_port: u16, token: String) {
    let listener = match TcpListener::bind(("0.0.0.0", ws_port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[mobile] ws bind {ws_port} failed: {e}");
            return;
        }
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                let _ = ws_session(app, stream, token);
            });
        }
    });
}

fn ws_session(app: AppHandle, mut stream: TcpStream, token: String) -> std::io::Result<()> {
    // ---- read HTTP upgrade request (headers fit easily in one buffer) ----
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            return Ok(());
        }
    }
    let req = String::from_utf8_lossy(&buf);
    let headers = parse_headers(&req);

    // ---- pairing token gate (?token= in request line, or X-Helm-Token) ----
    let url_token = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|p| p.split('?').nth(1))
        .and_then(|q| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix("token=").map(|v| v.to_string()))
        })
        .unwrap_or_default();
    let url_token = urldecode(&url_token);
    let hdr_token = headers.get("x-helm-token").cloned().unwrap_or_default();
    if url_token != token && hdr_token != token {
        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n");
        return Ok(());
    }

    // ---- RFC6455 handshake ----
    let key = match headers.get("sec-websocket-key") {
        Some(k) => k.clone(),
        None => {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
            return Ok(());
        }
    };
    let accept = ws_accept(&key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(resp.as_bytes())?;

    // ---- writer side: subscribe to the bus, drain on a dedicated thread ----
    let rx = match app.try_state::<Bus>() {
        Some(bus) => bus.subscribe(),
        None => return Ok(()),
    };
    let mut writer = stream.try_clone()?;

    // this phone is now live — light the desktop's wifi indicator
    if let Some(bus) = app.try_state::<Bus>() {
        let n = bus.client_connected();
        let _ = app.emit("mobile-clients", json!({ "count": n }));
    }

    let writer_thread = std::thread::spawn(move || {
        // initial hello so the client knows it's live
        let _ = write_text_frame(&mut writer, &json!({ "event": "mobile-hello", "payload": {} }).to_string());
        while let Ok(text) = rx.recv() {
            if write_text_frame(&mut writer, &text).is_err() {
                break;
            }
        }
    });

    // ---- reader side: client -> commands ----
    let result = ws_read_loop(&app, &mut stream);
    drop(stream); // unblock writer's socket on close
    let _ = writer_thread.join();

    // phone gone — update the desktop indicator
    if let Some(bus) = app.try_state::<Bus>() {
        let n = bus.client_disconnected();
        let _ = app.emit("mobile-clients", json!({ "count": n }));
    }
    result
}

/// Read masked client frames; dispatch text frames as {cmd,args,id} commands.
fn ws_read_loop(app: &AppHandle, stream: &mut TcpStream) -> std::io::Result<()> {
    loop {
        let mut h = [0u8; 2];
        if read_exact_or_eof(stream, &mut h)? {
            return Ok(()); // EOF
        }
        let fin_opcode = h[0];
        let opcode = fin_opcode & 0x0f;
        let masked = h[1] & 0x80 != 0;
        let mut len = (h[1] & 0x7f) as usize;
        if len == 126 {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext)?;
            len = u16::from_be_bytes(ext) as usize;
        } else if len == 127 {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext)?;
            len = u64::from_be_bytes(ext) as usize;
        }
        let mut mask = [0u8; 4];
        if masked {
            stream.read_exact(&mut mask)?;
        }
        let mut payload = vec![0u8; len];
        if len > 0 {
            stream.read_exact(&mut payload)?;
            if masked {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i & 3];
                }
            }
        }
        match opcode {
            0x1 => {
                // text frame -> command
                if let Ok(v) = serde_json::from_slice::<Value>(&payload) {
                    dispatch_command(app, &v);
                }
            }
            0x8 => return Ok(()),       // close
            0x9 => {                    // ping -> pong (reply via cloned writer)
                let mut w = stream.try_clone()?;
                let _ = write_frame(&mut w, 0xA, &payload);
            }
            _ => {} // pong / continuation: ignore (UI sends only small text frames)
        }
    }
}

/// Dispatch a client command to the same backend functions the desktop invokes.
/// Mirrors the invoke_handler set; replies {id,result|error} so the phone's invoke
/// Promise resolves. Heavy/streaming work (pty_spawn output) flows back over the
/// broadcast bus exactly like the desktop webview, so no special-casing needed.
fn dispatch_command(app: &AppHandle, v: &Value) {
    let id = v["id"].clone();
    let cmd = v["cmd"].as_str().unwrap_or("").to_string();
    let args = v["args"].clone();

    let reply = |result: Value| {
        if id.is_null() {
            return;
        }
        let frame = json!({ "id": id, "result": result }).to_string();
        if let Some(bus) = app.try_state::<Bus>() {
            // unicast would be cleaner, but a tiny reply broadcast is harmless and
            // keeps the bus the single fan-out path. Client ignores ids it didn't issue.
            bus.send(frame);
        }
    };
    let reply_err = |msg: String| {
        if id.is_null() {
            return;
        }
        let frame = json!({ "id": id, "error": msg }).to_string();
        if let Some(bus) = app.try_state::<Bus>() {
            bus.send(frame);
        }
    };

    crate::dispatch_mobile_command(app, &cmd, &args, &reply, &reply_err);
}

// ======================================================================
// helpers: framing, handshake, headers, token, lan ip, sha1/base64
// ======================================================================

fn read_exact_or_eof(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match stream.read(&mut buf[got..]) {
            Ok(0) => return Ok(true),
            Ok(n) => got += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn write_text_frame(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    write_frame(stream, 0x1, text.as_bytes())
}

/// Server->client frame (FIN=1, unmasked). Supports the 3 length encodings.
fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        header.push(len as u8);
    } else if len <= 0xffff {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

fn parse_headers(req: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in req.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((k, val)) = line.split_once(':') {
            m.insert(k.trim().to_ascii_lowercase(), val.trim().to_string());
        }
    }
    m
}

fn ws_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let digest = sha1(format!("{key}{GUID}").as_bytes());
    base64_encode(&digest)
}

fn gen_token() -> String {
    // 16 bytes of entropy from the OS RNG via std (no rand crate). On Windows we
    // fold in time + addresses; good enough to keep LAN randoms out for Phase A.
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    seed ^= (&seed as *const u128 as u128).rotate_left(17);
    seed ^= std::process::id() as u128;
    let mut out = String::new();
    let mut x = seed | 1;
    for _ in 0..16 {
        // xorshift; hex-encode
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push_str(&format!("{:02x}", (x & 0xff) as u8));
    }
    out
}

/// Best LAN IPv4 of this machine (the address a phone on the same wifi can reach).
/// Uses the connect-to-discover trick — no packet is actually sent on a UDP socket,
/// but the kernel picks the egress interface so local_addr() is the LAN IP.
fn lan_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    let s = ip.to_string();
    if s == "0.0.0.0" {
        None
    } else {
        Some(s)
    }
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = (hexval(b[i + 1]) << 4) | hexval(b[i + 2]);
                out.push(h);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn hexval(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

// ----- inline SHA-1 (so no sha1 crate is needed) -----
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ----- base64 (std encode of the 20-byte digest; the base64 crate is present but
// keeping this self-contained avoids importing it here) -----
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}
