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
fn def_flag_picks_one_definition_or_fails_with_the_choices() {
    let dir = project();
    run(dir.path(), &["index"]);
    let ok = run(dir.path(), &["callers", "alpha", "--def", "src/lib.rs:1"]);
    assert_eq!(stdout(&ok), "src/lib.rs:3  in pub fn beta\n");
    let explained = run(dir.path(), &["explain", "alpha", "--def", "src/lib.rs"]);
    assert!(stdout(&explained).contains("<- beta"));
    let bad = run(dir.path(), &["callers", "alpha", "--def", "nowhere.rs"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("src/lib.rs:1"));
}

#[test]
fn index_applies_saved_scip_data_again_after_it_parses() {
    let dir = project();
    let sym = "rust-analyzer cargo p 1 m/alpha().";
    fs::write(
        dir.path().join("index.scip"),
        scip_bytes("src/lib.rs", &[(0, sym, true), (2, sym, false)]),
    )
    .unwrap();
    let first = stdout(&run(dir.path(), &["index"]));
    assert!(
        first.contains("scip: re-applied 1 saved file(s)"),
        "{first}"
    );
    // Nothing was parsed this time, so there is nothing to bring back.
    let again = stdout(&run(dir.path(), &["index"]));
    assert!(!again.contains("scip:"), "{again}");
    let graph = fs::read_to_string(dir.path().join(".claude/graph.json")).unwrap();
    assert!(
        graph.contains("\"resolved\""),
        "the answer survives in the saved graph"
    );
}

/// `codegraph scip --run` with `path` as the only directory on PATH besides the system ones.
fn run_scip_with_path(dir: &Path, path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["scip", "--run", "--dir"])
        .arg(dir)
        .env("PATH", format!("{}:/usr/bin:/bin", path.display()))
        .output()
        .expect("binary runs")
}

#[test]
fn scip_run_starts_the_indexer_for_the_project_and_applies_its_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project();
    fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    run(dir.path(), &["index"]);
    let sym = "rust-analyzer cargo p 1 m/alpha().";
    let fake = tempfile::tempdir().unwrap();
    let canned = fake.path().join("canned.scip");
    fs::write(
        &canned,
        scip_bytes("src/lib.rs", &[(0, sym, true), (2, sym, false)]),
    )
    .unwrap();
    // A stand-in for `rust-analyzer scip . --output FILE`: copies the canned index to FILE.
    let script = fake.path().join("rust-analyzer");
    fs::write(
        &script,
        format!("#!/bin/sh\ncp '{}' \"$4\"\n", canned.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let out = run_scip_with_path(dir.path(), fake.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join(".claude/scip/rust.scip").is_file());
    assert!(
        stdout(&out).starts_with("resolved 1 of 1 calls"),
        "{}",
        stdout(&out)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("rust: indexed with rust-analyzer"));
}

#[test]
fn scip_run_says_what_it_needs_when_nothing_applies() {
    let dir = project();
    run(dir.path(), &["index"]);
    let empty = tempfile::tempdir().unwrap();
    let none = run_scip_with_path(dir.path(), empty.path());
    assert!(!none.status.success());
    assert!(String::from_utf8_lossy(&none.stderr).contains("Cargo.toml (rust-analyzer)"));
    // A marker without its program installed is reported, and nothing is written.
    fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    let missing = run_scip_with_path(dir.path(), empty.path());
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("rust-analyzer is not installed"));
}

#[test]
fn scip_run_reports_a_failing_indexer_and_keeps_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project();
    fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    run(dir.path(), &["index"]);
    let fake = tempfile::tempdir().unwrap();
    let script = fake.path().join("rust-analyzer");
    fs::write(&script, "#!/bin/sh\necho 'no cargo here' >&2\nexit 3\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let out = run_scip_with_path(dir.path(), fake.path());
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("rust-analyzer failed") && err.contains("no cargo here"),
        "{err}"
    );
    assert!(!dir.path().join(".claude/scip/rust.scip").exists());
}

#[test]
fn scip_run_works_with_a_relative_dir() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("proj");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(proj.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    fs::write(proj.join("Cargo.toml"), "[package]\n").unwrap();
    run(&proj, &["index"]);
    let fake = tempfile::tempdir().unwrap();
    let script = fake.path().join("rust-analyzer");
    // Writes to the --output it is given, relative to wherever it runs.
    fs::write(&script, "#!/bin/sh\nprintf '' > \"$4\"\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["scip", "--run", "--dir", "proj"])
        .current_dir(parent.path())
        .env("PATH", format!("{}:/usr/bin:/bin", fake.path().display()))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("rust: indexed with rust-analyzer"), "{err}");
    assert!(proj.join(".claude/scip/rust.scip").is_file());
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
        "def src/lib.rs:2 pub fn beta()\n<- show  src/app.ts:1\n-> alpha  src/lib.rs:1\n"
    );
    let full = stdout(&run(dir.path(), &["explain", "beta", "--mode", "full"]));
    assert_eq!(
        full,
        "def src/lib.rs:2 pub fn beta()\n<- show  src/app.ts:1 (1 site)\n-> alpha  src/lib.rs:1 (1 site)\n"
    );
    let callees = stdout(&run(dir.path(), &["explain", "beta", "--mode", "callees"]));
    assert_eq!(callees, "-> alpha  src/lib.rs:1 (1 site)\n");
    assert!(
        !run(dir.path(), &["explain", "beta", "--mode", "nope"])
            .status
            .success()
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
    // src/app.ts also calls something, but the index has no document for it.
    assert_eq!(
        stdout(&out),
        "resolved 1 of 1 calls (0 external); no index document for 1 file (1 calls): they keep name matching\n"
    );

    let json = stdout(&run(dir.path(), &["scip", "--json"]));
    assert_eq!(
        json.trim(),
        r#"{"calls":1,"resolved":1,"external":0,"unknown_documents":0,"uncovered_files":1,"uncovered_calls":1}"#
    );
    // A reference with no definition in the project is the standard library or a dependency.
    let external = scip_bytes("src/lib.rs", &[(2, "r c core 1 lib/alpha().", false)]);
    fs::write(dir.path().join("index.scip"), external).unwrap();
    run(dir.path(), &["scip"]);
    assert_eq!(stdout(&run(dir.path(), &["callers", "alpha"])), "");
    let bad = run(dir.path(), &["scip", "missing.scip"]);
    assert!(!bad.status.success());
}

#[test]
fn closed_stdout_ends_quietly() {
    // ~150 KB of output: more than a pipe holds, so the child is still writing when we hang up.
    let dir = tempfile::tempdir().unwrap();
    let src: String = (0..4000)
        .map(|i| format!("pub fn f{i}() {{ f{}(); }}\n", i + 1))
        .collect();
    fs::write(dir.path().join("lib.rs"), src).unwrap();
    run(dir.path(), &["index"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["hubs", "--top", "4000", "--dir"])
        .arg(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn serve_speaks_mcp_over_stdin_and_stdout() {
    use std::io::Write;
    let dir = project();
    run(dir.path(), &["index"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["serve", "--dir"])
        .arg(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"callers","arguments":{"name":"alpha"}}}"#,
    ];
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin); // the client hangs up: the server ends by itself
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let replies: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 3, "the notification gets no reply");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "codegraph");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 10);
    assert_eq!(
        replies[2]["result"]["content"][0]["text"],
        "src/lib.rs:3  in pub fn beta"
    );
}

#[test]
fn hook_prints_one_json_line_then_stays_within_budget() {
    use std::io::Write;
    let dir = project();
    run(dir.path(), &["index"]);
    let state = tempfile::tempdir().unwrap();
    let hook = |input: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_codegraph"))
            .args(["hook", "search"])
            .env("CLAUDE_PROJECT_DIR", dir.path())
            .env("XDG_RUNTIME_DIR", state.path())
            .env("CODEGRAPH_HOOK_NUDGES", "2")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "a hook never fails");
        assert!(out.stderr.is_empty());
        stdout(&out)
    };
    let grep = r#"{"session_id":"abc","tool_name":"Grep","tool_input":{"pattern":"alpha"}}"#;
    let first = hook(grep);
    let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert!(hook(grep).starts_with('{'));
    assert_eq!(hook(grep), "", "budget of 2 used up");
    assert_eq!(hook("garbage"), "");
    assert_eq!(
        hook(r#"{"session_id":"x","tool_input":{"command":"cargo build"}}"#),
        ""
    );
}

#[test]
fn install_and_uninstall_wire_one_project() {
    let dir = project();
    let shown = stdout(&run(dir.path(), &["install", "--print"]));
    assert!(shown.contains("codegraph hook search") && shown.contains("\"serve\""));
    assert!(!dir.path().join(".mcp.json").exists());

    let out = run(dir.path(), &["install"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mcp = fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
    assert!(mcp.contains("\"codegraph\"") && mcp.contains("\"serve\""));
    let settings = fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
    assert!(settings.contains("codegraph hook read"));
    assert!(
        dir.path()
            .join(".claude/skills/codegraph/SKILL.md")
            .is_file()
    );

    assert!(run(dir.path(), &["uninstall"]).status.success());
    let settings = fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
    assert!(!settings.contains("codegraph hook"));
    assert!(
        !dir.path()
            .join(".claude/skills/codegraph/SKILL.md")
            .exists()
    );
}

#[test]
fn hook_ignores_an_empty_project_dir_variable() {
    use std::io::Write;
    let project = project();
    run(project.path(), &["index"]);
    let elsewhere = tempfile::tempdir().unwrap(); // no index here: only --dir points at the project
    let state = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_codegraph"))
        .args(["hook", "search", "--dir"])
        .arg(project.path())
        .current_dir(elsewhere.path())
        .env("CLAUDE_PROJECT_DIR", "")
        .env("XDG_RUNTIME_DIR", state.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let grep = r#"{"session_id":"e","tool_input":{"pattern":"alpha"}}"#;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(grep.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        stdout(&out).contains("PreToolUse"),
        "fell back to --dir: {}",
        stdout(&out)
    );
}

#[test]
fn docs_pending_then_ingest_then_nothing_pending() {
    let dir = project();
    fs::write(dir.path().join("README.md"), "# Title\nUses `alpha`.\n").unwrap();
    assert!(run(dir.path(), &["index"]).status.success());

    let pending = stdout(&run(dir.path(), &["docs", "pending"]));
    assert!(pending.contains("README.md"), "{pending}");
    assert!(pending.contains("inline"), "{pending}");

    let json = stdout(&run(dir.path(), &["--json", "docs", "pending"]));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let hash = v["docs"][0]["hash"].as_str().unwrap();
    let sub = dir.path().join("sub.json");
    fs::write(
        &sub,
        format!(
            r#"{{"file":"README.md","hash":"{hash}","concepts":[],"links":[{{"line":2,"to":"alpha","rel":"describes","conf":"INFERRED"}}]}}"#
        ),
    )
    .unwrap();
    let out = run(dir.path(), &["docs", "ingest", sub.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout(&out).contains("1 links"), "{}", stdout(&out));
    assert!(stdout(&run(dir.path(), &["docs", "pending"])).contains("0 docs"));

    fs::write(dir.path().join("README.md"), "# Title\nchanged\n").unwrap();
    let stale = run(dir.path(), &["docs", "ingest", sub.to_str().unwrap()]);
    assert!(!stale.status.success(), "a stale hash must be refused");
}

#[cfg(unix)]
#[test]
fn docs_ingest_refuses_a_symlinked_doc() {
    let dir = project();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("real.md"), "# Outside\n").unwrap();
    std::os::unix::fs::symlink(outside.path().join("real.md"), dir.path().join("link.md")).unwrap();
    assert!(run(dir.path(), &["index"]).status.success());
    assert!(!stdout(&run(dir.path(), &["docs", "pending"])).contains("link.md"));

    let hash = blake3::hash(b"# Outside\n").to_hex().to_string();
    let sub = dir.path().join("sub.json");
    fs::write(
        &sub,
        format!(
            r#"{{"file":"link.md","hash":"{}","concepts":[],"links":[]}}"#,
            &hash[..16]
        ),
    )
    .unwrap();
    let out = run(dir.path(), &["docs", "ingest", sub.to_str().unwrap()]);
    assert!(!out.status.success(), "a symlink must not be ingested");
}

#[test]
fn docs_text_prints_listed_docs_only() {
    let dir = project();
    fs::write(dir.path().join("README.md"), "# Title\nbody\n").unwrap();
    let ok = run(dir.path(), &["docs", "text", "README.md"]);
    assert_eq!(stdout(&ok), "# Title\nbody\n");
    let no = run(dir.path(), &["docs", "text", "src/lib.rs"]);
    assert!(!no.status.success());
}

#[test]
fn docs_text_on_a_damaged_pdf_fails_fast() {
    let dir = project();
    fs::write(dir.path().join("bad.pdf"), b"%PDF-1.4 garbage").unwrap();
    let out = run(dir.path(), &["docs", "text", "bad.pdf"]);
    assert!(!out.status.success());
    assert!(stdout(&run(dir.path(), &["docs", "pending"])).contains("bad.pdf  0 lines"));
}

#[test]
fn query_answers_a_plain_english_question() {
    let dir = project();
    assert!(run(dir.path(), &["index"]).status.success());
    let out = stdout(&run(dir.path(), &["query", "who calls alpha"]));
    assert!(out.starts_with("start: alpha"), "{out}");
    assert!(
        out.contains("beta -> alpha") || out.contains("beta"),
        "{out}"
    );
    let none = stdout(&run(dir.path(), &["query", "zzzz"]));
    assert_eq!(none.trim(), "no matching symbols");
    let json = stdout(&run(dir.path(), &["--json", "query", "alpha"]));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["seeds"][0], "alpha");
    assert!(
        !run(dir.path(), &["query", "alpha", "--depth", "9"])
            .status
            .success()
    );
}

#[test]
fn tour_lists_a_reading_order() {
    let dir = project();
    assert!(run(dir.path(), &["index"]).status.success());
    let out = stdout(&run(dir.path(), &["tour", "--top", "3"]));
    assert!(out.starts_with("1. "), "{out}");
    assert!(out.lines().count() <= 3, "{out}");
    let json = stdout(&run(dir.path(), &["--json", "tour"]));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v[0]["path"].is_string());
}

#[test]
fn teach_adds_plain_language_lines_only_when_asked() {
    let dir = project();
    assert!(run(dir.path(), &["index"]).status.success());
    let plain = stdout(&run(dir.path(), &["def", "alpha"]));
    assert!(!plain.contains("terms:"), "{plain}");
    let taught = stdout(&run(dir.path(), &["def", "alpha", "--teach"]));
    assert!(
        taught.contains("  terms: pub = visible outside its module; fn = a function"),
        "{taught}"
    );
    assert!(
        taught.starts_with(plain.trim_end()),
        "plain line comes first"
    );
    let explained = stdout(&run(dir.path(), &["explain", "beta", "--teach"]));
    assert!(
        explained.contains("terms:") && explained.contains("read next: alpha"),
        "{explained}"
    );
}

#[test]
fn teach_changes_nothing_in_json_output() {
    let dir = project();
    assert!(run(dir.path(), &["index"]).status.success());
    let plain = stdout(&run(dir.path(), &["--json", "def", "alpha"]));
    let taught = stdout(&run(dir.path(), &["--json", "def", "alpha", "--teach"]));
    assert_eq!(plain, taught);
}
