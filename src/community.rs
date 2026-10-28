//! Groups files into communities (clusters of related code) using Louvain modularity.
//!
//! Louvain follows the approach of Graphify's `cluster.py` (Apache-2.0, safishamsi/graphify):
//! deterministic ordering, communities ordered by size, cohesion = intra edges / possible edges.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::model::{Index, Kind, Resolution};
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
    let position: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let mut sites = Vec::new();
    for (from, file) in files.iter().enumerate() {
        for (call, target) in index.files[file].graph_calls() {
            // SCIP names the defining file; without it, accept a name defined in one file only.
            let to = match target {
                Some(Resolution::Def { path, .. }) => position.get(path.as_str()).copied(),
                _ => match homes.get(call.callee.as_str()).map(Vec::as_slice) {
                    Some(&[to]) => Some(to),
                    _ => None,
                },
            };
            let Some(to) = to else { continue };
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
    for (file, group) in louvain(adj.clone()).into_iter().enumerate() {
        groups.entry(group).or_default().push(file);
    }
    let mut groups = refine(groups.into_values().collect(), &adj, &graph.edges);
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

// Splitting rules ported from Graphify's `cluster.py` (`_MAX_COMMUNITY_FRACTION`, `_MIN_SPLIT_SIZE`,
// `_COHESION_SPLIT_THRESHOLD`, `_COHESION_SPLIT_MIN_SIZE`); Louvain alone can leave one big loose
// cluster (tokio: 188 of 814 files, cohesion 0.03).
/// A group bigger than this share (1/n) of all files is split.
const MAX_SHARE_DIVISOR: usize = 4;
const MIN_SPLIT_SIZE: usize = 10;
/// A group of at least `COHESION_SPLIT_MIN_SIZE` files with less cohesion than this is split.
const COHESION_SPLIT_BELOW: f64 = 0.05;
const COHESION_SPLIT_MIN_SIZE: usize = 50;

/// Split oversized groups, then loosely linked big ones, by running Louvain on each one alone.
fn refine(
    groups: Vec<Vec<usize>>,
    adj: &Adj,
    edges: &BTreeMap<(usize, usize), u32>,
) -> Vec<Vec<usize>> {
    let max_size = MIN_SPLIT_SIZE.max(adj.len() / MAX_SHARE_DIVISOR);
    let by_size = groups.into_iter().flat_map(|g| {
        if g.len() > max_size {
            split_group(adj, &g)
        } else {
            vec![g]
        }
    });
    by_size
        .flat_map(|g| {
            if g.len() >= COHESION_SPLIT_MIN_SIZE
                && group_cohesion(&g, edges) < COHESION_SPLIT_BELOW
            {
                split_group(adj, &g)
            } else {
                vec![g]
            }
        })
        .collect()
}

fn group_cohesion(members: &[usize], edges: &BTreeMap<(usize, usize), u32>) -> f64 {
    let inside: std::collections::HashSet<usize> = members.iter().copied().collect();
    let linked = edges
        .keys()
        .filter(|(a, b)| inside.contains(a) && inside.contains(b))
        .count();
    cohesion(members.len(), linked)
}

/// `members` re-clustered on their own links; the group itself when that finds no split.
fn split_group(adj: &Adj, members: &[usize]) -> Vec<Vec<usize>> {
    let local: HashMap<usize, usize> = members.iter().enumerate().map(|(i, &m)| (m, i)).collect();
    let sub: Adj = members
        .iter()
        .map(|&m| {
            adj[m]
                .iter()
                .filter_map(|(other, &w)| local.get(other).map(|&i| (i, w)))
                .collect()
        })
        .collect();
    let mut parts: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, part) in louvain(sub).into_iter().enumerate() {
        parts.entry(part).or_default().push(members[i]);
    }
    if parts.len() < 2 {
        return vec![members.to_vec()];
    }
    parts.into_values().collect()
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
    fn scip_resolutions_pick_the_defining_file_and_drop_external_calls() {
        use crate::model::{Resolution, ResolvedCall};
        let mut index = index_of(&[
            ("x.rs", "fn x() { new(); new(); new(); }\n"),
            ("b.rs", "fn new() {}\n"),
            ("c.rs", "fn new() {}\n"),
        ]);
        let files: Vec<String> = ["x.rs", "b.rs", "c.rs"].map(String::from).to_vec();
        assert!(
            call_sites(&index, &files).is_empty(),
            "two homes: ambiguous"
        );
        index.files.get_mut("x.rs").unwrap().resolved = vec![
            ResolvedCall {
                call: 0,
                target: Resolution::Def {
                    path: "c.rs".into(),
                    line: 1,
                },
            },
            ResolvedCall {
                call: 1,
                target: Resolution::External,
            },
        ];
        let sites: Vec<_> = call_sites(&index, &files)
            .iter()
            .map(|s| (s.from, s.to))
            .collect();
        assert_eq!(sites, vec![(0, 2)]);
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

    /// `total` nodes wired by `links`; nodes no link mentions stay alone.
    fn adj_of(total: usize, links: &[(usize, usize)]) -> Adj {
        let mut adj: Adj = vec![BTreeMap::new(); total];
        for &(a, b) in links {
            *adj[a].entry(b).or_insert(0.0) += 1.0;
            *adj[b].entry(a).or_insert(0.0) += 1.0;
        }
        adj
    }

    fn edge_map(links: &[(usize, usize)]) -> BTreeMap<(usize, usize), u32> {
        links
            .iter()
            .map(|&(a, b)| ((a.min(b), a.max(b)), 1))
            .collect()
    }

    fn clique(nodes: std::ops::Range<usize>) -> Vec<(usize, usize)> {
        nodes
            .clone()
            .flat_map(|a| (a + 1..nodes.end).map(move |b| (a, b)))
            .collect()
    }

    #[test]
    fn split_group_separates_two_cliques_joined_by_one_bridge() {
        let mut links = clique(0..5);
        links.extend(clique(5..10));
        links.push((4, 5));
        let adj = adj_of(10, &links);
        let parts = split_group(&adj, &(0..10).collect::<Vec<_>>());
        assert_eq!(parts, vec![vec![0, 1, 2, 3, 4], vec![5, 6, 7, 8, 9]]);
        // One tight clique stays whole.
        let one = split_group(&adj_of(5, &clique(0..5)), &[0, 1, 2, 3, 4]);
        assert_eq!(one, vec![vec![0, 1, 2, 3, 4]]);
    }

    /// Two stars of 30 leaves (hubs 0 and 30) joined by their hubs: 59 links among 60 files.
    fn two_stars() -> Vec<(usize, usize)> {
        let mut links: Vec<_> = (1..30).map(|leaf| (0, leaf)).collect();
        links.extend((31..60).map(|leaf| (30, leaf)));
        links.push((0, 30));
        links
    }

    #[test]
    fn a_sparse_big_group_is_split_but_a_dense_one_is_not() {
        // 400 other lone files keep the 60-file group under the 25% size rule: only cohesion decides.
        let total = 460;
        let everyone: Vec<usize> = (0..60).collect();
        let lone = || (60..total).map(|i| vec![i]);

        let sparse = two_stars();
        let groups = std::iter::once(everyone.clone()).chain(lone()).collect();
        let refined = refine(groups, &adj_of(total, &sparse), &edge_map(&sparse));
        assert!(
            refined.len() > 401,
            "sparse group (cohesion 0.03) should split"
        );

        let band: Vec<_> = (0..60)
            .flat_map(|a| (a + 1..60.min(a + 11)).map(move |b| (a, b)))
            .collect();
        let groups = std::iter::once(everyone).chain(lone()).collect();
        let refined = refine(groups, &adj_of(total, &band), &edge_map(&band));
        assert_eq!(
            refined.len(),
            401,
            "dense group (cohesion 0.15) stays whole"
        );
    }

    #[test]
    fn a_group_over_a_quarter_of_the_files_is_split_by_size() {
        let mut links = clique(0..6);
        links.extend(clique(6..12));
        links.push((5, 6));
        let groups = vec![(0..12).collect::<Vec<_>>()];
        let refined = refine(groups, &adj_of(12, &links), &edge_map(&links));
        assert_eq!(refined.len(), 2);
    }
}
