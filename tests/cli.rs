//! End-to-end tests: run the real `codegraph` binary on a throwaway project.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(args)
        .arg("--dir")
        .arg(dir)
        .output()
        .expect("binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() {\n    alpha();\n}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/app.ts"),
        "export function show() {\n  return beta();\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn index_writes_codemap_and_graph() {
    let dir = project();
    let out = run(dir.path(), &["index"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "indexed 2 files (2 parsed, 0 cached)");
    let map = fs::read_to_string(dir.path().join(".claude/codemap.md")).unwrap();
    assert!(map.contains("## src/lib.rs (4 lines)"));
    assert!(map.contains("src/lib.rs:2 pub fn beta()"));
    assert!(dir.path().join(".claude/graph.json").is_file());

    let again = run(dir.path(), &["index"]);
    assert_eq!(
        stdout(&again).trim(),
        "indexed 2 files (0 parsed, 2 cached)"
    );
}

#[test]
fn def_and_callers_print_one_result_per_line() {
    let dir = project();
    run(dir.path(), &["index"]);
    assert_eq!(
        stdout(&run(dir.path(), &["def", "alpha"])),
        "src/lib.rs:1 pub fn alpha()\n"
    );
    assert_eq!(
        stdout(&run(dir.path(), &["callers", "alpha"])),
        "src/lib.rs:3  in pub fn beta\n"
    );
}

#[test]
fn json_flag_prints_json() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = run(dir.path(), &["def", "alpha", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value[0]["path"], "src/lib.rs");
    assert_eq!(value[0]["line"], 1);
}

#[test]
fn path_prints_one_hop_per_line() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = run(dir.path(), &["path", "show", "alpha"]);
    assert_eq!(
        stdout(&out),
        "show  src/app.ts:1\nbeta  src/lib.rs:2\nalpha  src/lib.rs:1\n"
    );
    assert_eq!(
        stdout(&run(dir.path(), &["path", "alpha", "show"])).trim(),
        "no path"
    );
}

#[test]
fn update_reindexes_one_file() {
    let dir = project();
    run(dir.path(), &["index"]);
    fs::write(dir.path().join("src/lib.rs"), "pub fn gamma() {}\n").unwrap();
    let file = dir.path().join("src/lib.rs");
    let out = run(dir.path(), &["update", file.to_str().unwrap()]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&run(dir.path(), &["def", "gamma"])),
        "src/lib.rs:1 pub fn gamma()\n"
    );
    assert_eq!(stdout(&run(dir.path(), &["def", "alpha"])), "");
}

#[test]
fn query_without_index_fails_with_hint() {
    let dir = project();
    let out = run(dir.path(), &["def", "alpha"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("codegraph index"));
}

#[test]
fn update_accepts_paths_relative_to_dir_and_deleted_files() {
    let dir = project();
    run(dir.path(), &["index"]);
    fs::create_dir_all(dir.path().join("web")).unwrap();
    fs::write(
        dir.path().join("web/new.ts"),
        "export function fresh() {}\n",
    )
    .unwrap();
    // `web/` doesn't exist in the test's cwd, so the path is taken from the project root.
    assert!(run(dir.path(), &["update", "web/new.ts"]).status.success());
    assert_eq!(
        stdout(&run(dir.path(), &["def", "fresh"])),
        "web/new.ts:1 export function fresh()\n"
    );

    fs::remove_file(dir.path().join("web/new.ts")).unwrap();
    assert!(run(dir.path(), &["update", "web/new.ts"]).status.success());
    assert_eq!(stdout(&run(dir.path(), &["def", "fresh"])), "");
}

#[test]
fn explain_affected_and_hubs_print_compact_lines() {
    let dir = project();
    run(dir.path(), &["index"]);
    let explain = stdout(&run(dir.path(), &["explain", "beta"]));
    assert_eq!(
        explain,
        "def src/lib.rs:2 pub fn beta()\n<- show  src/app.ts:1 (1 site)\n-> alpha  src/lib.rs:1 (1 site)\n"
    );
    let affected = stdout(&run(dir.path(), &["affected", "alpha"]));
    assert_eq!(
        affected,
        "beta  src/lib.rs:2  depth 1\nshow  src/app.ts:1  depth 2\n"
    );
    let hubs = stdout(&run(dir.path(), &["hubs", "--top", "1"]));
    assert_eq!(hubs, "beta  src/lib.rs:2  in=1 out=1\n");
    let json = run(dir.path(), &["explain", "beta", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["defs"][0]["line"], 2);
}

/// Kills the child process when dropped, so a failing test never leaves `watch` running.
struct KillOnDrop(std::process::Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn affected_depth_and_json_flags() {
    let dir = project();
    run(dir.path(), &["index"]);
    let near = stdout(&run(dir.path(), &["affected", "alpha", "--depth", "1"]));
    assert_eq!(near, "beta  src/lib.rs:2  depth 1\n");
    let json = run(dir.path(), &["affected", "alpha", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value[1]["name"], "show");
    assert_eq!(value[1]["depth"], 2);
    let hubs = run(dir.path(), &["hubs", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&hubs.stdout).unwrap();
    assert_eq!(value[0]["name"], "beta");
}

#[test]
fn watch_reindexes_added_modified_and_deleted_files() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let dir = project();
    let child = Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["watch", "--interval-ms", "50", "--dir"])
        .arg(dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .expect("binary runs");
    let mut child = KillOnDrop(child);
    // A reader thread keeps the pipe open and lets the test wait with a timeout.
    let pipe = child.0.stdout.take().expect("piped stdout");
    let (lines, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            let _ = lines.send(line);
        }
    });
    let first = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("watch prints a first line");
    assert!(first.starts_with("watching 2 files"), "got: {first}");

    let def_is = |name: &str, want: &str| stdout(&run(dir.path(), &["def", name])) == want;
    let wait_for = |what: &str, done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "watch never saw: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    let file = dir.path().join("src/new.rs");
    fs::write(&file, "pub fn fresh() {}\n").unwrap();
    wait_for("added file", &|| {
        def_is("fresh", "src/new.rs:1 pub fn fresh()\n")
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        "updated src/new.rs"
    );

    // The extra newline changes the size, so the change is seen even if the mtime tick is coarse.
    fs::write(&file, "pub fn renamed() {}\n\n").unwrap();
    wait_for("modified file", &|| {
        def_is("renamed", "src/new.rs:1 pub fn renamed()\n")
    });

    fs::remove_file(&file).unwrap();
    wait_for("deleted file", &|| def_is("renamed", ""));
}

#[test]
fn report_writes_graph_report_and_json_lists_communities() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = run(dir.path(), &["report"]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        "wrote .claude/GRAPH_REPORT.md (1 community)"
    );
    let report = fs::read_to_string(dir.path().join(".claude/GRAPH_REPORT.md")).unwrap();
    assert!(report.contains("## Key modules"));
    assert!(report.contains("`src/app.ts` - 1 links"));

    let json = stdout(&run(dir.path(), &["report", "--json"]));
    assert!(
        json.contains(r#""files":["src/app.ts","src/lib.rs"]"#),
        "{json}"
    );
}

#[test]
fn html_writes_a_self_contained_page() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = run(dir.path(), &["html"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "wrote .claude/graph.html (2 files)");
    let page = fs::read_to_string(dir.path().join(".claude/graph.html")).unwrap();
    assert!(page.contains("const DATA = {"));
    assert!(page.contains(r#""path":"src/lib.rs""#));
    assert!(!page.contains("http://") && !page.contains("https://"));
}

#[test]
fn html_without_index_fails_with_hint() {
    let dir = project();
    let out = run(dir.path(), &["html"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("codegraph index"));
}

#[test]
fn report_without_index_fails_with_hint() {
    let dir = project();
    let out = run(dir.path(), &["report"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("codegraph index"));
}

#[test]
fn benchmark_without_graphify_reports_codegraph_only() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["benchmark", "--name", "beta", "--dir"])
        .arg(dir.path())
        .env("PATH", "")
        .output()
        .expect("binary runs");
    assert!(out.status.success());
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "files 2");
    assert!(lines[1].starts_with("index  codegraph "), "{text}");
    assert_eq!(lines[2], "graphify: not found");
    assert!(lines[3].starts_with("explain beta  codegraph "), "{text}");
    assert!(lines[3].ends_with(" ms ok"), "{text}");
    assert!(!dir.path().join("graphify-out").exists());
}

#[test]
fn mermaid_prints_flowchart_or_no_calls() {
    let dir = project();
    run(dir.path(), &["index"]);
    let chart = stdout(&run(dir.path(), &["mermaid", "beta"]));
    assert_eq!(
        chart,
        "flowchart LR\n    n0[\"beta\"]\n    n1[\"alpha\"]\n    n0 --> n1\n"
    );
    assert_eq!(
        stdout(&run(dir.path(), &["mermaid", "alpha"])),
        "no calls\n"
    );
}

#[test]
fn mermaid_rejects_depth_zero() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = run(dir.path(), &["mermaid", "beta", "--depth", "0"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--depth"));
}

#[test]
fn obsidian_writes_vault_notes() {
    let dir = project();
    run(dir.path(), &["index"]);
    let out = run(dir.path(), &["obsidian"]);
    assert!(out.status.success());
    let vault = dir.path().join(".claude/obsidian");
    assert!(vault.join("src/lib.rs.md").is_file());
    assert!(vault.join("src/app.ts.md").is_file());
    assert!(
        stdout(&out).starts_with("wrote 3 notes to "),
        "{}",
        stdout(&out)
    );
}

#[test]
fn mixed_language_project_indexes_and_answers_callers() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        (
            "app/main.py",
            "from .util import helper\ndef run():\n    helper()\n",
        ),
        ("app/util.py", "def helper():\n    pass\n"),
        ("Store.java", "class Store { void add() { fmt(); } }\n"),
        ("Fmt.cs", "class Fmt { void fmt() { } }\n"),
        (
            "main.c",
            "#include \"util.h\"\nint main(void) { return util(); }\n",
        ),
        ("util.h", "int util(void);\n"),
        (
            "web.js",
            "const { x } = require('./lib');\nfunction go() { x(); }\n",
        ),
        ("lib.js", "function x() {}\n"),
        (
            "srv.go",
            "package s\nfunc Serve() { Handle() }\nfunc Handle() {}\n",
        ),
    ];
    for (path, src) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, src).unwrap();
    }
    let out = run(dir.path(), &["index"]);
    assert!(
        stdout(&out).starts_with("indexed 9 files"),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        stdout(&run(dir.path(), &["callers", "helper"])),
        "app/main.py:3  in run\n"
    );
    assert_eq!(
        stdout(&run(dir.path(), &["callers", "Handle"])),
        "srv.go:2  in Serve\n"
    );
    assert!(stdout(&run(dir.path(), &["def", "main"])).contains("main.c:2"));
}

/// A SCIP index with one document whose occurrences are (0-based line, symbol, is definition).
fn scip_bytes(path: &str, occurrences: &[(u64, &str, bool)]) -> Vec<u8> {
    fn field(number: u8, data: &[u8]) -> Vec<u8> {
        [vec![number << 3 | 2, data.len() as u8], data.to_vec()].concat()
    }
    let mut doc = field(1, path.as_bytes());
    for (line, symbol, definition) in occurrences {
        let range = field(1, &[*line as u8, 0, 5]);
        let roles = vec![3 << 3, u8::from(*definition)];
        doc.extend(field(
            2,
            &[range, field(2, symbol.as_bytes()), roles].concat(),
        ));
    }
    field(2, &doc)
}

#[test]
fn scip_resolves_calls_and_reports() {
    let dir = project();
    run(dir.path(), &["index"]);
    // src/lib.rs line 3 (0-based 2) calls alpha, defined on line 1.
    let data = scip_bytes(
        "src/lib.rs",
        &[
            (0, "r c p 1 lib/alpha().", true),
            (2, "r c p 1 lib/alpha().", false),
        ],
    );
    fs::write(dir.path().join("index.scip"), data).unwrap();
    let out = run(dir.path(), &["scip"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out), "resolved 1 of 1 calls (0 external)\n");

    let json = stdout(&run(dir.path(), &["scip", "--json"]));
    assert_eq!(
        json.trim(),
        r#"{"calls":1,"resolved":1,"external":0,"unknown_documents":0}"#
    );
    // A reference with no definition in the project is the standard library or a dependency.
    let external = scip_bytes("src/lib.rs", &[(2, "r c core 1 lib/alpha().", false)]);
    fs::write(dir.path().join("index.scip"), external).unwrap();
    run(dir.path(), &["scip"]);
    assert_eq!(stdout(&run(dir.path(), &["callers", "alpha"])), "");
    let bad = run(dir.path(), &["scip", "missing.scip"]);
    assert!(!bad.status.success());
}
