//! Claude Code `PreToolUse` hook: when the agent is about to grep or read source files in an
//! indexed project, add one line saying the codegraph tools exist. After Graphify's `hook-guard`
//! (`cli.py`): facts, not commands (commands in a tool result read like prompt injection), a small
//! budget per agent (nudging on every call costs more tokens than the queries it suggests), and it
//! never blocks: any problem means no output.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::index;
use crate::lang::Lang;

/// Nudges one agent gets per session unless `CODEGRAPH_HOOK_NUDGES` says otherwise (0 = none).
pub const DEFAULT_BUDGET: usize = 3;

const SEARCH_TEXT: &str = "codegraph: this project is indexed. The codegraph MCP tools `callers`, `def`, `explain` and `hubs` answer who-calls-what in a few lines; grep works best once they point to the exact file.";
const READ_TEXT: &str = "codegraph: this project is indexed. The codegraph MCP tools `explain` (definition, callers, callees) and `def` give the lines that matter without reading whole files; reading works best once they point to the exact lines.";

/// Commands that search file contents or names.
const SEARCH_PROGRAMS: [&str; 8] = ["rg", "grep", "egrep", "fgrep", "ag", "ack", "find", "fd"];
/// Words that come before the real program on a command line.
const WRAPPERS: [&str; 15] = [
    "sudo", "time", "env", "nice", "xargs", "command", "exec", "nohup", "do", "then", "else", "if",
    "while", "until", "!",
];

/// The JSON line to print for a tool call, or `None` for silence. `input` is the hook's stdin,
/// `kind` is `search` or `read`, `state_dir` holds the per-agent counters (see `state_dir`).
pub fn nudge(
    kind: &str,
    input: &str,
    project: &Path,
    state_dir: &Path,
    budget: usize,
) -> Option<String> {
    if budget == 0 || !index::cache_exists(project) {
        return None;
    }
    let call: Value = serde_json::from_str(input).ok()?;
    let tool = call.get("tool_input")?.as_object()?;
    let text = match kind {
        "search" if is_search(tool) => SEARCH_TEXT,
        "read" if reads_source_in(tool, project) => READ_TEXT,
        _ => return None,
    };
    spend(&call, state_dir, budget)?;
    let out = json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "additionalContext": text } });
    Some(format!("{out}\n"))
}

/// A Grep tool call (it has a `pattern`), or a shell command that runs a search program.
fn is_search(tool: &Map<String, Value>) -> bool {
    match tool.get("command").and_then(Value::as_str) {
        Some(command) => runs_search_program(command),
        None => tool.get("pattern").is_some_and(Value::is_string),
    }
}

/// Looks at the program each part of the command line starts: `cat a | grep b` searches,
/// `git commit -m "grep it"` and `echo "a | rg b"` do not (quoted text is ignored).
fn runs_search_program(command: &str) -> bool {
    let mut unquoted = String::new();
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            // `\'` is a plain quote character, not the start of a quoted span.
            (None, '\\') if matches!(chars.peek(), Some('\'' | '"')) => _ = chars.next(),
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, _) => unquoted.push(c),
            _ => {}
        }
    }
    // Words that come before the real program or are options/arguments of those wrappers.
    let skipped = |word: &&str| {
        word.contains('=')
            || word.starts_with('-')
            || WRAPPERS.contains(word)
            || word.chars().all(|c| c.is_ascii_digit())
    };
    unquoted
        .split(['|', ';', '&', '\n', '(', ')', '`'])
        .any(|part| {
            let mut words = part.split_whitespace().filter(|w| !skipped(w));
            let Some(first) = words.next() else {
                return false;
            };
            let first = first.trim_start_matches('\\');
            let program = first.rsplit('/').next().unwrap_or(first);
            SEARCH_PROGRAMS.contains(&program) || (program == "git" && words.next() == Some("grep"))
        })
}

/// A Read/Glob of a source file of a language we index, inside the project.
fn reads_source_in(tool: &Map<String, Value>, project: &Path) -> bool {
    // Both spellings of the project path; an empty path would match everything, so it is dropped.
    let roots: Vec<PathBuf> = [Some(project.to_path_buf()), project.canonicalize().ok()]
        .into_iter()
        .flatten()
        .filter(|root| !root.as_os_str().is_empty())
        .collect();
    let inside = |target: &str| {
        let path = Path::new(target);
        let local = !path.is_absolute() || roots.iter().any(|root| path.starts_with(root));
        local && !path.components().any(|c| c == Component::ParentDir)
    };
    let get = |key: &str| tool.get(key).and_then(Value::as_str);
    // A Glob aimed at another directory is not about this project, whatever its pattern says.
    if get("path").is_some_and(|dir| !inside(dir)) {
        return false;
    }
    ["file_path", "path", "pattern"]
        .iter()
        .filter_map(|key| get(key))
        .any(|target| inside(target) && Lang::from_path(target).is_some())
}

/// Where the per-agent counters live: a private directory (mode 0700) of this user, so nobody
/// else can plant files there. `None` when no such directory can be had.
pub fn state_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && dir.is_dir());
    if let Some(dir) = runtime {
        return Some(dir); // the system makes it private to the user
    }
    let user: String = ["USER", "LOGNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    private_dir(
        &std::env::temp_dir(),
        if user.is_empty() { "user" } else { &user },
    )
}

fn private_dir(base: &Path, user: &str) -> Option<PathBuf> {
    let dir = base.join(format!("codegraph-hook-{user}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        // Not a symlink, and closed to other users: anything else could be someone's trap.
        let meta = fs::symlink_metadata(&dir).ok()?;
        (meta.is_dir() && meta.permissions().mode() & 0o077 == 0).then_some(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }
}

/// The counter file of one agent: named by a hash of the raw ids, so odd ids cannot collide
/// with each other or point outside `state_dir`.
fn marker(call: &Value, state_dir: &Path) -> Option<PathBuf> {
    let id = |key: &str| call.get(key).and_then(Value::as_str).unwrap_or("");
    let (session, agent) = (id("session_id"), id("agent_id"));
    if session.is_empty() {
        return None; // cannot be counted, so stay silent
    }
    let hash = blake3::hash(format!("{session}\0{agent}").as_bytes()).to_hex();
    Some(state_dir.join(format!("codegraph-nudge-{}", &hash[..32])))
}

/// Count this nudge against the agent's budget; `None` when it is used up or cannot be recorded.
/// The counter is a file that grows by one byte per nudge (append is atomic), so parallel hooks
/// cannot overspend by more than their own number.
fn spend(call: &Value, state_dir: &Path, budget: usize) -> Option<()> {
    let marker = marker(call, state_dir)?;
    let budget = u64::try_from(budget).ok()?;
    if fs::symlink_metadata(&marker).is_ok_and(|meta| !meta.is_file()) {
        return None;
    }
    if fs::metadata(&marker).map_or(0, |meta| meta.len()) >= budget {
        return None;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&marker)
        .ok()?;
    file.write_all(b"x").ok()?;
    (file.metadata().ok()?.len() <= budget).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn a() {}\n").unwrap();
        let (idx, _) = index::build(dir.path(), None).unwrap();
        index::save(dir.path(), &idx).unwrap();
        dir
    }

    fn call(tool_input: Value) -> String {
        json!({ "session_id": "s1", "tool_name": "x", "tool_input": tool_input }).to_string()
    }

    fn run(kind: &str, input: &str, dir: &Path, state: &Path) -> Option<String> {
        nudge(kind, input, dir, state, 3)
    }

    #[test]
    fn nudges_a_grep_in_an_indexed_project_with_valid_claude_code_json() {
        let (dir, state) = (project(), tempfile::tempdir().unwrap());
        let out = run(
            "search",
            &call(json!({ "pattern": "foo" })),
            dir.path(),
            state.path(),
        )
        .unwrap();
        assert!(out.ends_with('\n'));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        let text = parsed["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(text.starts_with("codegraph:") && !text.to_lowercase().contains("must"));
    }

    #[test]
    fn silent_without_an_index_or_on_bad_input() {
        let (dir, state) = (project(), tempfile::tempdir().unwrap());
        let empty = tempfile::tempdir().unwrap();
        let grep = call(json!({ "pattern": "foo" }));
        assert_eq!(run("search", &grep, empty.path(), state.path()), None);
        for bad in [
            "",
            "not json",
            "[]",
            r#"{"tool_input": 3}"#,
            r#"{"session_id":"s"}"#,
        ] {
            assert_eq!(run("search", bad, dir.path(), state.path()), None, "{bad}");
        }
        assert_eq!(run("other", &grep, dir.path(), state.path()), None);
        let no_session = json!({ "tool_input": { "pattern": "x" } }).to_string();
        assert_eq!(run("search", &no_session, dir.path(), state.path()), None);
        assert_eq!(nudge("search", &grep, dir.path(), state.path(), 0), None);
    }

    #[test]
    fn only_shell_commands_that_run_a_search_program_count() {
        let (dir, state) = (project(), tempfile::tempdir().unwrap());
        let searches = |command: &str| {
            let input =
                json!({ "session_id": command, "tool_input": { "command": command } }).to_string();
            run("search", &input, dir.path(), state.path()).is_some()
        };
        for yes in [
            "rg foo src",
            "cat a.txt | grep b",
            "FOO=1 /usr/bin/rg x",
            "cd src && find . -name '*.rs'",
            "sudo grep -r x .",
            "env -i rg x",
            "ls | xargs grep x",
            "\\grep x f",
            "git grep foo",
            "nice -n 5 rg x",
            "echo it\\'s | grep x",
            "if grep -q x f; then echo y; fi",
        ] {
            assert!(searches(yes), "{yes}");
        }
        for no in [
            "cargo build",
            "git commit -m \"grep it\"",
            "git commit -m \"fix | grep bug\"",
            "echo \"a; rg x\"",
            "echo 'find me'",
            "echo hello",
        ] {
            assert!(!searches(no), "{no}");
        }
    }

    #[test]
    fn reads_count_only_for_source_files_inside_the_project() {
        let (dir, state) = (project(), tempfile::tempdir().unwrap());
        let nudged = |input: Value| {
            let mut v = json!({ "session_id": input.to_string(), "tool_input": input });
            v["tool_name"] = json!("Read");
            run("read", &v.to_string(), dir.path(), state.path()).is_some()
        };
        let inside = dir.path().join("lib.rs");
        assert!(nudged(json!({ "file_path": "src/main.rs" })));
        assert!(nudged(json!({ "file_path": inside.to_str().unwrap() })));
        assert!(
            nudged(json!({ "file_path": "src/a..b.rs" })),
            "dots in a name are fine"
        );
        assert!(nudged(
            json!({ "pattern": "**/*.rs", "path": dir.path().to_str().unwrap() })
        ));
        assert!(!nudged(json!({ "file_path": "README.md" })));
        assert!(!nudged(json!({ "file_path": "/etc/other/thing.rs" })));
        assert!(!nudged(json!({ "file_path": "../outside/x.rs" })));
        assert!(!nudged(
            json!({ "pattern": "**/*.rs", "path": "/somewhere/else" })
        ));
    }

    #[test]
    fn a_project_path_that_cannot_be_resolved_matches_nothing_outside_it() {
        let tool = json!({ "file_path": "/etc/passwd.rs" });
        let tool = tool.as_object().unwrap();
        assert!(!reads_source_in(tool, Path::new("/nonexistent/zzz")));
        assert!(!reads_source_in(tool, Path::new("")));
    }

    #[test]
    fn a_symlinked_project_path_still_counts_as_inside() {
        #[cfg(unix)]
        {
            let (dir, state) = (project(), tempfile::tempdir().unwrap());
            let link_dir = tempfile::tempdir().unwrap();
            let link = link_dir.path().join("proj");
            std::os::unix::fs::symlink(dir.path(), &link).unwrap();
            let target = dir.path().canonicalize().unwrap().join("lib.rs");
            let input = call(json!({ "file_path": target.to_str().unwrap() }));
            assert!(run("read", &input, &link, state.path()).is_some());
        }
    }

    #[test]
    fn each_agent_gets_a_small_budget_and_hostile_ids_stay_inside_the_state_dir() {
        let (dir, state) = (project(), tempfile::tempdir().unwrap());
        let grep = |session: &str, agent: Option<&str>| {
            let mut v = json!({ "session_id": session, "tool_input": { "pattern": "x" } });
            if let Some(agent) = agent {
                v["agent_id"] = json!(agent);
            }
            nudge("search", &v.to_string(), dir.path(), state.path(), 2).is_some()
        };
        assert_eq!(
            [
                grep("a", None),
                grep("a", None),
                grep("a", None),
                grep("a", None)
            ],
            [true, true, false, false]
        );
        assert!(grep("b", None), "another session starts fresh");
        assert!(grep("a", Some("sub1")), "a subagent has its own budget");
        // Ids that would collide or escape once sanitised stay apart and inside the directory.
        assert!(
            grep("a/b", None) && grep("a_b", None),
            "distinct ids, distinct budgets"
        );
        assert!(grep("../../etc/x", None));
        let names: Vec<_> = fs::read_dir(state.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .all(|n| n.starts_with("codegraph-nudge-") && n.len() == 48),
            "{names:?}"
        );
    }

    #[test]
    fn an_unwritable_counter_or_a_planted_symlink_means_silence() {
        let dir = project();
        let input = call(json!({ "pattern": "x" }));
        let missing = dir.path().join("no/such/state/dir");
        assert_eq!(run("search", &input, dir.path(), &missing), None);
        #[cfg(unix)]
        {
            let state = tempfile::tempdir().unwrap();
            let victim = state.path().join("victim");
            fs::write(&victim, "precious").unwrap();
            let call_json: Value = serde_json::from_str(&input).unwrap();
            let marker = marker(&call_json, state.path()).unwrap();
            std::os::unix::fs::symlink(&victim, &marker).unwrap();
            assert_eq!(run("search", &input, dir.path(), state.path()), None);
            assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_state_directory_must_be_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let made = private_dir(base.path(), "me").unwrap();
        assert_eq!(
            fs::metadata(&made).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            private_dir(base.path(), "me"),
            Some(made.clone()),
            "reusable"
        );
        // Someone pre-created it open to everyone, or as a symlink: refused.
        fs::set_permissions(&made, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(private_dir(base.path(), "me"), None);
        let trap = base.path().join("elsewhere");
        fs::create_dir(&trap).unwrap();
        std::os::unix::fs::symlink(&trap, base.path().join("codegraph-hook-you")).unwrap();
        assert_eq!(private_dir(base.path(), "you"), None);
    }
}
