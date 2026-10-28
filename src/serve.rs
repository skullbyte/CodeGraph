//! MCP server over stdio: lets Claude Code call codegraph's queries as tools.
//! MCP is JSON-RPC 2.0, one JSON message per line. Design after Graphify's `serve.py` (tools) with
//! hand-written protocol code, so there is no extra dependency. stdout carries protocol messages
//! only; the rest goes to stderr.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde_json::{Value, json};

use crate::community::{self, Community};
use crate::index;
use crate::model::Index;
use crate::query;

/// A longer line (line ending included) is refused: a client cannot make the server buffer
/// unbounded input. JSON-RPC batches (an array) are not supported; 2025-06-18 removed them.
const MAX_LINE: usize = 1 << 20;
/// Result lines returned when the caller gives no `limit`; the rest is summarised.
const DEFAULT_LIMIT: usize = 40;
const MAX_LIMIT: usize = 200;
const MAX_ARG_LEN: usize = 512;
/// Newest first; a client asking for an older one gets it back, an unknown one gets the newest.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub struct Server {
    root: PathBuf,
    loaded: Option<Loaded>,
}

/// The index in memory, with the `graph.bin` timestamp it was read at.
struct Loaded {
    stamp: Option<SystemTime>,
    index: Index,
    communities: Option<Vec<Community>>,
    /// The top `MAX_LIMIT` hubs, computed once per index version.
    hubs: Option<Vec<query::Hub>>,
}

impl Server {
    pub fn new(root: &Path) -> Self {
        Server {
            root: root.to_path_buf(),
            loaded: None,
        }
    }

    /// The reply to one incoming line, or `None` when none is due (notifications, responses).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return Some(error(Value::Null, -32700, "parse error"));
        };
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A reply to a request of ours (we send none) is ignored; anything else is malformed.
            let is_response = msg.get("result").is_some() || msg.get("error").is_some();
            return (!is_response)
                .then(|| error(id.unwrap_or(Value::Null), -32600, "invalid request"));
        };
        let id = id?; // no id: a notification, never answered
        // MCP ids are strings or integers, never null.
        if !(id.is_string() || id.is_i64() || id.is_u64()) {
            return Some(error(Value::Null, -32600, "invalid request id"));
        }
        let params = msg.get("params").unwrap_or(&Value::Null);
        Some(match method {
            "initialize" => reply(id, initialize(params)),
            "ping" => reply(id, json!({})),
            "tools/list" => reply(id, json!({ "tools": tools() })),
            "tools/call" => self.call(id, params),
            _ => error(id, -32601, "method not found"),
        })
    }

    fn call(&mut self, id: Value, params: &Value) -> String {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error(id, -32602, "missing tool name");
        };
        if !TOOL_NAMES.contains(&name) {
            return error(id, -32602, "unknown tool");
        }
        let args = params.get("arguments").unwrap_or(&Value::Null);
        let (text, is_error) = match self.run_tool(name, args) {
            Ok(text) => (text, false),
            Err(message) => (message, true),
        };
        reply(
            id,
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
        )
    }

    fn run_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        let limit = number(args, "limit", DEFAULT_LIMIT)?;
        let loaded = self.loaded()?;
        let index = &loaded.index;
        let lines: Vec<String> = match name {
            "def" => query::def(index, text(args, "name")?)
                .iter()
                .map(query::DefHit::text)
                .collect(),
            "callers" => query::callers(index, text(args, "name")?)
                .iter()
                .map(query::CallerHit::text)
                .collect(),
            "explain" => query::explain(index, text(args, "name")?).lines(),
            "path" => query::path(index, text(args, "from")?, text(args, "to")?)
                .iter()
                .map(query::Hop::text)
                .collect(),
            "affected" => {
                let depth = args
                    .get("depth")
                    .map(|_| number(args, "depth", 0))
                    .transpose()?;
                query::affected(index, text(args, "name")?, depth)
                    .iter()
                    .map(query::Affected::text)
                    .collect()
            }
            "hubs" => hubs(loaded, number(args, "top", 10)?),
            "community" => {
                let file = text(args, "file")?.to_string();
                community_of(loaded, &file)
            }
            _ => stats(loaded),
        };
        Ok(capped(lines, limit))
    }

    /// The index, re-read when `graph.bin` changed since; built first if there is none yet.
    fn loaded(&mut self) -> Result<&mut Loaded, String> {
        let stamp = self.stamp();
        let current = self
            .loaded
            .as_ref()
            .is_some_and(|l| stamp.is_some() && l.stamp == stamp);
        if !current {
            // The stamp read before loading, so a write that lands during the load is seen next time.
            let (index, stamp) = match index::load(&self.root) {
                Some(index) => (index, stamp),
                None => (self.build()?, self.stamp()),
            };
            self.loaded = Some(Loaded {
                stamp,
                index,
                communities: None,
                hubs: None,
            });
        }
        self.loaded
            .as_mut()
            .ok_or_else(|| "index unavailable".to_string())
    }

    fn stamp(&self) -> Option<SystemTime> {
        fs::metadata(self.root.join(".claude").join("graph.bin"))
            .and_then(|m| m.modified())
            .ok()
    }

    fn build(&self) -> Result<Index, String> {
        let fail = |e: anyhow::Error| format!("indexing {}: {e:#}", self.root.display());
        let _lock = index::lock(&self.root).map_err(fail)?;
        // Another process (or a hook) may have finished the same work while we waited.
        if let Some(index) = index::load(&self.root) {
            return Ok(index);
        }
        let (index, _) = index::build(&self.root, None).map_err(fail)?;
        index::save(&self.root, &index).map_err(fail)?;
        Ok(index)
    }
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOLS
        .iter()
        .find(|v| Some(**v) == asked)
        .unwrap_or(&PROTOCOLS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "codegraph", "version": env!("CARGO_PKG_VERSION") },
    })
}

const TOOL_NAMES: [&str; 8] = [
    "def",
    "callers",
    "explain",
    "path",
    "affected",
    "hubs",
    "community",
    "stats",
];

/// Descriptions are paid for in context every session, so they stay terse.
fn tools() -> Vec<Value> {
    let name =
        json!({ "name": { "type": "string", "description": "function, type or symbol name" } });
    let limit = json!({ "limit": { "type": "integer", "description": "max lines (default 40)" } });
    let with = |extra: &Value| {
        let mut props = limit.as_object().cloned().unwrap_or_default();
        props.extend(extra.as_object().cloned().unwrap_or_default());
        Value::Object(props)
    };
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "properties": props, "required": required },
        })
    };
    vec![
        tool(
            "def",
            "Where a symbol is defined (path:line signature).",
            with(&name),
            &["name"],
        ),
        tool(
            "callers",
            "Call sites of a function: path:line and calling function.",
            with(&name),
            &["name"],
        ),
        tool(
            "explain",
            "Definition, callers and callees of a name.",
            with(&name),
            &["name"],
        ),
        tool(
            "path",
            "Shortest call chain between two functions, or import chain between two files.",
            with(&json!({ "from": { "type": "string" }, "to": { "type": "string" } })),
            &["from", "to"],
        ),
        tool(
            "affected",
            "Everything that depends on a function (callers of callers).",
            with(&json!({
                "name": name["name"],
                "depth": { "type": "integer", "description": "hops to follow" },
            })),
            &["name"],
        ),
        tool(
            "hubs",
            "Most-connected functions.",
            with(&json!({ "top": { "type": "integer", "description": "how many (default 10)" } })),
            &[],
        ),
        tool(
            "community",
            "Files in the same cluster as a file.",
            with(&json!({ "file": { "type": "string", "description": "project-relative path" } })),
            &["file"],
        ),
        tool(
            "stats",
            "Index size: files, symbols, calls, SCIP-resolved calls, clusters.",
            limit,
            &[],
        ),
    ]
}

/// A required, bounded string argument.
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() && s.len() <= MAX_ARG_LEN => Ok(s.trim()),
        Some(_) => Err(format!(
            "argument `{key}` must be 1-{MAX_ARG_LEN} characters"
        )),
        None => Err(format!("missing string argument `{key}`")),
    }
}

/// An optional whole-number argument in `1..=MAX_LIMIT`.
fn number(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| {
                format!("argument `{key}` must be a whole number from 1 to {MAX_LIMIT}")
            }),
    }
}

/// `limit` lines, then how many were left out.
fn capped(mut lines: Vec<String>, limit: usize) -> String {
    if lines.is_empty() {
        return "no results".to_string();
    }
    let extra = lines.len().saturating_sub(limit);
    lines.truncate(limit);
    if extra > 0 {
        lines.push(format!("... +{extra} more"));
    }
    lines.join("\n")
}

fn hubs(loaded: &mut Loaded, top: usize) -> Vec<String> {
    let index = &loaded.index;
    let all = loaded
        .hubs
        .get_or_insert_with(|| query::hubs(index, MAX_LIMIT));
    all.iter().take(top).map(query::Hub::text).collect()
}

fn communities(loaded: &mut Loaded) -> &[Community] {
    let index = &loaded.index;
    loaded
        .communities
        .get_or_insert_with(|| community::detect(&community::file_graph(index)))
}

fn community_of(loaded: &mut Loaded, file: &str) -> Vec<String> {
    let found = communities(loaded)
        .iter()
        .find(|c| c.files.iter().any(|f| f == file));
    match found {
        Some(c) => {
            let mut lines = vec![format!(
                "community {} ({} files, cohesion {:.2})",
                c.id,
                c.files.len(),
                c.cohesion
            )];
            lines.extend(c.files.iter().cloned());
            lines
        }
        None => Vec::new(),
    }
}

fn stats(loaded: &mut Loaded) -> Vec<String> {
    let files = loaded.index.files.len();
    let symbols: usize = loaded.index.files.values().map(|f| f.symbols.len()).sum();
    let calls: usize = loaded.index.files.values().map(|f| f.calls.len()).sum();
    let resolved: usize = loaded.index.files.values().map(|f| f.resolved.len()).sum();
    let clusters = communities(loaded).len();
    vec![format!(
        "{files} files, {symbols} symbols, {calls} calls ({resolved} resolved by SCIP), {clusters} clusters"
    )]
}

fn reply(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error(id: Value, code: i32, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

enum Line {
    Eof,
    Text(String),
    TooLong,
}

/// One line, at most `MAX_LINE` bytes; a longer one is skipped to its end and reported.
fn read_line(reader: &mut impl BufRead) -> io::Result<Line> {
    let mut buf = Vec::new();
    let read = io::Read::take(&mut *reader, MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(Line::Eof);
    }
    if buf.len() > MAX_LINE && buf.last() != Some(&b'\n') {
        loop {
            let mut rest = Vec::new();
            let n = io::Read::take(&mut *reader, 64 * 1024).read_until(b'\n', &mut rest)?;
            if n == 0 || rest.last() == Some(&b'\n') {
                return Ok(Line::TooLong);
            }
        }
    }
    Ok(Line::Text(String::from_utf8_lossy(&buf).trim().to_string()))
}

/// Serve MCP on stdin/stdout until the client closes the pipe.
pub fn run(root: &Path) -> Result<()> {
    let mut server = Server::new(root);
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let reply = match read_line(&mut input)? {
            Line::Eof => return Ok(()),
            Line::TooLong => Some(error(Value::Null, -32600, "message too large")),
            Line::Text(line) if line.is_empty() => None,
            Line::Text(line) => server.handle_line(&line),
        };
        if let Some(reply) = reply {
            let sent = writeln!(output, "{reply}").and_then(|()| output.flush());
            match sent {
                Ok(()) => {}
                // The client hung up: a normal end, not a failure.
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn alpha() {}\npub fn beta() {\n    alpha();\n}\n",
        )
        .unwrap();
        let (idx, _) = index::build(dir.path(), None).unwrap();
        index::save(dir.path(), &idx).unwrap();
        dir
    }

    fn ask(server: &mut Server, method: &str, params: Value) -> Value {
        let request = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
        let reply = server.handle_line(&request.to_string()).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    fn call(server: &mut Server, tool: &str, args: Value) -> (String, bool) {
        let reply = ask(
            server,
            "tools/call",
            json!({ "name": tool, "arguments": args }),
        );
        let result = &reply["result"];
        (
            result["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            result["isError"].as_bool().unwrap(),
        )
    }

    #[test]
    fn initialize_echoes_known_versions_and_offers_the_newest_otherwise() {
        let mut server = Server::new(Path::new("."));
        let old = ask(
            &mut server,
            "initialize",
            json!({ "protocolVersion": "2024-11-05" }),
        );
        assert_eq!(old["id"], 7);
        assert_eq!(old["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(old["result"]["serverInfo"]["name"], "codegraph");
        assert!(old["result"]["capabilities"]["tools"].is_object());
        let odd = ask(
            &mut server,
            "initialize",
            json!({ "protocolVersion": "1999-01-01" }),
        );
        assert_eq!(odd["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(ask(&mut server, "ping", Value::Null)["result"], json!({}));
    }

    #[test]
    fn notifications_get_no_reply_and_bad_input_gets_an_error() {
        let mut server = Server::new(Path::new("."));
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(server.handle_line(note), None);
        let unknown_note = r#"{"jsonrpc":"2.0","method":"whatever"}"#;
        assert_eq!(server.handle_line(unknown_note), None);
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert_eq!(server.handle_line(response), None);
        let mut code = |line: &str| -> i64 {
            let reply = server.handle_line(line).unwrap();
            serde_json::from_str::<Value>(&reply).unwrap()["error"]["code"]
                .as_i64()
                .unwrap()
        };
        assert_eq!(code("not json"), -32700);
        assert_eq!(code(r#"{"id":1}"#), -32600);
        assert_eq!(code(r#"[1,2]"#), -32600);
        assert_eq!(code(r#"{"id":1,"method":"nope"}"#), -32601);
        assert_eq!(
            code(r#"{"id":1,"method":"tools/call","params":{"name":"nope"}}"#),
            -32602
        );
        assert_eq!(
            code(r#"{"id":1,"method":"tools/call","params":{}}"#),
            -32602
        );
    }

    #[test]
    fn tools_list_names_every_tool_and_stays_small() {
        let mut server = Server::new(Path::new("."));
        let listed = ask(&mut server, "tools/list", Value::Null);
        let tools = listed["result"]["tools"].as_array().unwrap();
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
        // Every session pays for this text: ~4 characters per token.
        assert!(
            listed.to_string().len() < 4500,
            "{} bytes",
            listed.to_string().len()
        );
    }

    #[test]
    fn tools_answer_like_the_cli() {
        let dir = project();
        let mut server = Server::new(dir.path());
        assert_eq!(
            call(&mut server, "def", json!({ "name": "alpha" })),
            ("src/lib.rs:1 pub fn alpha()".into(), false)
        );
        assert_eq!(
            call(&mut server, "callers", json!({ "name": "alpha" })).0,
            "src/lib.rs:3  in pub fn beta"
        );
        assert!(
            call(&mut server, "explain", json!({ "name": "alpha" }))
                .0
                .contains("<- beta")
        );
        assert!(
            call(
                &mut server,
                "path",
                json!({ "from": "beta", "to": "alpha" })
            )
            .0
            .contains("alpha")
        );
        assert!(
            call(
                &mut server,
                "affected",
                json!({ "name": "alpha", "depth": 1 })
            )
            .0
            .contains("beta")
        );
        assert!(call(&mut server, "hubs", json!({})).0.contains("alpha"));
        assert!(
            call(&mut server, "community", json!({ "file": "src/lib.rs" }))
                .0
                .contains("src/lib.rs")
        );
        assert!(
            call(&mut server, "stats", json!({}))
                .0
                .starts_with("1 files, 2 symbols, 1 calls")
        );
        assert_eq!(
            call(&mut server, "def", json!({ "name": "zzz" })),
            ("no results".into(), false)
        );
    }

    #[test]
    fn bad_arguments_are_tool_errors_not_crashes() {
        let dir = project();
        let mut server = Server::new(dir.path());
        for args in [
            json!({}),
            json!({ "name": 5 }),
            json!({ "name": "" }),
            json!({ "name": "x".repeat(MAX_ARG_LEN + 1) }),
            json!({ "name": "alpha", "limit": 0 }),
            json!({ "name": "alpha", "limit": 1_000_000 }),
            json!({ "name": "alpha", "limit": "ten" }),
            json!({ "name": "alpha", "limit": -3 }),
        ] {
            let (message, is_error) = call(&mut server, "callers", args);
            assert!(is_error, "{message}");
        }
    }

    #[test]
    fn long_answers_are_cut_with_a_count() {
        let dir = tempfile::tempdir().unwrap();
        let calls: String = (0..60)
            .map(|i| format!("pub fn f{i}() {{ target(); }}\n"))
            .collect();
        fs::write(
            dir.path().join("lib.rs"),
            format!("pub fn target() {{}}\n{calls}"),
        )
        .unwrap();
        let mut server = Server::new(dir.path());
        let (all, _) = call(&mut server, "callers", json!({ "name": "target" }));
        assert_eq!(all.lines().count(), DEFAULT_LIMIT + 1);
        assert!(all.ends_with("... +20 more"));
        let (few, _) = call(
            &mut server,
            "callers",
            json!({ "name": "target", "limit": 5 }),
        );
        assert_eq!(few.lines().count(), 6);
        assert!(few.ends_with("... +55 more"));
    }

    #[test]
    fn a_missing_index_is_built_and_a_changed_one_is_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn one() {}\n").unwrap();
        let mut server = Server::new(dir.path());
        // No .claude/ yet: the first call builds it.
        assert!(
            call(&mut server, "def", json!({ "name": "one" }))
                .0
                .contains("lib.rs:1")
        );
        assert!(dir.path().join(".claude/graph.bin").is_file());
        assert_eq!(
            call(&mut server, "def", json!({ "name": "two" })).0,
            "no results"
        );
        // Another process (the editor hook) updates the index; the server notices.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(
            dir.path().join("lib.rs"),
            "pub fn one() {}\npub fn two() {}\n",
        )
        .unwrap();
        let (mut idx, _) = index::build(dir.path(), None).unwrap();
        index::update_file(dir.path(), &mut idx, "lib.rs").unwrap();
        index::save(dir.path(), &idx).unwrap();
        assert!(
            call(&mut server, "def", json!({ "name": "two" }))
                .0
                .contains("lib.rs:2")
        );
    }

    #[test]
    fn ids_must_be_strings_or_integers() {
        let mut server = Server::new(Path::new("."));
        for id in ["null", "true", "1.5", "[1]", "{}"] {
            let reply = server
                .handle_line(&format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#))
                .unwrap();
            let reply: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(reply["error"]["code"], -32600, "{id}");
        }
        let ok = server
            .handle_line(r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#)
            .unwrap();
        assert!(ok.contains(r#""id":"abc""#));
    }

    #[test]
    fn a_broken_project_directory_is_a_tool_error_and_caches_follow_reloads() {
        let mut server = Server::new(Path::new("/nonexistent/for/codegraph/test"));
        let (message, is_error) = call(&mut server, "def", json!({ "name": "x" }));
        assert!(is_error && message.contains("indexing"), "{message}");

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn one() {}\n").unwrap();
        let mut server = Server::new(dir.path());
        assert!(
            call(&mut server, "stats", json!({}))
                .0
                .starts_with("1 files, 1 symbols")
        );
        let hubs_before = call(&mut server, "hubs", json!({})).0;
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.path().join("two.rs"), "pub fn two() { one(); }\n").unwrap();
        let (idx, _) = index::build(dir.path(), None).unwrap();
        index::save(dir.path(), &idx).unwrap();
        // The cached cluster count and hub list must not survive the reload.
        assert!(
            call(&mut server, "stats", json!({}))
                .0
                .starts_with("2 files, 2 symbols")
        );
        assert_ne!(call(&mut server, "hubs", json!({})).0, hubs_before);
    }

    #[test]
    fn crlf_and_invalid_utf8_lines_are_read() {
        let mut crlf = io::Cursor::new(b"{\"a\":1}\r\n".to_vec());
        assert!(matches!(read_line(&mut crlf).unwrap(), Line::Text(t) if t == "{\"a\":1}"));
        let mut bad = io::Cursor::new(b"\xff\xfe{\n".to_vec());
        assert!(matches!(read_line(&mut bad).unwrap(), Line::Text(_)));
        let mut server = Server::new(Path::new("."));
        // Replacement characters make it invalid JSON: a parse error, not a crash.
        let reply = server.handle_line("\u{fffd}{").unwrap();
        assert!(reply.contains("-32700"));
    }

    #[test]
    fn lines_are_bounded() {
        let mut ok = io::Cursor::new(b"{\"a\":1}\n".to_vec());
        assert!(matches!(read_line(&mut ok).unwrap(), Line::Text(t) if t == "{\"a\":1}"));
        assert!(matches!(read_line(&mut ok).unwrap(), Line::Eof));
        let mut big = vec![b'x'; MAX_LINE + 10];
        big.extend(b"\n{\"b\":2}\n");
        let mut cursor = io::Cursor::new(big);
        assert!(matches!(read_line(&mut cursor).unwrap(), Line::TooLong));
        assert!(matches!(read_line(&mut cursor).unwrap(), Line::Text(t) if t == "{\"b\":2}"));
        // A line of exactly MAX_LINE bytes plus its newline is still allowed.
        let mut exact = vec![b'y'; MAX_LINE];
        exact.push(b'\n');
        assert!(
            matches!(read_line(&mut io::Cursor::new(exact)).unwrap(), Line::Text(t) if t.len() == MAX_LINE)
        );
    }
}
