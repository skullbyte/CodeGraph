//! codegraph command line: parses arguments, calls the library, prints results.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;

use codegraph::model::Index;
use codegraph::query::{Affected, CallerHit, DefHit, Hop, Hub};
use codegraph::{
    benchmark, codemap, community, html, index, mermaid, obsidian, query, report, watch,
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
    /// Where is <name> defined?
    Def { name: String },
    /// Who calls <name>?
    Callers { name: String },
    /// Shortest call chain (function names) or import chain (file paths) from A to B.
    Path { from: String, to: String },
    /// Re-index one file.
    Update { file: PathBuf },
    /// Definition, callers and callees of <name>.
    Explain { name: String },
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

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Index { root } => {
            let root = root.as_ref().unwrap_or(&cli.dir);
            cmd_index(root)
        }
        Command::Def { name } => {
            print_results(&query::def(&load(&cli.dir)?, name), cli.json, DefHit::text)
        }
        Command::Callers { name } => print_results(
            &query::callers(&load(&cli.dir)?, name),
            cli.json,
            CallerHit::text,
        ),
        Command::Path { from, to } => {
            let hops = query::path(&load(&cli.dir)?, from, to);
            if hops.is_empty() && !cli.json {
                println!("no path");
                return Ok(());
            }
            print_results(&hops, cli.json, Hop::text)
        }
        Command::Update { file } => cmd_update(&cli.dir, file),
        Command::Explain { name } => {
            let explained = query::explain(&load(&cli.dir)?, name);
            if cli.json {
                println!("{}", serde_json::to_string(&explained)?);
            } else {
                explained.lines().iter().for_each(|line| println!("{line}"));
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
