//! Groups files into communities (clusters of related code) using Louvain modularity.
//!
//! Louvain follows the approach of Graphify's `cluster.py` (Apache-2.0, safishamsi/graphify):
//! deterministic ordering, communities ordered by size, cohesion = intra edges / possible edges.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::model::{Index, Kind};
use crate::query;

/// Undirected, weighted graph of files; `edges` keys are `(i, j)` with `i < j`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileGraph {
    pub files: Vec<String>,
    pub edges: BTreeMap<(usize, usize), u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Community {
    pub id: usize,
    pub files: Vec<String>,
    /// Linked file pairs inside the community / all possible pairs (1.0 for a single file).
    pub cohesion: f64,
}

/// A call whose callee is defined in exactly one project file; `from` and `to` index `files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub from: usize,
    pub to: usize,
    pub line: u32,
    /// Signature of the enclosing function; `None` means top level.
    pub caller: Option<String>,
    pub callee: String,
}

/// Calls to names defined in one file only: names defined in several files are too ambiguous.
pub fn call_sites(index: &Index, files: &[String]) -> Vec<CallSite> {
    let mut homes: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, file) in files.iter().enumerate() {
        for s in index.files[file]
            .symbols
            .iter()
            .filter(|s| s.kind != Kind::Impl)
        {
            let home = homes.entry(s.name.as_str()).or_default();
            if !home.contains(&i) {
                home.push(i);
            }
        }
    }
    let mut sites = Vec::new();
    for (from, file) in files.iter().enumerate() {
        for call in &index.files[file].calls {
            let Some(&[to]) = homes.get(call.callee.as_str()).map(Vec::as_slice) else {
                continue;
            };
            sites.push(CallSite {
                from,
                to,
                line: call.line,
                caller: call.caller.clone(),
                callee: call.callee.clone(),
            });
        }
    }
    sites
}

/// Files linked by resolved imports (+1 each) and by calls to names defined in one file only (+1
/// per call site).
pub fn file_graph(index: &Index) -> FileGraph {
    let files: Vec<String> = index.files.keys().cloned().collect();
    let pos: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let mut edges = BTreeMap::new();
    let mut link = |a: usize, b: usize| {
        if a != b {
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    };
    for (from, targets) in query::import_edges(index) {
        for to in targets {
            link(pos[from.as_str()], pos[to.as_str()]);
        }
    }
    for site in call_sites(index, &files) {
        link(site.from, site.to);
    }
    FileGraph { files, edges }
}

/// Communities ordered biggest first (ties: first file name); ids follow that order.
pub fn detect(graph: &FileGraph) -> Vec<Community> {
    let n = graph.files.len();
    let mut adj: Adj = vec![BTreeMap::new(); n];
    for (&(a, b), &w) in &graph.edges {
        *adj[a].entry(b).or_insert(0.0) += f64::from(w);
        *adj[b].entry(a).or_insert(0.0) += f64::from(w);
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (file, group) in louvain(adj).into_iter().enumerate() {
        groups.entry(group).or_default().push(file);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let mut community_of = vec![0; n];
    for (id, members) in groups.iter().enumerate() {
        for &m in members {
            community_of[m] = id;
        }
    }
    let mut linked = vec![0usize; groups.len()];
    for &(a, b) in graph.edges.keys() {
        if community_of[a] == community_of[b] {
            linked[community_of[a]] += 1;
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(id, members)| Community {
            id,
            cohesion: cohesion(members.len(), linked[id]),
            files: members.iter().map(|&m| graph.files[m].clone()).collect(),
        })
        .collect()
}

fn cohesion(files: usize, linked: usize) -> f64 {
    if files <= 1 {
        return 1.0;
    }
    linked as f64 / (files * (files - 1) / 2) as f64
}

/// Symmetric weights; a node's own entry holds the weight of edges folded into it.
type Adj = Vec<BTreeMap<usize, f64>>;

/// Node -> group number. Repeats "move nodes to the best neighbour group" then "merge each group
/// into one node" until nothing moves.
fn louvain(mut adj: Adj) -> Vec<usize> {
    let mut member: Vec<usize> = (0..adj.len()).collect();
    loop {
        let (group, moved) = local_moves(&adj);
        if !moved {
            return member;
        }
        let ids: BTreeMap<usize, usize> = group
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(new, old)| (old, new))
            .collect();
        for m in &mut member {
            *m = ids[&group[*m]];
        }
        let mut merged: Adj = vec![BTreeMap::new(); ids.len()];
        for (i, row) in adj.iter().enumerate() {
            for (&j, &w) in row {
                *merged[ids[&group[i]]].entry(ids[&group[j]]).or_insert(0.0) += w;
            }
        }
        adj = merged;
    }
}

/// One Louvain pass: nodes in order, each joins the neighbour group with the best modularity gain
/// (ties go to the lowest group number).
fn local_moves(adj: &Adj) -> (Vec<usize>, bool) {
    let degree: Vec<f64> = adj.iter().map(|row| row.values().sum()).collect();
    let total: f64 = degree.iter().sum();
    let mut group: Vec<usize> = (0..adj.len()).collect();
    if total == 0.0 {
        return (group, false);
    }
    let mut sum = degree.clone();
    let mut moved_ever = false;
    for _ in 0..100 {
        let mut moved = false;
        for i in 0..adj.len() {
            let old = group[i];
            sum[old] -= degree[i];
            let mut to: BTreeMap<usize, f64> = BTreeMap::new();
            for (&j, &w) in adj[i].iter().filter(|&(&j, _)| j != i) {
                *to.entry(group[j]).or_insert(0.0) += w;
            }
            let gain = |g: usize| to.get(&g).unwrap_or(&0.0) - sum[g] * degree[i] / total;
            let mut best = old;
            let mut best_gain = gain(old);
            for &g in to.keys() {
                if gain(g) > best_gain + 1e-12 {
                    best = g;
                    best_gain = gain(g);
                }
            }
            sum[best] += degree[i];
            group[i] = best;
            moved |= best != old;
        }
        moved_ever |= moved;
        if !moved {
            break;
        }
    }
    (group, moved_ever)
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

    fn graph(n: usize, edges: &[(usize, usize)]) -> FileGraph {
        FileGraph {
            files: (0..n).map(|i| format!("f{i}.rs")).collect(),
            edges: edges.iter().map(|&e| (e, 1)).collect(),
        }
    }

    fn names(c: &Community) -> Vec<&str> {
        c.files.iter().map(String::as_str).collect()
    }

    #[test]
    fn calls_to_a_name_defined_in_one_file_link_the_files() {
        let index = index_of(&[("a.rs", "fn a() { b(); b(); }\n"), ("b.rs", "fn b() {}\n")]);
        let g = file_graph(&index);
        assert_eq!(g.files, vec!["a.rs", "b.rs"]);
        assert_eq!(g.edges, BTreeMap::from([((0, 1), 2)]));
    }

    #[test]
    fn names_defined_in_several_files_make_no_link() {
        let index = index_of(&[
            ("a.rs", "fn a() { dup(); }\n"),
            ("b.rs", "fn dup() {}\n"),
            ("c.rs", "fn dup() {}\n"),
        ]);
        assert!(file_graph(&index).edges.is_empty());
    }

    #[test]
    fn rust_module_imports_link_files() {
        let index = index_of(&[("a.rs", "use b::thing;\n"), ("b.rs", "fn thing() {}\n")]);
        // the import adds 1 and the (unresolvable, no call) edge nothing else
        assert_eq!(file_graph(&index).edges, BTreeMap::from([((0, 1), 1)]));
    }

    #[test]
    fn two_triangles_joined_by_one_edge_are_two_communities() {
        let g = graph(6, &[(0, 1), (0, 2), (1, 2), (3, 4), (3, 5), (4, 5), (2, 3)]);
        let found = detect(&g);
        assert_eq!(found.len(), 2);
        assert_eq!(names(&found[0]), vec!["f0.rs", "f1.rs", "f2.rs"]);
        assert_eq!(names(&found[1]), vec!["f3.rs", "f4.rs", "f5.rs"]);
        assert_eq!((found[0].id, found[1].id), (0, 1));
    }

    #[test]
    fn unlinked_files_are_singletons_and_bigger_communities_come_first() {
        let g = graph(4, &[(2, 3)]);
        let found = detect(&g);
        assert_eq!(found.len(), 3);
        assert_eq!(names(&found[0]), vec!["f2.rs", "f3.rs"]);
        assert_eq!(names(&found[1]), vec!["f0.rs"]);
        assert_eq!(names(&found[2]), vec!["f1.rs"]);
    }

    #[test]
    fn detection_is_deterministic() {
        let g = graph(
            8,
            &[
                (0, 1),
                (1, 2),
                (2, 0),
                (2, 3),
                (3, 4),
                (4, 5),
                (5, 3),
                (6, 7),
            ],
        );
        assert_eq!(detect(&g), detect(&g));
    }

    #[test]
    fn cohesion_is_linked_pairs_over_possible_pairs() {
        let tri = detect(&graph(3, &[(0, 1), (0, 2), (1, 2)]));
        assert_eq!(tri[0].cohesion, 1.0);
        let single = detect(&graph(1, &[]));
        assert_eq!(single[0].cohesion, 1.0);
        let line = detect(&graph(3, &[(0, 1), (1, 2)]));
        assert_eq!(line.len(), 1);
        assert!((line[0].cohesion - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_graph_has_no_communities() {
        assert!(detect(&FileGraph::default()).is_empty());
    }
}
