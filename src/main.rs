//! codegraph command line: parses arguments, calls the library, prints results.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;

use codegraph::model::Index;
use codegraph::query::{Affected, CallerHit, DefHit, Hop, Hub};
use codegraph::{
    benchmark, codemap, community, glossary, hook, html, index, install, mermaid, obsidian, query,
    report, scip, semantic, serve, watch,
};

/// Fast offline code graph: where is X, who calls X, how does A reach B.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Project root (defaults to the current directory).
    #[arg(long, global = true, default_value = ".")]
    dir: PathBuf,
    /// Print JSON instead of plain text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index the project: writes .claude/codemap.md and .claude/graph.json.
    Index {
        /// Project root; overrides --dir.
        #[arg(value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Ask in plain English: the matching symbols and what they call or are called by.
    Query {
        question: String,
        /// How many hops to walk from the matching symbols.
        #[arg(long, default_value_t = codegraph::ask::DEFAULT_DEPTH as u64, value_parser = clap::value_parser!(u64).range(1..=codegraph::ask::MAX_DEPTH as u64))]
        depth: u64,
        /// Answer size in tokens (a token counts as 3 characters).
        #[arg(long, default_value_t = codegraph::ask::DEFAULT_BUDGET)]
        budget: usize,
        /// Walk depth-first instead of breadth-first.
        #[arg(long)]
        dfs: bool,
    },
    /// A reading order for the project: entry files, then the key file of each cluster.
    Tour {
        /// How many files to list.
        #[arg(long, default_value_t = codegraph::tour::DEFAULT_TOP)]
        top: usize,
    },
    /// Where is <name> defined?
    Def {
        name: String,
        /// Add a plain-language line under each hit (what the keywords mean, its doc comment); text output only.
        #[arg(long)]
        teach: bool,
    },
    /// Who calls <name>?
    Callers {
        name: String,
        /// Only calls that can reach this definition: PATH or PATH:LINE (see `def <name>`).
        #[arg(long)]
        def: Option<String>,
    },
    /// Shortest call chain (function names) or import chain (file paths) from A to B.
    Path { from: String, to: String },
    /// Re-index one file.
    Update { file: PathBuf },
    /// Definition, callers and callees of <name>.
    Explain {
        name: String,
        /// Add plain-language lines: what the keywords mean, the doc comment, what to read next (text output only).
        #[arg(long)]
        teach: bool,
        /// How much to print: brief (default), full, callers, callees or defs (--json is always full).
        #[arg(long, default_value = "brief")]
        mode: query::Mode,
        /// Only this definition: PATH or PATH:LINE (see `def <name>`).
        #[arg(long)]
        def: Option<String>,
    },
    /// Everything that depends on <name>: its callers, their callers, and so on.
    Affected {
        name: String,
        /// Stop after this many hops.
        #[arg(long)]
        depth: Option<usize>,
    },
    /// Most-connected symbols in the project.
    Hubs {
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
    /// Write .claude/GRAPH_REPORT.md: key modules, hubs, surprising links, communities.
    Report,
    /// Write .claude/graph.html: one offline page with an interactive map of the files.
    Html,
    /// Print a Mermaid flowchart of what <name> calls (GitHub and Obsidian draw it).
    Mermaid {
        name: String,
        /// How many call hops to follow.
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..))]
        depth: u64,
    },
    /// Write an Obsidian vault: one linked note per file and per community.
    Obsidian {
        /// Vault folder (defaults to .claude/obsidian under the project).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Make `callers`, `hubs` and the graph exact: read a SCIP index (from `rust-analyzer scip .`
    /// or `scip-typescript index`) and record what every call really refers to.
    Scip {
        /// The index file (defaults to index.scip in the project root).
        file: Option<PathBuf>,
    },
    /// Wire Claude Code to the graph for this project: search/read hooks, the MCP server, a skill.
    Install {
        /// Show what would be written, change nothing.
        #[arg(long)]
        print: bool,
    },
    /// Remove what `install` added (only our own entries).
    Uninstall,
    /// Claude Code PreToolUse hook (reads the tool call on stdin); prints nothing when no nudge is due.
    Hook {
        /// `search` for Bash/Grep calls, `read` for Read/Glob calls.
        #[arg(value_parser = ["search", "read"])]
        kind: String,
    },
    /// The optional AI layer for documents: list what Claude still has to read, store what it found.
    Docs {
        #[command(subcommand)]
        action: DocsAction,
    },
    /// Serve the queries to Claude Code as MCP tools over stdin/stdout.
    Serve,
    /// How exact are the call links? Scores each strategy against SCIP (run `just scip` first).
    Precision {
        /// Also list up to N calls per form kind that were not pinned to the right definition.
        #[arg(long, default_value_t = 0)]
        misses: usize,
    },
    /// Compare codegraph with Graphify (if installed): index time and `explain` answers.
    Benchmark {
        /// Symbol to compare on (repeatable); defaults to the 5 top hubs.
        #[arg(long = "name")]
        names: Vec<String>,
    },
    /// Index, then keep the index fresh as files change (stop with Ctrl-C).
    Watch {
        /// Project root; overrides --dir.
        #[arg(value_name = "DIR")]
        root: Option<PathBuf>,
        /// How often to check for changes.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
}

#[derive(Subcommand)]
enum DocsAction {
    /// Docs not read yet (new or changed), with line counts and the plan: inline or subagent chunks.
    Pending {
        /// Stop the list before it goes over this many lines.
        #[arg(long)]
        max_lines: Option<usize>,
    },
    /// Validate and store what an agent found in one doc (a JSON file).
    Ingest { file: PathBuf },
    /// Print the text of a listed doc; for a PDF the text is extracted locally (cheap in tokens).
    Text {
        /// Path relative to the project, as `docs pending` shows it.
        doc: String,
    },
}

fn main() -> ExitCode {
    quiet_on_closed_pipe();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Rust ignores SIGPIPE, so `println!` into a closed pipe (`codegraph callers x | head`) panics.
/// A reader that hangs up is not an error: exit quietly.
fn quiet_on_closed_pipe() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if is_broken_pipe(&info.to_string()) {
            std::process::exit(0);
        }
        default(info);
    }));
}

fn is_broken_pipe(panic_message: &str) -> bool {
    panic_message.contains("failed printing to stdout") && panic_message.contains("Broken pipe")
}

fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Index { root } => {
            let root = root.as_ref().unwrap_or(&cli.dir);
            cmd_index(root)
        }
        Command::Query {
            question,
            depth,
            budget,
            dfs,
        } => {
            let opts = codegraph::ask::Options {
                depth: *depth as usize,
                budget: *budget,
                dfs: *dfs,
            };
            let answer = codegraph::ask::query(&load(&cli.dir)?, question, &opts);
            if cli.json {
                println!("{}", serde_json::to_string(&answer)?);
            } else {
                answer.lines().iter().for_each(|line| println!("{line}"));
            }
            Ok(())
        }
        Command::Precision { misses } => {
            anyhow::ensure!(
                !(cli.json && *misses > 0),
                "--misses prints text; drop --json"
            );
            let index = load(&cli.dir)?;
            let report = codegraph::precision::measure(&index);
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                for line in report.lines() {
                    println!("{line}");
                }
                for line in codegraph::precision::misses(&index, *misses) {
                    println!("miss {line}");
                }
            }
            Ok(())
        }
        Command::Tour { top } => {
            let stops = codegraph::tour::tour(&cli.dir, &load(&cli.dir)?, *top);
            if cli.json {
                println!("{}", serde_json::to_string(&stops)?);
            } else {
                for (n, stop) in stops.iter().enumerate() {
                    println!("{}", stop.text(n + 1));
                }
            }
            Ok(())
        }
        Command::Def { name, teach } => {
            let hits = semantic::def_with(&load(&cli.dir)?, &semantic::load_store(&cli.dir)?, name);
            if *teach && !cli.json {
                glossary::def_lines(&cli.dir, &hits, true)
                    .iter()
                    .for_each(|line| println!("{line}"));
                return Ok(());
            }
            print_results(&hits, cli.json, DefHit::text)
        }
        Command::Callers { name, def } => {
            let (index, store) = (load(&cli.dir)?, semantic::load_store(&cli.dir)?);
            let hits = match def {
                Some(sel) => {
                    semantic::callers_for(&index, &store, name, sel).map_err(|e| anyhow!(e))?
                }
                None => semantic::callers_with(&index, &store, name),
            };
            print_results(&hits, cli.json, CallerHit::text)
        }
        Command::Path { from, to } => {
            let hops = query::path(&load(&cli.dir)?, from, to);
            if hops.is_empty() && !cli.json {
                println!("no path");
                return Ok(());
            }
            print_results(&hops, cli.json, Hop::text)
        }
        Command::Update { file } => cmd_update(&cli.dir, file),
        Command::Explain {
            name,
            teach,
            mode,
            def,
        } => {
            let index = load(&cli.dir)?;
            let explained = match def {
                Some(sel) => {
                    semantic::explain_for(&index, &semantic::load_store(&cli.dir)?, name, sel)
                        .map_err(|e| anyhow!(e))?
                }
                None => semantic::explain_with(&index, &semantic::load_store(&cli.dir)?, name),
            };
            if cli.json {
                println!("{}", serde_json::to_string(&explained)?);
            } else {
                glossary::explain_lines(&cli.dir, &explained, *teach, *mode)
                    .iter()
                    .for_each(|line| println!("{line}"));
            }
            Ok(())
        }
        Command::Affected { name, depth } => print_results(
            &query::affected(&load(&cli.dir)?, name, *depth),
            cli.json,
            Affected::text,
        ),
        Command::Hubs { top } => {
            print_results(&query::hubs(&load(&cli.dir)?, *top), cli.json, Hub::text)
        }
        Command::Report => cmd_report(&cli.dir, cli.json),
        Command::Html => cmd_html(&cli.dir),
        Command::Mermaid { name, depth } => {
            let chart = mermaid::callflow(&load(&cli.dir)?, name, *depth as usize);
            print!(
                "{}",
                if chart.is_empty() {
                    "no calls\n"
                } else {
                    &chart
                }
            );
            Ok(())
        }
        Command::Install { print } => {
            install::install(&cli.dir, *print)?
                .iter()
                .for_each(|line| println!("{line}"));
            Ok(())
        }
        Command::Uninstall => {
            install::uninstall(&cli.dir)?
                .iter()
                .for_each(|line| println!("{line}"));
            Ok(())
        }
        Command::Hook { kind } => {
            cmd_hook(&cli.dir, kind);
            Ok(())
        }
        Command::Docs { action } => cmd_docs(&cli.dir, action, cli.json),
        Command::Serve => serve::run(&cli.dir),
        Command::Scip { file } => cmd_scip(&cli.dir, file.as_deref(), cli.json),
        Command::Obsidian { out } => {
            let out = out
                .clone()
                .unwrap_or_else(|| cli.dir.join(".claude/obsidian"));
            let count = obsidian::write(&load(&cli.dir)?, &out)?;
            println!("wrote {count} notes to {}", out.display());
            Ok(())
        }
        Command::Benchmark { names } => cmd_benchmark(&cli.dir, names, cli.json),
        Command::Watch { root, interval_ms } => {
            let root = root.as_ref().unwrap_or(&cli.dir);
            cmd_watch(root, Duration::from_millis(*interval_ms))
        }
    }
}

fn cmd_index(root: &Path) -> Result<()> {
    let _lock = index::lock(root)?;
    let (files, stats) = reindex(root)?;
    println!(
        "indexed {files} files ({} parsed, {} cached)",
        stats.parsed, stats.reused
    );
    if stats.skipped > 0 {
        eprintln!("warning: skipped {} unreadable files", stats.skipped);
    }
    Ok(())
}

/// Full (incremental) index; the caller must hold the lock. Returns the file count and stats.
fn reindex(root: &Path) -> Result<(usize, index::Stats)> {
    let previous = index::load(root);
    let (idx, stats) = index::build(root, previous.as_ref())?;
    write_outputs(root, &idx)?;
    Ok((idx.files.len(), stats))
}

fn cmd_update(root: &Path, file: &Path) -> Result<()> {
    let rel = relative_to(root, file)?;
    match update_paths(root, &[rel])?.first() {
        Some(failure) => bail!("{failure}"),
        None => Ok(()),
    }
}

/// Re-index the given files (paths relative to `root`) under the lock; with no cache yet, do a
/// full index instead. A file that can't be indexed doesn't stop the others: it comes back as
/// a message in the returned list.
fn update_paths(root: &Path, rels: &[String]) -> Result<Vec<String>> {
    let _lock = index::lock(root)?;
    let Some(mut idx) = index::load(root) else {
        reindex(root)?;
        return Ok(Vec::new());
    };
    let mut failed = Vec::new();
    for rel in rels {
        if let Err(e) = index::update_file(root, &mut idx, rel) {
            failed.push(format!("{rel}: {e:#}"));
        }
    }
    write_outputs(root, &idx)?;
    Ok(failed)
}

/// Print a line; false means stdout is closed (the reader went away), so the caller should stop.
fn say(line: &str) -> bool {
    writeln!(io::stdout(), "{line}").is_ok()
}

/// Index once, then poll for changed files and apply them until interrupted.
fn cmd_watch(root: &Path, interval: Duration) -> Result<()> {
    // Snapshot before indexing, so a change made while indexing is seen on the first tick.
    let mut previous = watch::snapshot(root)?;
    let files = {
        let _lock = index::lock(root)?;
        reindex(root)?.0
    };
    if !say(&format!("watching {files} files (Ctrl-C to stop)")) {
        return Ok(());
    }
    let (mut scan_failed, mut update_failed) = (false, false);
    loop {
        thread::sleep(interval);
        let current = match watch::snapshot(root) {
            Ok(current) => current,
            Err(e) => {
                if !root.is_dir() {
                    bail!("project dir {} is gone", root.display());
                }
                // Report a failing scan once, not on every tick.
                if !scan_failed {
                    eprintln!("error: scanning {}: {e:#}", root.display());
                }
                scan_failed = true;
                continue;
            }
        };
        scan_failed = false;
        let changed = watch::changed(&previous, &current);
        if changed.is_empty() {
            continue;
        }
        match update_paths(root, &changed) {
            Ok(failed) => {
                failed.iter().for_each(|f| eprintln!("error: {f}"));
                if !say(&format!("updated {}", changed.join(", "))) {
                    return Ok(());
                }
            }
            Err(e) => {
                // Lock or write failure: keep the old snapshot so the same files are retried
                // on the next tick, but say so only once.
                if !update_failed {
                    eprintln!("error: {e:#}");
                }
                update_failed = true;
                continue;
            }
        }
        update_failed = false;
        // Files that failed individually stay "seen": retried only once they change again.
        previous = current;
    }
}

/// `file` as a `/`-separated path relative to `root`, whether given absolute or relative to the cwd.
fn relative_to(root: &Path, file: &Path) -> Result<String> {
    let root = fs::canonicalize(root).with_context(|| format!("project dir {}", root.display()))?;
    // A relative path is taken from the cwd, unless its folder only exists under the project root.
    let from_root = root.join(file);
    let file = if file.is_relative() && !file.exists() && !cwd_has_folder_of(file) {
        from_root.as_path()
    } else {
        file
    };
    let abs = match fs::canonicalize(file) {
        Ok(p) => p,
        // A deleted file can't be canonicalized; resolve its folder instead.
        Err(_) => {
            let parent = file
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = file.file_name().context("file has no name")?;
            fs::canonicalize(parent)?.join(name)
        }
    };
    let rel = abs
        .strip_prefix(&root)
        .with_context(|| format!("{} is outside {}", file.display(), root.display()))?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

fn cwd_has_folder_of(file: &Path) -> bool {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .is_some_and(Path::exists)
}

fn write_outputs(root: &Path, idx: &Index) -> Result<()> {
    index::save(root, idx)?;
    let map = root.join(".claude").join("codemap.md");
    index::write_atomic(&map, codemap::render(&project_name(root)?, idx).as_bytes())
}

fn project_name(root: &Path) -> Result<String> {
    Ok(fs::canonicalize(root)?.file_name().map_or_else(
        || "project".to_string(),
        |n| n.to_string_lossy().into_owned(),
    ))
}

fn cmd_benchmark(root: &Path, names: &[String], json: bool) -> Result<()> {
    let idx = load(root)?;
    let names: Vec<String> = if names.is_empty() {
        query::hubs(&idx, 5).into_iter().map(|h| h.name).collect()
    } else {
        names.to_vec()
    };
    let outcome = benchmark::run(root, idx.files.len(), &names)?;
    if json {
        println!("{}", serde_json::to_string(&outcome)?);
    } else {
        outcome.lines().iter().for_each(|line| println!("{line}"));
    }
    Ok(())
}

fn cmd_report(root: &Path, json: bool) -> Result<()> {
    let idx = load(root)?;
    if json {
        let found = community::detect(&community::file_graph(&idx));
        println!("{}", serde_json::to_string(&found)?);
        return Ok(());
    }
    let path = root.join(".claude").join("GRAPH_REPORT.md");
    let (text, count) = report::render(&project_name(root)?, &idx);
    index::write_atomic(&path, text.as_bytes())?;
    let noun = if count == 1 {
        "community"
    } else {
        "communities"
    };
    println!("wrote .claude/GRAPH_REPORT.md ({count} {noun})");
    Ok(())
}

fn cmd_html(root: &Path) -> Result<()> {
    let idx = load(root)?;
    let (page, files) = html::render(&project_name(root)?, &idx);
    let path = root.join(".claude").join("graph.html");
    index::write_atomic(&path, page.as_bytes())?;
    println!("wrote .claude/graph.html ({files} files)");
    Ok(())
}

/// Never fails and never blocks the tool call: any problem just prints nothing.
fn cmd_hook(dir: &Path, kind: &str) {
    use std::io::Read;
    let mut input = String::new();
    if io::stdin()
        .take(1 << 20)
        .read_to_string(&mut input)
        .is_err()
    {
        return;
    }
    let project = std::env::var_os("CLAUDE_PROJECT_DIR")
        .filter(|value| !value.is_empty())
        .map_or_else(|| dir.to_path_buf(), PathBuf::from);
    // Read tool calls carry absolute paths, so compare against an absolute project path.
    let project = std::path::absolute(&project).unwrap_or(project);
    let budget = std::env::var("CODEGRAPH_HOOK_NUDGES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(hook::DEFAULT_BUDGET);
    let Some(state) = hook::state_dir() else {
        return;
    };
    if let Some(line) = hook::nudge(kind, &input, &project, &state, budget) {
        // A closed stdout is not worth a panic.
        let _ = io::stdout().write_all(line.as_bytes());
    }
}

fn cmd_docs(root: &Path, action: &DocsAction, json: bool) -> Result<()> {
    match action {
        DocsAction::Pending { max_lines } => {
            let store = semantic::load_store(root)?;
            let pending = semantic::pending(root, &store, *max_lines)?;
            if json {
                println!("{}", serde_json::to_string(&pending)?);
            } else {
                print_pending(&pending);
            }
            Ok(())
        }
        DocsAction::Text { doc } => {
            print!("{}", semantic::doc_text(root, doc)?);
            Ok(())
        }
        DocsAction::Ingest { file } => {
            let raw = read_capped(file)?;
            let idx = load(root)?;
            let _lock = index::lock(root)?;
            let mut store = semantic::load_store(root)?;
            let summary = semantic::ingest(root, &idx, &mut store, &raw)?;
            semantic::save_store(root, &store)?;
            if json {
                println!("{}", serde_json::to_string(&summary)?);
            } else {
                println!(
                    "stored {} concepts, {} links ({} dropped)",
                    summary.concepts, summary.links, summary.dropped
                );
            }
            Ok(())
        }
    }
}

fn print_pending(p: &semantic::Pending) {
    let plan = if p.chunks.is_empty() {
        "inline: the main agent reads them, no subagents".to_string()
    } else {
        format!(
            "{} subagent chunks of at most {} lines",
            p.chunks.len(),
            semantic::CHUNK_LINES
        )
    };
    println!("{} docs, {} lines: {plan}", p.docs.len(), p.total_lines);
    println!("{}", semantic::advice(p.total_lines));
    for (n, chunk) in p.chunks.iter().enumerate() {
        println!("chunk {}: {}", n + 1, chunk.join(" "));
    }
    for d in &p.docs {
        println!("{}  {} lines  {}", d.path, d.lines, d.hash);
    }
}

fn cmd_scip(root: &Path, file: Option<&Path>, json: bool) -> Result<()> {
    let path = file.map_or_else(|| root.join("index.scip"), Path::to_path_buf);
    let data = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let docs = scip::parse(&data).with_context(|| format!("parsing {}", path.display()))?;
    let _lock = index::lock(root)?;
    let mut idx = load(root)?;
    let stats = scip::apply(&mut idx, &docs);
    index::save(root, &idx)?;
    if json {
        println!("{}", serde_json::to_string(&stats)?);
        return Ok(());
    }
    print!(
        "resolved {} of {} calls ({} external)",
        stats.resolved, stats.calls, stats.external
    );
    if stats.unknown_documents > 0 {
        print!(
            "; {} index documents match no indexed file (was it built from another root?)",
            stats.unknown_documents
        );
    }
    if stats.uncovered_files > 0 {
        let files = if stats.uncovered_files == 1 {
            "file"
        } else {
            "files"
        };
        print!(
            "; no index document for {} {files} ({} calls): they keep name matching",
            stats.uncovered_files, stats.uncovered_calls
        );
    }
    println!();
    Ok(())
}

/// Read an agent-written file, never more than the ingest cap (it may be huge or a device).
fn read_capped(file: &Path) -> Result<String> {
    use std::io::Read;
    let mut raw = String::new();
    let limit = semantic::MAX_INGEST_BYTES as u64 + 1;
    fs::File::open(file)
        .with_context(|| format!("opening {}", file.display()))?
        .take(limit)
        .read_to_string(&mut raw)
        .with_context(|| format!("reading {}", file.display()))?;
    Ok(raw)
}

fn load(root: &Path) -> Result<Index> {
    match index::load(root) {
        Some(idx) => Ok(idx),
        None => bail!(
            "no usable index in {} (missing, or built by another codegraph version); run `codegraph index` first",
            root.display()
        ),
    }
}

/// One line per result in text mode, a single JSON array with `--json`.
fn print_results<T: Serialize>(items: &[T], json: bool, text: fn(&T) -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(items)?);
    } else {
        for item in items {
            println!("{}", text(item));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_broken_pipe;

    #[test]
    fn only_a_closed_stdout_counts_as_a_broken_pipe() {
        assert!(is_broken_pipe(
            "panicked at stdio.rs:1168:9:\nfailed printing to stdout: Broken pipe (os error 32)"
        ));
        assert!(!is_broken_pipe("panicked: index out of bounds"));
        assert!(!is_broken_pipe(
            "failed printing to stdout: No space left on device"
        ));
    }
}
