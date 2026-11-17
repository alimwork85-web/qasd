//! `fsearch mcp`: a Model Context Protocol server on stdin/stdout for coding
//! agents (Claude Code, Codex, ...): JSON-RPC 2.0, one message per line. The
//! tool is fff-mcp's find_files, so prompts written for it work here. Each
//! call is a request or two to the shared daemon (started if need be), so
//! every agent shares one live index; answers are terse text: paths
//! relative to the project, git status where it isn't clean.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Protocol versions spoken, newest first. A client asking for another one
/// gets the newest and decides for itself.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// How long a call waits for a first index build before giving up.
const WAIT_INDEX: Duration = Duration::from_secs(30);

/// The most daemon requests one tool call makes (the most fallbacks of an
/// unlucky query).
const MAX_ASKS: usize = 16;

const INSTRUCTIONS: &str = "qasd: indexed search over this project (or any folder like C:\\work, or every drive with path \"/\").
- find_files: file names (fuzzy, typo-tolerant), to locate a file or see what exists.
Filters go inline: *.rs or *.{ts,tsx}, src/ (in a folder), src/**/*.rs (glob), !test/ or !*.md (exclude), \
git:modified|staged|untracked (git status). Results are best first: read the top file instead of searching again.";

pub fn run(dir: PathBuf) {
    let mut s = Session::new(dir);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) if batch.is_empty() => Some(error(&Value::Null, -32600, "invalid request: an empty batch")),
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(|m| s.message(m)).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(m) => s.message(&m),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        for m in reply.into_iter().chain(s.outbox.drain(..)) {
            if writeln!(out, "{m}").and_then(|_| out.flush()).is_err() {
                return;
            }
        }
    }
}

struct Session {
    dir: PathBuf,
    home: String,
    /// The project: the first MCP root, else the git repo holding the
    /// working directory, else the working directory.
    root: String,
    /// Messages to send after the current reply (requests to the client).
    outbox: Vec<Value>,
    asked_roots: u32,
    /// Daemon requests made for the current tool call (see `spend`).
    asks: std::cell::Cell<usize>,
}

fn error(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

fn result(id: &Value, r: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": r})
}

impl Session {
    fn new(dir: PathBuf) -> Session {
        // Paths as the index holds them: /C:/Users/me.
        let home = qasd::sys::home();
        let cwd = std::env::current_dir().map(|p| String::from_utf8_lossy(&qasd::sys::bytes(p)).into_owned()).unwrap_or_else(|_| home.clone());
        let real = |p: String| String::from_utf8_lossy(&qasd::sys::real(p.as_bytes())).into_owned();
        let root = real(repo_root(&cwd, &home).unwrap_or(cwd));
        Session { dir, home, root, outbox: Vec::new(), asked_roots: 0, asks: Default::default() }
    }

    /// The reply to one message, if it needs one.
    fn message(&mut self, m: &Value) -> Option<Value> {
        let id = m.get("id");
        let Some(method) = m.get("method").and_then(Value::as_str) else {
            if m.get("result").is_none() && m.get("error").is_none() {
                return Some(error(id.unwrap_or(&Value::Null), -32600, "invalid request: no method"));
            }
            // A reply to one of our requests (roots/list).
            if let Some(roots) = m.pointer("/result/roots").and_then(Value::as_array)
                && let Some(p) = roots.iter().find_map(|r| r["uri"].as_str().and_then(file_uri))
            {
                self.root = String::from_utf8_lossy(&qasd::sys::real(p.as_bytes())).into_owned();
            }
            return None;
        };
        let params = m.get("params").unwrap_or(&Value::Null);
        let Some(id) = id else {
            match method {
                "notifications/initialized" | "notifications/roots/list_changed" if self.asked_roots > 0 => self.ask_roots(),
                _ => {}
            }
            return None;
        };
        Some(match method {
            "initialize" => {
                let want = params["protocolVersion"].as_str().unwrap_or("");
                let version = VERSIONS.iter().find(|v| **v == want).unwrap_or(&VERSIONS[0]);
                if params.pointer("/capabilities/roots").is_some() {
                    // Asked once the client says it's ready (notifications/initialized).
                    self.asked_roots = 1;
                }
                self.warm();
                result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "qasd", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({ "tools": tools() })),
            "tools/call" => {
                let args = params.get("arguments").unwrap_or(&Value::Null);
                self.asks.set(0);
                let r = match params["name"].as_str().unwrap_or("") {
                    "find_files" => self.find_files(args),
                    "find_apps" => find_apps(args),
                    other => return Some(error(id, -32602, &format!("unknown tool {other}"))),
                };
                let (text, is_error) = match r {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                result(id, json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
            }
            _ => error(id, -32601, &format!("method not found: {method}")),
        })
    }

    /// Have the daemon up and its index loading while the agent thinks.
    /// (The project's git status is read on the first search that uses it:
    /// many sessions never search.)
    fn warm(&self) {
        let dir = self.dir.clone();
        std::thread::spawn(move || ask_at(&dir, &json!({"op": "ping"})).ok());
    }

    fn ask_roots(&mut self) {
        self.asked_roots += 1;
        self.outbox.push(json!({"jsonrpc": "2.0", "id": format!("qasd-roots-{}", self.asked_roots), "method": "roots/list"}));
    }

    fn ask(&self, req: &Value) -> Result<Value, String> {
        self.spend(1)?;
        ask_at(&self.dir, req)
    }

    /// Count `n` more daemon requests for this tool call: fallbacks stop
    /// when MAX_ASKS run out.
    fn spend(&self, n: usize) -> Result<(), String> {
        let used = self.asks.get() + n;
        if used > MAX_ASKS {
            return Err(format!("stopped after {MAX_ASKS} searches for one call"));
        }
        self.asks.set(used);
        Ok(())
    }

    /// The folder a call searches (None: the whole disk), from its `path`.
    fn scope(&self, a: &Value) -> Option<String> {
        let p = a["path"].as_str().map(str::trim).filter(|p| !p.is_empty() && *p != ".").unwrap_or(&self.root);
        // A Windows path (C:\x, C:/x) in the index's form; `~\x` as `~/x`.
        let fixed;
        let p = if p.len() >= 2 && p.as_bytes()[1] == b':' {
            fixed = String::from_utf8_lossy(&qasd::sys::from_user(p)).into_owned();
            fixed.as_str()
        } else if p.contains('\\') {
            fixed = p.replace('\\', "/");
            fixed.as_str()
        } else {
            p
        };
        Some(match p {
            "/" => return None,
            p if p.starts_with('/') => p.trim_end_matches('/').to_string(),
            p if p == "~" || p.starts_with("~/") => format!("{}{}", self.home, &p[1..]).trim_end_matches('/').to_string(),
            p => format!("{}/{}", self.root, p.trim_start_matches("./").trim_end_matches('/')),
        })
    }

    /// How this call shows paths: relative to the project, or to the
    /// folder searched when that's elsewhere (said once, up top).
    fn view(&self, scope: &Option<String>) -> View {
        match scope {
            Some(s) if !within(s, &self.root) => View { base: s.clone(), head: Some(format!("in {}/:", win(s))) },
            _ => View { base: self.root.clone(), head: None },
        }
    }

    fn find_files(&self, a: &Value) -> Result<String, String> {
        let query = arg_str(a, &["query", "pattern"]).unwrap_or_default();
        let max = arg_num(a, "maxResults", 20);
        let offset = cursor(a)?;
        let scope = self.scope(a);
        if query.trim().is_empty() || query.trim() == "*" {
            return self.worked_on(&scope, max);
        }
        let mut p = split(&query);
        // A lone `name/`: the folders called that.
        let dirs = p.words.is_empty() && p.globs.len() == 1 && p.globs[0].ends_with('/') && !p.globs[0].starts_with('!');
        if dirs {
            let d = p.globs.pop().unwrap();
            p.words.push(d.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string());
        }
        let req = |words: &[String], hide: bool| {
            let mut r = json!({
                "q": words.iter().chain(&p.filters).cloned().collect::<Vec<_>>().join(" "),
                "limit": max, "offset": offset, "git": true, "boost": true,
            });
            // What git ignores isn't the project's, unless asked for.
            if hide && !query.contains(".git") {
                r["ignored"] = false.into();
            }
            if !p.filters.iter().any(|f| f.starts_with("kind:")) {
                r["kind"] = (if dirs { "dir" } else { "file" }).into();
            }
            if let Some(s) = &scope {
                r["in"] = s.as_str().into();
            }
            // A repo's own files aren't the project's (unless asked for):
            // git status leaves them out of a repo, this out of a folder of repos.
            let mut globs = p.globs.clone();
            if !query.contains(".git") && (!hide || !Path::new(&self.root).join(".git").exists()) {
                globs.push("!.git/".into());
            }
            if !globs.is_empty() {
                r["glob"] = json!(globs);
            }
            r
        };
        let v = self.view(&scope);
        let r = self.ask(&req(&p.words, true))?;
        let hits = r["hits"].as_array().cloned().unwrap_or_default();
        if hits.is_empty() && offset > 0 {
            return Ok(format!("no more files past cursor {offset}"));
        }
        if !hits.is_empty() {
            return Ok(v.list_files(&hits, &p.words, r["next"].as_u64(), None));
        }
        // A short word with two letters swapped ("mian"): the index forgives
        // typos only from five letters.
        for (i, w) in p.words.iter().enumerate().filter(|(_, w)| (3..5).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_alphanumeric())) {
            for j in 0..w.len() - 1 {
                let mut t = w.clone().into_bytes();
                t.swap(j, j + 1);
                let mut words = p.words.clone();
                words[i] = String::from_utf8(t).unwrap_or_default();
                if words[i] == *w {
                    continue;
                }
                let Ok(r) = self.ask(&req(&words, true)) else { break };
                let hits = r["hits"].as_array().cloned().unwrap_or_default();
                if !hits.is_empty() {
                    return Ok(v.list_files(&hits, &words, r["next"].as_u64(), Some(format!("0 files match '{w}'; as '{}':", words[i]))));
                }
            }
        }
        // Nothing the project tracks: maybe in what git ignores (build output).
        let r = self.ask(&req(&p.words, false)).unwrap_or_default();
        let hits = r["hits"].as_array().cloned().unwrap_or_default();
        if !hits.is_empty() {
            return Ok(v.list_files(&hits, &p.words, r["next"].as_u64(), Some("0 matches outside git-ignored files; ignored:".into())));
        }
        // Nothing: drop one word at a time (the last first).
        if p.words.len() > 1 {
            for i in (0..p.words.len()).rev() {
                let fewer: Vec<String> = p.words.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, w)| w.clone()).collect();
                let Ok(r) = self.ask(&req(&fewer, true)) else { break };
                let hits = r["hits"].as_array().cloned().unwrap_or_default();
                if !hits.is_empty() {
                    let note = format!("0 files match all of '{}'; without '{}':", p.words.join(" "), p.words[i]);
                    return Ok(v.list_files(&hits, &fewer, r["next"].as_u64(), Some(note)));
                }
            }
        }
        Ok(format!("0 files match '{query}' in {}", scope.as_deref().unwrap_or("the whole disk")))
    }

    /// No query: what git says is changed, then what you worked on lately
    /// (opened, committed).
    fn worked_on(&self, scope: &Option<String>, max: usize) -> Result<String, String> {
        let mut changed = json!({"q": "git:modified", "kind": "file", "limit": max, "git": true, "boost": true});
        let mut recent = json!({"op": "recent", "kind": "file", "limit": max, "git": true});
        if let Some(s) = scope {
            (changed["in"], recent["in"]) = (s.as_str().into(), s.as_str().into());
        }
        let mut hits = self.ask(&changed).ok().and_then(|r| r["hits"].as_array().cloned()).unwrap_or_default();
        hits.truncate(max);
        for h in self.ask(&recent)?["hits"].as_array().cloned().unwrap_or_default() {
            if hits.len() < max && !hits.iter().any(|o| o["path"] == h["path"]) {
                hits.push(h);
            }
        }
        if hits.is_empty() {
            return Ok("nothing changed in git or worked on lately here; give a query".into());
        }
        Ok(self.view(scope).list_files(&hits, &[], None, Some("changed, then worked on lately:".into())))
    }
}

/// How a call shows paths (see `Session::view`).
struct View {
    base: String,
    head: Option<String>,
}

impl View {
    fn show<'a>(&self, path: &'a str) -> &'a str {
        match path.strip_prefix(self.base.as_str()) {
            Some(rest) if rest.starts_with('/') => &rest[1..],
            // Elsewhere: as Windows takes it, C:/x/y (the index's /C:/x/y).
            _ => win(path),
        }
    }

    fn list_files(&self, hits: &[Value], words: &[String], next: Option<u64>, note: Option<String>) -> String {
        // A path given (`include/linux/sched.h`): the file at it first.
        let at = |h: &&Value| {
            let p = self.show(h["path"].as_str().unwrap_or(""));
            words.iter().any(|w| w.contains('/') && (p == w.trim_start_matches("./") || p.ends_with(&format!("/{}", w.trim_start_matches("./")))))
        };
        let mut hits: Vec<&Value> = hits.iter().collect();
        hits.sort_by_key(|h| !at(h));
        // Weak: the best hit doesn't spell every word out (typos, scattered letters).
        let spelled = |h: &Value| {
            let p = h["path"].as_str().unwrap_or("").to_lowercase();
            words.iter().all(|w| p.contains(&w.to_lowercase()))
        };
        let weak = note.is_none() && !words.is_empty() && hits.first().is_some_and(|h| !spelled(h));
        let mut out: Vec<String> = self.head.iter().cloned().chain(note).collect();
        if weak {
            out.push(format!("no exact match for '{}'; closest:", words.join(" ")));
        }
        for h in hits {
            let path = self.show(h["path"].as_str().unwrap_or(""));
            let dir = if h["kind"] == "dir" { "/" } else { "" };
            let git = h["git"].as_str().map_or(String::new(), |g| format!(" ({g})"));
            out.push(format!("{path}{dir}{git}"));
        }
        if let Some(n) = next {
            out.push(format!("cursor: {n}"));
        }
        out.join("\n")
    }
}

/// One request to the daemon. A connection each, its write side closed once
/// the request is out: the daemon then knows no keystroke follows and
/// doesn't keep the cores spinning for one (agents think for seconds).
fn ask_at(dir: &Path, req: &Value) -> Result<Value, String> {
    let t = Instant::now();
    loop {
        let mut s = crate::server::connect(dir).map_err(|e| format!("cannot reach the qasd daemon: {e}"))?;
        s.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("daemon: {e}"))?;
        if v["ok"] == true {
            return Ok(v);
        }
        let e = v["error"].as_str().unwrap_or("error");
        if !e.starts_with("indexing") || t.elapsed() > WAIT_INDEX {
            return Err(e.to_string());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn tools() -> Value {
    let common = |extra: Value| {
        let mut p = json!({
            "path": {"type": "string", "description": "Folder to search: relative to the project, absolute (C:\\Users\\me\\app), or \"/\" for every drive. Default: the project."},
            "maxResults": {"type": "number", "description": "Default 20."},
            "cursor": {"type": "string", "description": "From the previous result, for the next page."},
        });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    let ro = json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false});
    json!([
        {
            "name": "find_files",
            "description": "Find files by name: fuzzy and typo-tolerant, over the whole path. Use it to locate a file or see which files exist. All words must match. Inline filters: *.rs, src/, src/**/*.rs, !test/, git:modified. A lone 'name/' finds folders; an empty query lists files changed in git, then those worked on lately.",
            "inputSchema": {"type": "object", "properties": common(json!({
                "query": {"type": "string", "description": "Name words and filters, e.g. 'config *.ts !test/'."},
            })), "required": ["query"]},
            "annotations": ro,
        },
        {
            "name": "find_apps",
            "description": "Find installed apps (the Windows Start menu's, Store and desktop) by name, fuzzy and typo-tolerant. Each result gives a target that opens the app: explorer.exe \"shell:AppsFolder\\<id>\". An empty query lists them all.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "Name words, e.g. 'calculator' or 'الحاسبة'."},
                "maxResults": {"type": "number", "description": "Default 10."},
            }, "required": ["query"]},
            "annotations": ro,
        },
    ])
}

/// The `find_apps` tool: one line per app, `name<TAB>shell:AppsFolder\<id>`.
fn find_apps(a: &Value) -> Result<String, String> {
    let q = arg_str(a, &["query"]).unwrap_or_default();
    let max = arg_num(a, "maxResults", 10);
    let all = qasd::apps::list();
    let found: Vec<&qasd::apps::App> = if q.trim().is_empty() { all.iter().take(max).collect() } else { qasd::apps::find(&all, &q, max) };
    if found.is_empty() {
        return Ok(format!("no app matches '{q}'"));
    }
    Ok(found.iter().map(|x| format!("{}	{}", x.name, x.target())).collect::<Vec<_>>().join("
"))
}

/// The first of `keys` given as a string.
fn arg_str(a: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| a[*k].as_str()).map(str::to_string)
}

/// A count argument (models send 20, 20.0 or "20"); `default` when absent or not positive.
fn arg_num(a: &Value, key: &str, default: usize) -> usize {
    let v = &a[key];
    let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(-1.0);
    if n.is_finite() && n >= 0.5 { (n.round() as usize).min(1000) } else { default }
}

/// The page offset a cursor carries (cursors are offsets: no state to lose).
fn cursor(a: &Value) -> Result<usize, String> {
    match &a["cursor"] {
        Value::Null => Ok(0),
        Value::Number(n) => Ok(n.as_u64().unwrap_or(0) as usize),
        Value::String(s) if s.trim().is_empty() => Ok(0),
        Value::String(s) => s.trim().parse().map_err(|_| format!("bad cursor {s}")),
        other => Err(format!("bad cursor {other}")),
    }
}

/// A tool query taken apart: name words,
/// fsearch filters, and path globs for the daemon.
#[derive(Default)]
struct Parts {
    words: Vec<String>,
    filters: Vec<String>,
    globs: Vec<String>,
}

/// fsearch filters a query may carry as they are.
const FILTERS: &[&str] = &["ext", "type", "kind", "in", "size", "mtime", "modified", "re", "path", "git", "limit"];

fn split(query: &str) -> Parts {
    let mut p = Parts::default();
    for w in words(query) {
        let (neg, body) = match w.strip_prefix('!') {
            Some(b) if !b.is_empty() => (true, b),
            _ => (false, w.as_str()),
        };
        if let Some((k, v)) = body.split_once(':')
            && !v.is_empty()
            && FILTERS.contains(&k)
        {
            p.filters.push(w.clone());
        } else if let Some(exts) = ext_glob(body) {
            if neg { p.globs.push(w.clone()) } else { p.filters.push(format!("ext:{exts}")) }
        } else if is_glob(body) || (body.len() > 1 && body.ends_with('/')) {
            p.globs.push(w.clone());
        } else if neg {
            // `!word`: leave out paths below the project with it in a name.
            p.globs.push(format!("!*{body}*/"));
        } else {
            p.words.push(w);
        }
    }
    p
}

/// Words, with "double quoted" runs kept whole.
fn words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `*.rs`, `**/*.rs`, `*.{ts,tsx}`: the extensions, as `ext:` takes them.
fn ext_glob(w: &str) -> Option<String> {
    let rest = w.strip_prefix("**/").unwrap_or(w).strip_prefix("*.")?;
    let list = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(rest);
    let ok = !list.is_empty() && list.split(',').all(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    ok.then(|| list.to_string())
}

/// A path glob (rather than code that happens to hold a `*`): wildcards
/// with a `/` or braces, or `*word*`.
fn is_glob(w: &str) -> bool {
    let wild = w.contains(['*', '?', '[']) || (w.contains('{') && w.contains(','));
    wild && (w.contains('/')
        || (w.contains('{') && w.contains('}'))
        || (w.len() > 2 && w.starts_with('*') && w.ends_with('*') && !w[1..w.len() - 1].contains('*')))
}

/// An index path (`/C:/x`) as Windows takes it (`C:/x`).
fn win(path: &str) -> &str {
    path.strip_prefix('/').filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(path)
}

/// Is `path` `dir` or inside it?
fn within(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir).is_some_and(|r| r.is_empty() || r.starts_with('/'))
}

/// The repo holding `dir` (its top folder), unless that's the home folder
/// itself or `/` (a dotfiles repo isn't a project).
fn repo_root(dir: &str, home: &str) -> Option<String> {
    let mut d = dir;
    loop {
        if qasd::sys::os(format!("{d}/.git").as_bytes()).exists() {
            return (d != home && d != "/").then(|| d.to_string());
        }
        d = &d[..d.rfind('/').filter(|&i| i > 0)?];
    }
}

/// The path of a `file://` URI.
fn file_uri(u: &str) -> Option<String> {
    let p = u.strip_prefix("file://")?;
    let p = p.strip_prefix("localhost").unwrap_or(p);
    let b = p.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = p.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    Some(s.trim_end_matches('/').to_string()).filter(|s| s.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_queries() {
        let p = split("*.rs src/ !test/ parse config");
        assert_eq!(p.filters, ["ext:rs"]);
        assert_eq!(p.globs, ["src/", "!test/"]);
        assert_eq!(p.words, ["parse", "config"]);
        assert_eq!(split("*.{ts,tsx} git:modified button").filters, ["ext:ts,tsx", "git:modified"]);
    }
}
