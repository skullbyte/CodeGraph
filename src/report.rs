//! Renders `GRAPH_REPORT.md`: key modules, hubs, surprising links and communities.
//!
//! The "surprising links" scoring is ported from Graphify's `analyze.py` (Apache-2.0,
//! safishamsi/graphify).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use crate::community::{self, Community, FileGraph};
use crate::model::Index;
use crate::query;

/// Communities with fewer files than this are counted but not listed.
const MIN_FILES: usize = 3;
const TOP: usize = 10;
const SURPRISES: usize = 5;
const FILES_SHOWN: usize = 8;

/// Returns the report text and the number of communities it covers.
pub fn render(project: &str, index: &Index) -> (String, usize) {
    let graph = community::file_graph(index);
    let found = community::detect(&graph);
    let degree = degrees(&graph);
    let shown: Vec<&Community> = found
        .iter()
        .filter(|c| c.files.len() >= MIN_FILES)
        .collect();
    let pos: HashMap<&str, usize> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let (symbols, _) = index.code_totals();
    let mut out = format!("# Graph Report - {project}\n\n## Summary\n");
    let _ = writeln!(
        out,
        "- {} files · {symbols} symbols · {} file links · {} communities ({} shown, {} thin omitted)",
        graph.files.len(),
        graph.edges.len(),
        found.len(),
        shown.len(),
        found.len() - shown.len(),
    );
    out.push_str("\n## Key modules (most-connected files)\n");
    for (rank, (file, links)) in top_files(&graph, &degree).into_iter().enumerate() {
        let _ = writeln!(out, "{}. `{file}` - {links} links", rank + 1);
    }
    out.push_str("\n## Hubs (most-connected symbols)\n");
    for (rank, hub) in query::hubs(index, TOP).iter().enumerate() {
        let _ = writeln!(
            out,
            "{}. `{}` - {} - in={} out={}",
            rank + 1,
            hub.name,
            hub.location,
            hub.incoming,
            hub.outgoing
        );
    }
    out.push_str("\n## Surprising links (calls across communities)\n");
    for line in surprises(index, &graph, &found, &degree) {
        let _ = writeln!(out, "{line}");
    }
    out.push_str("\n## Communities\n");
    for c in shown {
        let _ = write!(out, "\n{}", community_block(c, &pos, &degree));
    }
    (out, found.len())
}

/// File index -> number of distinct files it is linked with.
fn degrees(graph: &FileGraph) -> Vec<usize> {
    let mut degree = vec![0; graph.files.len()];
    for &(a, b) in graph.edges.keys() {
        degree[a] += 1;
        degree[b] += 1;
    }
    degree
}

fn top_files<'a>(graph: &'a FileGraph, degree: &[usize]) -> Vec<(&'a str, usize)> {
    let mut files: Vec<(&str, usize)> = graph
        .files
        .iter()
        .map(String::as_str)
        .zip(degree.iter().copied())
        .filter(|&(_, links)| links > 0)
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    files.truncate(TOP);
    files
}

fn community_block(c: &Community, pos: &HashMap<&str, usize>, degree: &[usize]) -> String {
    let mut label = c.files[0].as_str();
    for file in &c.files {
        if degree[pos[file.as_str()]] > degree[pos[label]] {
            label = file;
        }
    }
    let listed = c
        .files
        .iter()
        .take(FILES_SHOWN)
        .cloned()
        .collect::<Vec<_>>();
    let more = c.files.len().saturating_sub(FILES_SHOWN);
    let tail = if more > 0 {
        format!(", (+{more} more)")
    } else {
        String::new()
    };
    format!(
        "### Community {} - {label}\nCohesion: {:.2}\nFiles ({}): {}{tail}\n",
        c.id,
        c.cohesion,
        c.files.len(),
        listed.join(", ")
    )
}

/// Graphify's score: +2 other top-level folder, +1 other community, +1 when a peripheral file
/// (<= 2 links) reaches a hub (>= 5 links). The best link per community pair, top few overall.
fn surprises(
    index: &Index,
    graph: &FileGraph,
    found: &[Community],
    degree: &[usize],
) -> Vec<String> {
    let community_of: HashMap<&str, usize> = found
        .iter()
        .flat_map(|c| c.files.iter().map(move |f| (f.as_str(), c.id)))
        .collect();
    let folder = |path: &str| path.split_once('/').map_or("", |(top, _)| top).to_string();
    let mut best: BTreeMap<(usize, usize), (u32, String, String)> = BTreeMap::new();
    for site in community::call_sites(index, &graph.files) {
        let Some(caller) = &site.caller else { continue };
        let (from, to) = (&graph.files[site.from], &graph.files[site.to]);
        let (ca, cb) = (community_of[from.as_str()], community_of[to.as_str()]);
        if ca == cb {
            continue;
        }
        let (lo, hi) = (
            degree[site.from].min(degree[site.to]),
            degree[site.from].max(degree[site.to]),
        );
        let score = 1 + 2 * u32::from(folder(from) != folder(to)) + u32::from(lo <= 2 && hi >= 5);
        let to_line = index.files[to]
            .symbols
            .iter()
            .find(|s| s.name == site.callee)
            .map_or(1, |s| s.line);
        let line = format!(
            "- `{}` --calls--> `{}`  {from}:{} → {to}:{to_line}",
            query::bare_name(caller),
            site.callee,
            site.line
        );
        let entry =
            best.entry((ca.min(cb), ca.max(cb)))
                .or_insert((0, String::new(), String::new()));
        if (score, std::cmp::Reverse(&line)) > (entry.0, std::cmp::Reverse(&entry.2)) {
            *entry = (score, from.clone(), line);
        }
    }
    let mut ranked: Vec<_> = best.into_values().collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    ranked
        .into_iter()
        .take(SURPRISES)
        .map(|(_, _, line)| line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{self, Lang};
    use crate::model::FileEntry;

    fn index_of(files: &[(&str, &str)]) -> Index {
        let mut index = Index::default();
        for (path, src) in files {
            let facts = lang::extract(Lang::from_path(path).unwrap(), src).unwrap();
            index.files.insert(
                path.to_string(),
                FileEntry {
                    path: path.to_string(),
                    symbols: facts.symbols,
                    imports: facts.imports,
                    calls: facts.calls,
                    ..FileEntry::default()
                },
            );
        }
        index
    }

    fn two_clusters() -> Index {
        index_of(&[
            ("x/a1.rs", "fn a1() { a2(); a3(); }\n"),
            ("x/a2.rs", "fn a2() { a3(); a1(); }\n"),
            ("x/a3.rs", "fn a3() { a1(); b1(); }\n"),
            ("y/b1.rs", "fn b1() { b2(); b3(); }\n"),
            ("y/b2.rs", "fn b2() { b3(); b1(); }\n"),
            ("y/b3.rs", "fn b3() { b1(); b2(); }\n"),
        ])
    }

    fn section<'a>(report: &'a str, title: &str) -> &'a str {
        let start = report.find(title).unwrap_or_else(|| panic!("no {title}"));
        let rest = &report[start + title.len()..];
        let rest = &rest[rest.find('\n').unwrap_or(0)..];
        &rest[..rest.find("\n## ").unwrap_or(rest.len())]
    }

    #[test]
    fn summary_counts_files_symbols_and_communities() {
        let (report, count) = render("demo", &two_clusters());
        assert_eq!(count, 2);
        assert!(report.starts_with("# Graph Report - demo\n"));
        let summary = section(&report, "## Summary");
        assert!(summary.contains("6 files · 6 symbols"), "{summary}");
        assert!(
            summary.contains("2 communities (2 shown, 0 thin omitted)"),
            "{summary}"
        );
    }

    #[test]
    fn key_modules_rank_files_by_distinct_neighbours() {
        let report = render("demo", &two_clusters()).0;
        let modules = section(&report, "## Key modules");
        assert!(modules.contains("1. `x/a3.rs` - 3 links"), "{modules}");
    }

    #[test]
    fn hubs_section_reuses_symbol_hubs() {
        let report = render("demo", &two_clusters()).0;
        assert!(section(&report, "## Hubs").contains("`a1`"));
    }

    #[test]
    fn surprising_links_are_cross_community_calls() {
        let report = render("demo", &two_clusters()).0;
        let links = section(&report, "## Surprising links");
        assert_eq!(
            links.trim(),
            "- `a3` --calls--> `b1`  x/a3.rs:1 → y/b1.rs:1",
            "{links}"
        );
    }

    #[test]
    fn communities_list_label_cohesion_and_files() {
        let report = render("demo", &two_clusters()).0;
        let communities = section(&report, "## Communities");
        assert!(
            communities.contains("### Community 0 - x/a3.rs"),
            "{communities}"
        );
        assert!(communities.contains("Cohesion: 1.00"), "{communities}");
        assert!(
            communities.contains("Files (3): x/a1.rs, x/a2.rs, x/a3.rs"),
            "{communities}"
        );
    }

    #[test]
    fn thin_communities_are_counted_but_not_listed() {
        let mut files = vec![
            ("x/a1.rs", "fn a1() { a2(); }\n"),
            ("x/a2.rs", "fn a2() { a3(); }\n"),
            ("x/a3.rs", "fn a3() { a1(); }\n"),
        ];
        files.push(("p.rs", "fn p() { q(); }\n"));
        files.push(("q.rs", "fn q() {}\n"));
        let report = render("demo", &index_of(&files)).0;
        assert!(
            report.contains("2 communities (1 shown, 1 thin omitted)"),
            "{report}"
        );
        assert!(!report.contains("p.rs, q.rs"));
    }

    #[test]
    fn long_file_lists_are_cut_at_eight() {
        let sources: Vec<(String, String)> = (0..10)
            .map(|i| {
                let calls: String = (0..10)
                    .filter(|&j| j != i)
                    .map(|j| format!("f{j}(); "))
                    .collect();
                (format!("m{i}.rs"), format!("fn f{i}() {{ {calls}}}\n"))
            })
            .collect();
        let refs: Vec<(&str, &str)> = sources
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let report = render("demo", &index_of(&refs)).0;
        let communities = section(&report, "## Communities");
        assert!(communities.contains("(+"), "{communities}");
    }

    #[test]
    fn empty_index_still_renders() {
        let (report, count) = render("demo", &Index::default());
        assert_eq!(count, 0);
        assert!(section(&report, "## Summary").contains("0 files"));
    }
}
