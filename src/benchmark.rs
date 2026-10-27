//! `codegraph benchmark`: codegraph vs Graphify on index time and `explain` answers.
//!
//! Tokens are estimated as chars / 4, the same rule Graphify's `benchmark.py` uses
//! (Apache-2.0, safishamsi/graphify).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use crate::index;

/// One side of a query comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub tokens: usize,
    pub ms: u128,
    /// `ok`, `ambiguous`, `not found` or `error`.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub name: String,
    pub codegraph: Answer,
    /// `None` when Graphify is not installed.
    pub graphify: Option<Answer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub files: usize,
    pub codegraph_index_ms: u128,
    pub graphify_index_ms: Option<u128>,
    pub rows: Vec<Row>,
}

/// Rough token count: one token per 4 characters, at least 1.
pub fn tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

/// Maps a `graphify explain` exit code: 0 answer, 3 ambiguous name, 4 no match.
pub fn graphify_status(code: Option<i32>) -> &'static str {
    match code {
        Some(0) => "ok",
        Some(3) => "ambiguous",
        Some(4) => "not found",
        _ => "error",
    }
}

impl Answer {
    fn text(&self) -> String {
        format!("{} tok {} ms {}", self.tokens, self.ms, self.status)
    }
}

impl Row {
    pub fn text(&self) -> String {
        let mut line = format!("explain {}  codegraph {}", self.name, self.codegraph.text());
        if let Some(g) = &self.graphify {
            line.push_str(&format!(" | graphify {}", g.text()));
        }
        line
    }
}

impl Outcome {
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("files {}", self.files)];
        match self.graphify_index_ms {
            Some(g) => lines.push(format!(
                "index  codegraph {} ms | graphify {g} ms",
                self.codegraph_index_ms
            )),
            None => {
                lines.push(format!("index  codegraph {} ms", self.codegraph_index_ms));
                lines.push("graphify: not found".to_string());
            }
        }
        lines.extend(self.rows.iter().map(Row::text));
        lines
    }
}

/// Times a cold index, then `explain` for each name; Graphify runs only if it is installed.
/// The `explain` runs read the index saved in `root`, which can be older than the cold build timed
/// here: run `codegraph index` first for matching answers.
pub fn run(root: &Path, files: usize, names: &[String]) -> Result<Outcome> {
    let start = Instant::now();
    index::build(root, None)?;
    let codegraph_index_ms = start.elapsed().as_millis();

    let me = std::env::current_exe()?;
    let work = private_dir()?;
    let graph = work.join("graphify-out").join("graph.json");
    let graphify_index_ms = graphify_extract(root, &work).filter(|_| graph.is_file());

    let mut rows = Vec::new();
    for name in names {
        let mut cmd = Command::new(&me);
        cmd.args(["explain", "--dir"]).arg(root).args(["--", name]);
        let codegraph = timed(&mut cmd).map_or_else(
            || Answer {
                tokens: 1,
                ms: 0,
                status: "error".to_string(),
            },
            |(out, ms)| {
                let text = String::from_utf8_lossy(&out.stdout);
                let status = if text.trim().is_empty() {
                    "not found"
                } else {
                    "ok"
                };
                Answer {
                    tokens: tokens(&text),
                    ms,
                    status: status.to_string(),
                }
            },
        );
        let graphify = graphify_index_ms.map(|_| {
            let mut cmd = Command::new("graphify");
            cmd.args(["explain", name, "--graph"]).arg(&graph);
            timed(&mut cmd).map_or_else(
                || Answer {
                    tokens: 1,
                    ms: 0,
                    status: "error".to_string(),
                },
                |(out, ms)| Answer {
                    tokens: tokens(&String::from_utf8_lossy(&out.stdout)),
                    ms,
                    status: graphify_status(out.status.code()).to_string(),
                },
            )
        });
        rows.push(Row {
            name: name.clone(),
            codegraph,
            graphify,
        });
    }
    let _ = fs::remove_dir_all(&work);
    Ok(Outcome {
        files,
        codegraph_index_ms,
        graphify_index_ms,
        rows,
    })
}

/// A new empty directory in the temp dir; `create_dir` fails if the name already exists (even as a
/// symlink), so nothing planted there is ever followed.
fn private_dir() -> Result<PathBuf> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("codegraph-bench-{}-{nanos}", std::process::id()));
    fs::create_dir(&dir)?;
    Ok(dir)
}

/// Milliseconds Graphify took to extract `root` into `work`; `None` if it is missing or failed.
fn graphify_extract(root: &Path, work: &Path) -> Option<u128> {
    let mut cmd = Command::new("graphify");
    cmd.args(["extract"])
        .arg(root)
        .args(["--code-only", "--force", "--out"])
        .arg(work)
        .env("GRAPHIFY_BACKEND", "none")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (out, ms) = timed(&mut cmd)?;
    out.status.success().then_some(ms)
}

/// Runs the command to completion; `None` if it could not be started.
fn timed(cmd: &mut Command) -> Option<(Output, u128)> {
    let start = Instant::now();
    let out = cmd.output().ok()?;
    Some((out, start.elapsed().as_millis()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(tokens: usize, ms: u128, status: &str) -> Answer {
        Answer {
            tokens,
            ms,
            status: status.to_string(),
        }
    }

    #[test]
    fn tokens_are_chars_over_four_and_at_least_one() {
        assert_eq!(tokens(""), 1);
        assert_eq!(tokens("abc"), 1);
        assert_eq!(tokens("abcdefgh"), 2);
        assert_eq!(tokens("ñññññññññ"), 2);
    }

    #[test]
    fn graphify_exit_codes_become_statuses() {
        assert_eq!(graphify_status(Some(0)), "ok");
        assert_eq!(graphify_status(Some(3)), "ambiguous");
        assert_eq!(graphify_status(Some(4)), "not found");
        assert_eq!(graphify_status(Some(1)), "error");
        assert_eq!(graphify_status(None), "error");
    }

    #[test]
    fn row_shows_both_sides() {
        let row = Row {
            name: "notify".to_string(),
            codegraph: answer(120, 2, "ok"),
            graphify: Some(answer(900, 410, "ambiguous")),
        };
        assert_eq!(
            row.text(),
            "explain notify  codegraph 120 tok 2 ms ok | graphify 900 tok 410 ms ambiguous"
        );
    }

    #[test]
    fn row_without_graphify_shows_codegraph_only() {
        let row = Row {
            name: "notify".to_string(),
            codegraph: answer(120, 2, "ok"),
            graphify: None,
        };
        assert_eq!(row.text(), "explain notify  codegraph 120 tok 2 ms ok");
    }

    #[test]
    fn outcome_lines_start_with_files_and_index_times() {
        let outcome = Outcome {
            files: 81,
            codegraph_index_ms: 12,
            graphify_index_ms: Some(553),
            rows: vec![],
        };
        assert_eq!(
            outcome.lines(),
            vec!["files 81", "index  codegraph 12 ms | graphify 553 ms"]
        );
    }

    #[test]
    fn outcome_without_graphify_says_so() {
        let outcome = Outcome {
            files: 3,
            codegraph_index_ms: 1,
            graphify_index_ms: None,
            rows: vec![],
        };
        assert_eq!(
            outcome.lines(),
            vec!["files 3", "index  codegraph 1 ms", "graphify: not found"]
        );
    }
}
