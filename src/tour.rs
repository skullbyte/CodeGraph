//! `codegraph tour`: a reading order for a project, built from the graph alone (no AI tokens).
//! Entry files first, then the best-connected file of each community, one line each:
//! the first line of the file's top doc comment, or its most-called symbols.
//! Ours, not in Graphify.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path};

use serde::Serialize;

use crate::community;
use crate::model::{FileEntry, Index};
use crate::semantic;

pub const DEFAULT_TOP: usize = 10;
pub const MAX_TOP: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stop {
    pub path: String,
    /// What the file is for, in a few words.
    pub summary: String,
}

impl Stop {
    pub fn text(&self, number: usize) -> String {
        format!("{number}. {}  {}", self.path, self.summary)
    }
}

/// Names that usually start a program or a package.
const ENTRY_NAMES: [&str; 14] = [
    "main.rs",
    "lib.rs",
    "index.ts",
    "index.tsx",
    "index.js",
    "main.ts",
    "main.tsx",
    "main.js",
    "app.ts",
    "__main__.py",
    "main.py",
    "main.go",
    "Program.cs",
    "Main.java",
];
const SUMMARY_CHARS: usize = 100;
const DOC_SCAN_LINES: usize = 25;

/// Entry files (shallowest first, up to a third of `top`), then the best-connected file of each
/// community not chosen yet, biggest community first.
pub fn tour(root: &Path, index: &Index, top: usize) -> Vec<Stop> {
    let top = top.clamp(1, MAX_TOP);
    let mut chosen: Vec<String> = entry_files(index, (top / 3).max(1));
    let graph = community::file_graph(index);
    let mut degree = vec![0usize; graph.files.len()];
    for &(a, b) in graph.edges.keys() {
        degree[a] += 1;
        degree[b] += 1;
    }
    let at: HashMap<&str, usize> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    for c in community::detect(&graph) {
        let best = c
            .files
            .iter()
            .filter(|f| {
                !chosen.contains(f) && index.files.get(*f).is_some_and(|e| worth_reading(f, e))
            })
            .max_by(|a, b| {
                degree[at[a.as_str()]]
                    .cmp(&degree[at[b.as_str()]])
                    .then_with(|| b.cmp(a))
            });
        if let Some(file) = best {
            chosen.push(file.clone());
        }
    }
    chosen.truncate(top);
    let calls = call_counts(index);
    chosen
        .into_iter()
        .map(|path| {
            let summary = summarize(root, index, &path, &calls);
            Stop { path, summary }
        })
        .collect()
}

/// Folders that hold tests, benchmarks, examples and fixtures rather than the product.
const SIDE: [&str; 8] = [
    "tests",
    "test",
    "__tests__",
    "fixtures",
    "benches",
    "bench",
    "examples",
    "node_modules",
];

/// Is this path test, benchmark, example or fixture code? By folder (`tests/`) or by file name
/// (`foo_test.go`, `test_foo.py`, `FooTest.kt`, `FooTests.swift`, `foo_spec.rb`, `a.test.ts`).
pub(crate) fn is_side_path(path: &str) -> bool {
    let (dirs, file) = path.rsplit_once('/').unwrap_or(("", path));
    if dirs.split('/').any(|part| SIDE.contains(&part)) {
        return true;
    }
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let named = |suffix: &str| {
        stem.strip_suffix(suffix)
            .is_some_and(|rest| !rest.is_empty())
    };
    file.contains(".test.")
        || file.contains(".spec.")
        || stem.starts_with("test_")
        || named("_test")
        || named("_spec")
        || named("Test")
        || named("Tests")
}

/// Code with at least one definition, outside tests, benchmarks, examples and fixtures.
fn worth_reading(path: &str, entry: &FileEntry) -> bool {
    !entry.is_doc() && entry.symbols.iter().any(|s| s.kind.is_code_def()) && !is_side_path(path)
}

fn entry_files(index: &Index, limit: usize) -> Vec<String> {
    let mut found: Vec<&String> = index
        .files
        .iter()
        .filter(|(path, e)| {
            worth_reading(path, e) && ENTRY_NAMES.contains(&path.rsplit('/').next().unwrap_or(path))
        })
        .map(|(path, _)| path)
        .collect();
    found.sort_by_key(|p| (p.matches('/').count(), p.to_string()));
    found.into_iter().take(limit).cloned().collect()
}

/// Callee name -> how many calls name it, over the whole project.
fn call_counts(index: &Index) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for (call, _) in index.files.values().flat_map(|f| f.graph_calls()) {
        *counts.entry(call.callee.as_str()).or_default() += 1;
    }
    counts
}

fn summarize(root: &Path, index: &Index, path: &str, calls: &HashMap<&str, usize>) -> String {
    if let Some(line) = top_doc_line(root, path) {
        return line;
    }
    let mut names: Vec<&str> = index
        .files
        .get(path)
        .map(|e| {
            e.symbols
                .iter()
                .filter(|s| s.kind.is_code_def())
                .map(|s| s.name.as_str())
                .collect()
        })
        .unwrap_or_default();
    names.sort_by(|a, b| {
        calls
            .get(b)
            .unwrap_or(&0)
            .cmp(calls.get(a).unwrap_or(&0))
            .then(a.cmp(b))
    });
    names.dedup();
    names.truncate(3);
    if names.is_empty() {
        "no symbols".to_string()
    } else {
        format!("defines {}", names.join(", "))
    }
}

/// Read at most this much of a file to find its top comment (one minified line can be huge).
const DOC_SCAN_BYTES: u64 = 64 * 1024;

/// First line of the doc comment at the top of the file, if there is one. Only regular files
/// inside the project are opened (no symlinks, FIFOs, `..` or absolute paths).
fn top_doc_line(root: &Path, path: &str) -> Option<String> {
    let text = read_head(root, path)?;
    let lines: Vec<&str> = text.lines().take(DOC_SCAN_LINES).collect();
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    let found = first_doc_text(&lines, ext == "py")?;
    let plain = semantic::clean(&found);
    (!plain.is_empty()).then(|| plain.chars().take(SUMMARY_CHARS).collect())
}

/// The first `DOC_SCAN_BYTES` of a project file as text, or `None` for anything that is not a plain
/// regular file inside `root`.
pub(crate) fn read_head(root: &Path, path: &str) -> Option<String> {
    let relative = Path::new(path);
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let full = root.join(relative);
    if !std::fs::symlink_metadata(&full).ok()?.file_type().is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&full)
        .ok()?
        .take(DOC_SCAN_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Comments that tell tools something instead of describing the file.
const DIRECTIVES: [&str; 8] = [
    "go:", "+build", "-*-", "eslint", "@ts-", "nolint", "!/", "@flow",
];

/// The first descriptive line of the leading comment. Python uses `#` and docstrings; every other
/// supported language uses `//` and `/* */` (a `#` there is a preprocessor line or an attribute).
pub(crate) fn first_doc_text(lines: &[&str], python: bool) -> Option<String> {
    let mut in_docstring = false;
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("#!") {
            continue;
        }
        let text = if in_docstring {
            let (before, closed) = until_quotes(line);
            in_docstring = !closed;
            if closed && before.trim().is_empty() {
                return None; // the docstring ended without a description
            }
            before.to_string()
        } else if python && (line.starts_with("\"\"\"") || line.starts_with("'''")) {
            let (before, closed) = until_quotes(&line[3..]);
            in_docstring = !closed;
            before.to_string()
        } else if python && line.starts_with('#') {
            line.trim_start_matches('#').to_string()
        } else if !python {
            comment_text(line)?
        } else {
            return None; // code starts here
        };
        let text = strip_tag(text.trim());
        let lower = text.to_lowercase();
        let skip = lower.starts_with("copyright")
            || lower.contains("licensed under")
            || lower.starts_with("spdx-")
            || DIRECTIVES.iter().any(|d| lower.starts_with(d));
        if !text.is_empty() && !skip {
            return Some(text.to_string());
        }
    }
    None
}

/// Text before the closing quotes of a docstring, and whether they were found.
fn until_quotes(line: &str) -> (&str, bool) {
    ["\"\"\"", "'''"]
        .iter()
        .filter_map(|q| line.find(q))
        .min()
        .map_or((line, false), |at| (&line[..at], true))
}

/// The text of a `//`, `///`, `//!`, `/*`, `/**` or ` * ` comment line; `None` when the line is code
/// or the end of a block comment.
fn comment_text(line: &str) -> Option<String> {
    for marker in ["//!", "///", "//", "/**", "/*"] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim_end_matches("*/").to_string());
        }
    }
    let inside = line.strip_prefix('*').filter(|_| !line.starts_with("*/"))?;
    Some(inside.to_string())
}

/// `@fileoverview Foo` -> `Foo`.
fn strip_tag(text: &str) -> &str {
    for tag in ["@fileoverview", "@file", "@module"] {
        if let Some(rest) = text.strip_prefix(tag) {
            return rest.trim();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
        let (idx, _) = index::build(dir.path(), None).unwrap();
        (dir, idx)
    }

    fn paths(stops: &[Stop]) -> Vec<&str> {
        stops.iter().map(|s| s.path.as_str()).collect()
    }

    #[test]
    fn entry_files_come_first_then_one_file_per_community() {
        let (dir, idx) = project(&[
            ("src/util.rs", "pub fn helper() {}\n"),
            ("src/main.rs", "fn main() { helper(); }\n"),
            ("other/solo.rs", "pub fn alone() {}\n"),
        ]);
        let stops = tour(dir.path(), &idx, 10);
        assert_eq!(paths(&stops)[0], "src/main.rs");
        assert_eq!(stops.len(), 3, "{stops:?}");
        let unique: std::collections::BTreeSet<_> = paths(&stops).into_iter().collect();
        assert_eq!(unique.len(), 3, "no file twice");
    }

    #[test]
    fn the_summary_is_the_top_doc_comment_in_each_language() {
        let (dir, idx) = project(&[
            ("a.rs", "//! Builds the graph.\n//! More.\nfn a() {}\n"),
            (
                "b.py",
                "\"\"\"Reads the config.\n\nDetails.\"\"\"\ndef b(): pass\n",
            ),
            (
                "c.ts",
                "/**\n * Draws the cart.\n */\nexport function c() {}\n",
            ),
            (
                "d.go",
                "// Package d serves requests.\npackage d\nfunc D() {}\n",
            ),
        ]);
        let stops = tour(dir.path(), &idx, 10);
        let summary = |p: &str| stops.iter().find(|s| s.path == p).unwrap().summary.clone();
        assert_eq!(summary("a.rs"), "Builds the graph.");
        assert_eq!(summary("b.py"), "Reads the config.");
        assert_eq!(summary("c.ts"), "Draws the cart.");
        assert_eq!(summary("d.go"), "Package d serves requests.");
    }

    #[test]
    fn without_a_doc_comment_it_names_the_most_called_symbols() {
        let (dir, idx) = project(&[(
            "a.rs",
            "fn busy() {}\nfn quiet() {}\nfn x() { busy(); busy(); quiet(); }\n",
        )]);
        let stops = tour(dir.path(), &idx, 10);
        assert_eq!(stops[0].summary, "defines busy, quiet, x");
    }

    #[test]
    fn docs_are_not_stops_and_top_caps_the_list() {
        let (dir, idx) = project(&[
            ("README.md", "# Title\n"),
            ("a.rs", "fn a() {}\n"),
            ("b.rs", "fn b() {}\n"),
            ("c.rs", "fn c() {}\n"),
        ]);
        let stops = tour(dir.path(), &idx, 2);
        assert_eq!(stops.len(), 2);
        assert!(!paths(&stops).contains(&"README.md"));
    }

    #[test]
    fn text_is_numbered() {
        let stop = Stop {
            path: "a.rs".into(),
            summary: "x".into(),
        };
        assert_eq!(stop.text(3), "3. a.rs  x");
    }

    #[test]
    fn tests_configs_and_attributes_do_not_make_stops_or_summaries() {
        let (dir, idx) = project(&[
            (
                "src/app.rs",
                "#[derive(Debug)]\npub struct App;\nfn run() {}\n",
            ),
            ("tests/it.rs", "fn check() {}\n"),
            ("benches/b.rs", "fn bench() {}\n"),
            ("conf.mjs", "export default {};\n"),
        ]);
        let stops = tour(dir.path(), &idx, 10);
        assert_eq!(paths(&stops), ["src/app.rs"], "{stops:?}");
        assert!(
            stops[0].summary.starts_with("defines"),
            "{}",
            stops[0].summary
        );
    }

    fn summary_of(file: &str, body: &str) -> String {
        let (dir, idx) = project(&[(file, body)]);
        tour(dir.path(), &idx, 5).remove(0).summary
    }

    #[test]
    fn directives_preprocessor_lines_and_licences_are_not_summaries() {
        assert!(
            summary_of("a.h", "#include <stdio.h>\nint f(void) { return 1; }\n")
                .starts_with("defines")
        );
        assert!(
            summary_of("a.go", "//go:build linux\n\npackage a\nfunc A() {}\n")
                .starts_with("defines")
        );
        assert!(
            summary_of("a.ts", "// eslint-disable\nexport function a() {}\n")
                .starts_with("defines")
        );
        assert_eq!(
            summary_of(
                "a.py",
                "# -*- coding: utf-8 -*-\n# Loads the data.\ndef a(): pass\n"
            ),
            "Loads the data."
        );
        assert!(
            summary_of(
                "a.py",
                "\"\"\"\nCopyright 2020\n\"\"\"\nimport os\ndef a(): pass\n"
            )
            .starts_with("defines")
        );
        assert_eq!(
            summary_of("a.rs", "// License checks for the API\nfn a() {}\n"),
            "License checks for the API"
        );
        assert_eq!(
            summary_of(
                "a.js",
                "/** @fileoverview Draws /path/x */\nfunction a() {}\n"
            ),
            "Draws /path/x"
        );
    }

    #[test]
    fn crlf_non_utf8_and_deleted_files_fall_back_to_symbols() {
        assert_eq!(
            summary_of("a.rs", "//! Windows file.\r\nfn a() {}\r\n"),
            "Windows file."
        );
        let (dir, idx) = project(&[("b.rs", "fn b() {}\n")]);
        fs::write(dir.path().join("b.rs"), b"\xff\xfe not text\nfn b() {}\n").unwrap();
        assert_eq!(tour(dir.path(), &idx, 5)[0].summary, "defines b");
        fs::remove_file(dir.path().join("b.rs")).unwrap();
        assert_eq!(tour(dir.path(), &idx, 5)[0].summary, "defines b");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_replaced_by_a_symlink_is_not_read() {
        let (dir, idx) = project(&[("a.rs", "fn a() {}\n")]);
        let secret = dir.path().join("elsewhere.txt");
        fs::write(&secret, "// leaked text\n").unwrap();
        fs::remove_file(dir.path().join("a.rs")).unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("a.rs")).unwrap();
        assert_eq!(tour(dir.path(), &idx, 5)[0].summary, "defines a");
    }

    #[test]
    fn control_characters_never_reach_the_summary_and_a_zero_top_gives_one_stop() {
        assert_eq!(
            summary_of("a.rs", "//! Bad \u{1b}[31mred\nfn a() {}\n"),
            "Bad [31mred"
        );
        let (dir, idx) = project(&[("a.rs", "fn a() {}\n"), ("b.rs", "fn b() {}\n")]);
        assert_eq!(tour(dir.path(), &idx, 0).len(), 1);
    }

    #[test]
    fn the_biggest_community_is_listed_first() {
        let (dir, idx) = project(&[
            ("small1.rs", "pub fn s_one() { s_two(); }\n"),
            ("small2.rs", "pub fn s_two() {}\n"),
            ("big1.rs", "pub fn b_one() { b_two(); }\n"),
            ("big2.rs", "pub fn b_two() { b_three(); }\n"),
            ("big3.rs", "pub fn b_three() {}\n"),
        ]);
        let stops = tour(dir.path(), &idx, 2);
        assert!(stops[0].path.starts_with("big"), "{stops:?}");
        assert!(stops[1].path.starts_with("small"), "{stops:?}");
    }

    #[test]
    fn side_paths_are_tests_benches_examples_and_test_named_files() {
        for side in [
            "tests/a.rs",
            "src/__tests__/x.ts",
            "bench/b.rs",
            "node_modules/x/y.js",
            "pkg/foo_test.go",
            "pkg/test_foo.py",
            "app/FooTest.kt",
            "app/FooTests.swift",
            "spec/foo_spec.rb",
            "src/a.test.ts",
            "src/a.spec.tsx",
        ] {
            assert!(is_side_path(side), "{side}");
        }
        for code in [
            "src/latest.rs",
            "src/contest.go",
            "src/protest/a.py",
            "src/Test.kt",
            "src/testing.rs",
        ] {
            assert!(!is_side_path(code), "{code}");
        }
    }
}
