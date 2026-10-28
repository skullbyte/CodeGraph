//! `codegraph install` / `uninstall`: wire Claude Code to the graph for ONE project. Writes
//! `.claude/settings.json` (the PreToolUse hooks), `.mcp.json` (the MCP server) and
//! `.claude/skills/codegraph/SKILL.md`. Design after Graphify's `install.py` for Claude Code, but
//! project-scoped only (never `~/.claude`), and it only ever touches what it owns: our hooks are
//! recognised by their exact command, our server entry by its exact value, the skill by a marker
//! line. Anything else of the same name is kept and reported.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::index;

const SKILL: &str = include_str!("skill.md");
const SKILL_MARKER: &str = "<!-- codegraph-managed";
const HOOK_COMMANDS: [&str; 2] = ["codegraph hook search", "codegraph hook read"];
/// The line `~/.claude/bin/codemap.sh` used to run at session start; `codegraph index` replaces it.
const OLD_SESSION_START: &str =
    "\"$HOME\"/.claude/bin/codemap.sh \"$CLAUDE_PROJECT_DIR\" >/dev/null 2>&1";
const NEW_SESSION_START: &str = "codegraph index \"$CLAUDE_PROJECT_DIR\" >/dev/null 2>&1";

fn our_hooks() -> Vec<Value> {
    let entry = |matcher: &str, command: &str| {
        json!({
            "matcher": matcher,
            "hooks": [{ "type": "command", "command": command, "timeout": 10 }],
        })
    };
    vec![
        entry("Bash|Grep", HOOK_COMMANDS[0]),
        entry("Read|Glob", HOOK_COMMANDS[1]),
    ]
}

fn our_server() -> Value {
    json!({ "command": "codegraph", "args": ["serve"] })
}

fn is_our_command(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| HOOK_COMMANDS.contains(&c))
}

/// Take our hooks out of every group; a group left empty goes too, a group with other hooks stays.
/// Returns whether anything of ours was found.
fn strip_our_hooks(groups: &mut Vec<Value>) -> bool {
    let mut found = false;
    groups.retain_mut(|group| {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|h| !is_our_command(h));
        found |= hooks.len() != before;
        // Drop the group only when it was our hooks that emptied it.
        before == 0 || !hooks.is_empty()
    });
    found
}

/// A JSON object member that must itself be an object / array, created when absent.
fn member<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    empty: Value,
) -> Result<&'a mut Value> {
    let value = object.entry(key).or_insert(empty.clone());
    if std::mem::discriminant(value) != std::mem::discriminant(&empty) {
        bail!(
            "`{key}` is not a JSON {}",
            if empty.is_array() { "array" } else { "object" }
        );
    }
    Ok(value)
}

fn root_object(value: &mut Value) -> Result<&mut Map<String, Value>> {
    value
        .as_object_mut()
        .context("the top level is not a JSON object")
}

/// `settings` with our two PreToolUse hooks present exactly once; nothing else changes, except that
/// the old codemap.sh session-start line (exact match only) becomes `codegraph index`.
pub fn add_to_settings(mut settings: Value) -> Result<Value> {
    let hooks = member(root_object(&mut settings)?, "hooks", json!({}))?;
    let hooks = root_object(hooks)?;
    let pre = member(hooks, "PreToolUse", json!([]))?
        .as_array_mut()
        .context("PreToolUse")?;
    strip_our_hooks(pre);
    pre.extend(our_hooks());
    if let Some(groups) = hooks.get_mut("SessionStart").and_then(Value::as_array_mut) {
        for hook in groups
            .iter_mut()
            .filter_map(|g| g.get_mut("hooks").and_then(Value::as_array_mut))
            .flatten()
        {
            if hook.get("command").and_then(Value::as_str) == Some(OLD_SESSION_START) {
                hook["command"] = json!(NEW_SESSION_START);
            }
        }
    }
    Ok(settings)
}

/// `settings` without our hooks. A container we emptied (`PreToolUse`, then `hooks`) goes too;
/// one that was already empty is not ours to tidy.
pub fn remove_from_settings(mut settings: Value) -> Result<Value> {
    let root = root_object(&mut settings)?;
    let mut tidy_hooks = false;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut)
        && let Some(pre) = hooks.get_mut("PreToolUse").and_then(Value::as_array_mut)
        && strip_our_hooks(pre)
        && pre.is_empty()
    {
        hooks.remove("PreToolUse");
        tidy_hooks = hooks.is_empty();
    }
    if tidy_hooks {
        root.remove("hooks");
    }
    Ok(settings)
}

/// `.mcp.json` with the `codegraph` server registered. A server of that name that is not exactly
/// ours is the user's: it stays, and the flag is `false`.
pub fn add_to_mcp(mut config: Value) -> Result<(Value, bool)> {
    let servers = member(root_object(&mut config)?, "mcpServers", json!({}))?;
    let servers = root_object(servers)?;
    let ours = servers.get("codegraph").is_none_or(|s| *s == our_server());
    if ours {
        servers.insert("codegraph".into(), our_server());
    }
    Ok((config, ours))
}

/// `.mcp.json` without our `codegraph` server (a different server of that name stays).
pub fn remove_from_mcp(mut config: Value) -> Result<Value> {
    let root = root_object(&mut config)?;
    let mut emptied = false;
    if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut)
        && servers.get("codegraph") == Some(&our_server())
    {
        servers.remove("codegraph");
        emptied = servers.is_empty();
    }
    if emptied {
        root.remove("mcpServers");
    }
    Ok(config)
}

/// The JSON file's content (`{}` when missing or blank); an invalid file is refused, never replaced.
fn read_json(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not valid JSON; fix or remove it first",
                path.display()
            )
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    write_text(path, &(serde_json::to_string_pretty(value)? + "\n"))
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    index::write_atomic(path, text.as_bytes())
}

/// What is at the skill path: nothing, our file, or somebody else's (also: unreadable).
enum Skill {
    Absent,
    Ours,
    Foreign,
}

fn skill_at(path: &Path) -> Skill {
    match fs::read_to_string(path) {
        Ok(text) if text.contains(SKILL_MARKER) => Skill::Ours,
        Ok(_) => Skill::Foreign,
        Err(e) if e.kind() == ErrorKind::NotFound => Skill::Absent,
        Err(_) => Skill::Foreign,
    }
}

/// Install into `root`; with `print_only` nothing is written and the new contents are returned.
/// Returns one line per file for the user. Everything is read and checked before the first write,
/// and `settings.json` (the hooks that point at the rest) is written last.
pub fn install(root: &Path, print_only: bool) -> Result<Vec<String>> {
    let settings_path = root.join(".claude/settings.json");
    let mcp_path = root.join(".mcp.json");
    let skill_path = root.join(".claude/skills/codegraph/SKILL.md");
    let in_file = |path: &Path| format!("in {}", path.display());
    let settings =
        add_to_settings(read_json(&settings_path)?).with_context(|| in_file(&settings_path))?;
    let (mcp, mcp_is_ours) =
        add_to_mcp(read_json(&mcp_path)?).with_context(|| in_file(&mcp_path))?;
    let skill = skill_at(&skill_path);

    if print_only {
        return Ok(vec![
            format!(
                "# {}\n{}",
                settings_path.display(),
                serde_json::to_string_pretty(&settings)?
            ),
            format!(
                "# {}\n{}",
                mcp_path.display(),
                serde_json::to_string_pretty(&mcp)?
            ),
            format!("# {}\n{SKILL}", skill_path.display()),
        ]);
    }
    let mut done = Vec::new();
    if mcp_is_ours {
        write_json(&mcp_path, &mcp)?;
        done.push("wrote .mcp.json (codegraph MCP server; Claude Code asks you to approve it at the next start)".to_string());
    } else {
        done.push("kept your own `codegraph` server in .mcp.json".to_string());
    }
    if matches!(skill, Skill::Foreign) {
        done.push("kept your own .claude/skills/codegraph/SKILL.md".to_string());
    } else {
        write_text(&skill_path, SKILL)?;
        done.push("wrote .claude/skills/codegraph/SKILL.md".to_string());
    }
    write_json(&settings_path, &settings)?;
    done.push("wrote .claude/settings.json (search/read hooks)".to_string());
    Ok(done)
}

/// Remove what `install` added to `root`. A file is rewritten only when something of ours is in it;
/// a file that cannot be read or is not valid JSON is left alone and reported, and the rest goes on.
pub fn uninstall(root: &Path) -> Result<Vec<String>> {
    let mut done = Vec::new();
    let mut edit_json = |relative: &str, strip: fn(Value) -> Result<Value>| {
        let path = root.join(relative);
        let outcome = (|| -> Result<bool> {
            if !path.is_file() {
                return Ok(false);
            }
            let before = read_json(&path)?;
            let after = strip(before.clone())?;
            if after == before {
                return Ok(false);
            }
            write_json(&path, &after)?;
            Ok(true)
        })();
        match outcome {
            Ok(true) => done.push(format!("removed ours from {relative}")),
            Ok(false) => {}
            Err(e) => done.push(format!("left {relative} alone: {e:#}")),
        }
    };
    edit_json(".claude/settings.json", remove_from_settings);
    edit_json(".mcp.json", remove_from_mcp);
    let skill_path = root.join(".claude/skills/codegraph/SKILL.md");
    if matches!(skill_at(&skill_path), Skill::Ours) {
        match fs::remove_file(&skill_path) {
            Ok(()) => done.push("removed .claude/skills/codegraph/SKILL.md".to_string()),
            Err(e) => done.push(format!("left .claude/skills/codegraph/SKILL.md alone: {e}")),
        }
    }
    let settings_text = fs::read_to_string(root.join(".claude/settings.json")).unwrap_or_default();
    if settings_text.contains("codegraph index") {
        done.push(
            "note: a SessionStart hook still runs `codegraph index`; edit it if you remove the binary"
                .to_string(),
        );
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Value {
        json!({
            "permissions": { "allow": ["Bash(cargo build*)"] },
            "hooks": {
                "SessionStart": [{ "hooks": [{ "type": "command", "command": OLD_SESSION_START }] }],
                "PreToolUse": [{ "matcher": "Edit", "hooks": [{ "type": "command", "command": "my-linter" }] }],
                "PostToolUse": [{ "matcher": "Write", "hooks": [{ "type": "command", "command": "fmt.sh" }] }],
            },
        })
    }

    #[test]
    fn adding_keeps_everything_else_and_is_idempotent() {
        let once = add_to_settings(settings()).unwrap();
        assert_eq!(once["permissions"], settings()["permissions"]);
        assert_eq!(
            once["hooks"]["PostToolUse"],
            settings()["hooks"]["PostToolUse"]
        );
        let pre = once["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 3, "the user's own hook stays, ours are added");
        assert_eq!(pre[0]["hooks"][0]["command"], "my-linter");
        assert_eq!(pre[1]["matcher"], "Bash|Grep");
        assert_eq!(pre[1]["hooks"][0]["command"], "codegraph hook search");
        assert_eq!(pre[2]["hooks"][0]["command"], "codegraph hook read");
        assert_eq!(add_to_settings(once.clone()).unwrap(), once);
    }

    #[test]
    fn only_the_exact_old_session_start_line_is_replaced() {
        let once = add_to_settings(settings()).unwrap();
        assert_eq!(
            once["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            NEW_SESSION_START
        );
        let custom = json!({ "hooks": { "SessionStart": [{ "hooks": [{ "command": "codemap.sh --other" }] }] } });
        let kept = add_to_settings(custom).unwrap();
        assert_eq!(
            kept["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "codemap.sh --other"
        );
    }

    #[test]
    fn only_our_exact_commands_are_ours() {
        let foreign = json!({ "hooks": { "PreToolUse": [
            { "matcher": "Bash", "hooks": [{ "command": "echo codegraph hook search" }] },
            { "matcher": "Bash", "hooks": [{ "command": "mycodegraph hook read" }] },
            { "matcher": "Grep", "hooks": [{ "command": "my-grep-guard" }, { "command": "codegraph hook search" }] },
        ] } });
        let installed = add_to_settings(foreign.clone()).unwrap();
        let pre = installed["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 5, "3 of theirs (one group trimmed) + our 2");
        assert_eq!(
            pre[2]["hooks"],
            json!([{ "command": "my-grep-guard" }]),
            "their hook in a mixed group stays"
        );
        let removed = remove_from_settings(installed).unwrap();
        let mut expected = foreign;
        expected["hooks"]["PreToolUse"][2]["hooks"] = json!([{ "command": "my-grep-guard" }]);
        assert_eq!(removed, expected);
    }

    #[test]
    fn an_already_empty_group_of_the_user_is_not_ours_to_drop() {
        let theirs = json!({ "hooks": { "PreToolUse": [{ "matcher": "X", "hooks": [] }] } });
        let installed = add_to_settings(theirs.clone()).unwrap();
        assert_eq!(
            installed["hooks"]["PreToolUse"][0],
            json!({ "matcher": "X", "hooks": [] })
        );
        assert_eq!(remove_from_settings(installed).unwrap(), theirs);
    }

    #[test]
    fn removing_takes_only_ours_and_tidies_empty_containers() {
        let installed = add_to_settings(settings()).unwrap();
        let removed = remove_from_settings(installed).unwrap();
        assert_eq!(
            removed["hooks"]["PreToolUse"],
            settings()["hooks"]["PreToolUse"]
        );
        let bare = remove_from_settings(add_to_settings(json!({})).unwrap()).unwrap();
        assert_eq!(bare, json!({}));
    }

    #[test]
    fn odd_shapes_are_refused_not_overwritten() {
        assert!(add_to_settings(json!([1])).is_err());
        assert!(add_to_settings(json!({ "hooks": 3 })).is_err());
        assert!(add_to_settings(json!({ "hooks": { "PreToolUse": {} } })).is_err());
        assert!(add_to_mcp(json!({ "mcpServers": [] })).is_err());
    }

    #[test]
    fn mcp_registration_merges_unregisters_and_respects_a_server_of_the_same_name() {
        let other = json!({ "mcpServers": { "other": { "command": "x" } } });
        let (added, ours) = add_to_mcp(other.clone()).unwrap();
        assert!(ours);
        assert_eq!(added["mcpServers"]["codegraph"]["args"], json!(["serve"]));
        assert_eq!(added["mcpServers"]["other"], other["mcpServers"]["other"]);
        assert_eq!(remove_from_mcp(added).unwrap(), other);
        let (bare, _) = add_to_mcp(json!({})).unwrap();
        assert_eq!(remove_from_mcp(bare).unwrap(), json!({}));

        let mine = json!({ "mcpServers": { "codegraph": { "command": "/opt/codegraph", "args": ["serve", "--dir", "x"] } } });
        let (kept, ours) = add_to_mcp(mine.clone()).unwrap();
        assert!(
            !ours && kept == mine,
            "a different server of that name stays"
        );
        assert_eq!(remove_from_mcp(mine.clone()).unwrap(), mine);
        let untouched = json!({ "mcpServers": {} });
        assert_eq!(
            remove_from_mcp(untouched.clone()).unwrap(),
            untouched,
            "an empty container is not ours to tidy"
        );
    }

    #[test]
    fn install_and_uninstall_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = |p: &str| dir.path().join(p);
        fs::create_dir_all(path(".claude")).unwrap();
        fs::write(
            path(".claude/settings.json"),
            serde_json::to_string(&settings()).unwrap(),
        )
        .unwrap();

        let printed = install(dir.path(), true).unwrap();
        assert_eq!(printed.len(), 3);
        assert!(!path(".mcp.json").exists(), "--print writes nothing");

        install(dir.path(), false).unwrap();
        let first = fs::read_to_string(path(".claude/settings.json")).unwrap();
        install(dir.path(), false).unwrap();
        assert_eq!(
            fs::read_to_string(path(".claude/settings.json")).unwrap(),
            first,
            "idempotent"
        );
        assert!(path(".mcp.json").is_file());
        assert!(
            fs::read_to_string(path(".claude/skills/codegraph/SKILL.md"))
                .unwrap()
                .contains(SKILL_MARKER)
        );

        let report = uninstall(dir.path()).unwrap();
        assert!(
            report.iter().any(|l| l.contains("SessionStart")),
            "{report:?}"
        );
        assert!(!path(".claude/skills/codegraph/SKILL.md").exists());
        let after: Value =
            serde_json::from_str(&fs::read_to_string(path(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            after["hooks"]["PreToolUse"],
            settings()["hooks"]["PreToolUse"]
        );
        assert_eq!(after["permissions"], settings()["permissions"]);
        let mcp: Value =
            serde_json::from_str(&fs::read_to_string(path(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp, json!({}), "our server is gone");
    }

    #[test]
    fn uninstall_touches_only_files_that_hold_something_of_ours() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let original = "{\n  \"hooks\":   {},\n  \"b\": 1,\n  \"a\": 2\n}";
        fs::write(dir.path().join(".claude/settings.json"), original).unwrap();
        fs::write(dir.path().join(".mcp.json"), "{ not json").unwrap();
        let report = uninstall(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
            original,
            "not reformatted"
        );
        assert!(
            report.iter().any(|l| l.starts_with("left .mcp.json alone")),
            "{report:?}"
        );
        assert!(
            !report.iter().any(|l| l.starts_with("removed")),
            "{report:?}"
        );
    }

    #[test]
    fn a_foreign_skill_and_invalid_json_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join(".claude/skills/codegraph/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::write(&skill, "my own skill").unwrap();
        install(dir.path(), false).unwrap();
        assert_eq!(fs::read_to_string(&skill).unwrap(), "my own skill");
        uninstall(dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&skill).unwrap(), "my own skill");

        // A skill file that cannot be read as text is not ours to replace either.
        fs::write(&skill, [0xff, 0xfe, 0x00]).unwrap();
        install(dir.path(), false).unwrap();
        assert_eq!(fs::read(&skill).unwrap(), vec![0xff, 0xfe, 0x00]);

        // Either JSON file being invalid stops everything before any write.
        for broken_file in [".claude/settings.json", ".mcp.json"] {
            let broken = tempfile::tempdir().unwrap();
            fs::create_dir_all(broken.path().join(".claude")).unwrap();
            fs::write(broken.path().join(broken_file), "{ not json").unwrap();
            assert!(install(broken.path(), false).is_err(), "{broken_file}");
            assert_eq!(
                fs::read_to_string(broken.path().join(broken_file)).unwrap(),
                "{ not json"
            );
            let written = ["settings.json", "skills"]
                .iter()
                .filter(|name| broken.path().join(".claude").join(name).exists())
                .count();
            let expected = usize::from(broken_file.ends_with("settings.json"));
            assert_eq!(written, expected, "nothing half-written for {broken_file}");
            assert!(broken_file == ".mcp.json" || !broken.path().join(".mcp.json").exists());
        }
    }
}
