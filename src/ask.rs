//! `codegraph query "<question>"`: turn a plain-English question into start symbols, walk the call
//! graph around them and print a small answer that fits a token budget.
//! Ported from Graphify's `query` (serve.py: `_query_terms`, `_score_query`, `_pick_seeds`, `_bfs`,
//! `_dfs`, `_subgraph_to_text`): keyword tiers weighted by IDF, coverage-squared scaling, at most 3
//! seeds within 20% of the top score plus one per term, hub-guarded walk, seeds-first output cut
//! at a character budget. Output format and symbol-level nodes are ours.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::Serialize;

use crate::model::Index;
use crate::query;

pub const DEFAULT_DEPTH: usize = 2;
pub const MAX_DEPTH: usize = 6;
/// Half of Graphify's 2000: the goal here is fewer tokens.
pub const DEFAULT_BUDGET: usize = 1000;
/// Graphify counts a token as 3 characters.
const CHARS_PER_TOKEN: usize = 3;

pub struct Options {
    pub depth: usize,
    pub budget: usize,
    pub dfs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            depth: DEFAULT_DEPTH,
            budget: DEFAULT_BUDGET,
            dfs: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeHit {
    pub name: String,
    /// At most 3 definitions; `more` counts the rest.
    pub locations: Vec<String>,
    pub more: usize,
    /// Steps from the nearest start symbol (0 = a start symbol).
    pub hop: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub seeds: Vec<String>,
    pub nodes: Vec<NodeHit>,
    /// `(caller, callee)` pairs among the shown nodes.
    pub edges: Vec<(String, String)>,
    /// What was left out and the `--budget` that shows everything.
    pub notice: Option<String>,
}

impl Answer {
    pub fn lines(&self) -> Vec<String> {
        if self.seeds.is_empty() {
            return vec!["no matching symbols".to_string()];
        }
        let mut out = vec![format!("start: {}", self.seeds.join(", "))];
        out.extend(self.nodes.iter().map(node_line));
        out.extend(self.edges.iter().map(|(a, b)| edge_line(a, b)));
        out.extend(self.notice.clone());
        out
    }
}

fn node_line(n: &NodeHit) -> String {
    let shown: Vec<&str> = n.locations.iter().map(String::as_str).collect();
    let extra = if n.more > 0 {
        format!(", +{} more", n.more)
    } else {
        String::new()
    };
    format!("{}  {}{extra}", n.name, shown.join(", "))
}

fn edge_line(a: &str, b: &str) -> String {
    format!("{a} -> {b}")
}

/// Call graph over code symbols: one node per name, locations of every definition, directed
/// caller -> callee edges, and the same edges both ways for walking.
struct Graph {
    names: Vec<String>,
    locations: Vec<Vec<String>>,
    out: Vec<BTreeSet<usize>>,
    adj: Vec<BTreeSet<usize>>,
}

fn build(index: &Index) -> Graph {
    let mut found: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for file in index.files.values() {
        for s in file.symbols.iter().filter(|s| s.kind.is_code_def()) {
            found
                .entry(s.name.as_str())
                .or_default()
                .insert(format!("{}:{}", file.path, s.line));
        }
    }
    let names: Vec<String> = found.keys().map(|n| n.to_string()).collect();
    let at: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let locations = found
        .into_values()
        .map(|l| l.into_iter().collect())
        .collect();
    let mut out = vec![BTreeSet::new(); names.len()];
    let mut adj = vec![BTreeSet::new(); names.len()];
    for (caller, callees) in query::call_edges(index) {
        let Some(&from) = at.get(caller.as_str()) else {
            continue;
        };
        for to in callees.iter().filter_map(|c| at.get(c.as_str())) {
            out[from].insert(*to);
            adj[from].insert(*to);
            adj[*to].insert(from);
        }
    }
    Graph {
        names,
        locations,
        out,
        adj,
    }
}

const STOPWORDS: [&str; 33] = [
    "a", "an", "the", "of", "to", "in", "on", "is", "are", "was", "were", "who", "what", "where",
    "how", "does", "do", "did", "which", "for", "and", "or", "it", "its", "this", "that", "with",
    "by", "from", "me", "show", "find", "get",
];
/// Words that ask for a relation, not a symbol (Graphify's `_RELATIONAL_INTENT_TERMS`).
const RELATIONAL: [&str; 12] = [
    "call", "calls", "called", "caller", "callers", "use", "uses", "used", "depend", "depends",
    "import", "imports",
];

/// Lowercase words of the question without stopwords and relation verbs; if nothing is left the
/// words are kept as typed.
fn terms(question: &str) -> Vec<String> {
    let words: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut seen = BTreeSet::new();
    let kept: Vec<String> = words
        .iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()) && !RELATIONAL.contains(&w.as_str()))
        .filter(|w| seen.insert((*w).clone()))
        .take(MAX_TERMS)
        .cloned()
        .collect();
    if kept.is_empty() {
        let mut seen = BTreeSet::new();
        words
            .into_iter()
            .filter(|w| seen.insert(w.clone()))
            .collect()
    } else {
        kept
    }
}

/// `notifyWaiters`, `notify_waiters` and `HTTPServer` -> lowercase words.
fn split_identifier(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let lower_to_upper = i > 0 && c.is_uppercase() && chars[i - 1].is_lowercase();
        let acronym_end = i > 0
            && c.is_uppercase()
            && chars[i - 1].is_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if (lower_to_upper || acronym_end) && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

const EXACT: f64 = 1000.0;
const PREFIX: f64 = 100.0;
const SUBSTRING: f64 = 1.0;
const PATH: f64 = 0.5;

/// How well one term matches one symbol name: exact word, word prefix, or substring.
fn tier(lower: &str, words: &[String], term: &str) -> f64 {
    if lower == term || words.iter().any(|w| w == term) {
        EXACT
    } else if lower.starts_with(term) || words.iter().any(|w| w.starts_with(term)) {
        PREFIX
    } else if lower.contains(term) {
        SUBSTRING
    } else {
        0.0
    }
}

struct Scored {
    total: f64,
    /// Per term: weight x tier (+ path), used to give every term a seed.
    per_term: Vec<f64>,
}

fn score_all(graph: &Graph, terms: &[String]) -> Vec<Scored> {
    let lowers: Vec<String> = graph.names.iter().map(|n| n.to_lowercase()).collect();
    let words: Vec<Vec<String>> = graph.names.iter().map(|n| split_identifier(n)).collect();
    let n = graph.names.len() as f64;
    let weight: Vec<f64> = terms
        .iter()
        .map(|t| {
            let df = lowers.iter().filter(|l| l.contains(t.as_str())).count() as f64;
            (1.0 + n / (1.0 + df)).ln()
        })
        .collect();
    let max_weight = weight.iter().cloned().fold(0.0, f64::max);
    let joined = terms.join("_");
    // Lowercased file paths of each symbol, made once instead of per term.
    let paths: Vec<Vec<String>> = graph
        .locations
        .iter()
        .map(|locs| {
            locs.iter()
                .map(|l| {
                    l.rsplit_once(':')
                        .map_or(l.as_str(), |p| p.0)
                        .to_lowercase()
                })
                .collect()
        })
        .collect();
    (0..graph.names.len())
        .map(|i| {
            let tiers: Vec<f64> = terms
                .iter()
                .map(|t| tier(&lowers[i], &words[i], t))
                .collect();
            let matched = tiers.iter().filter(|&&t| t > 0.0).count() as f64;
            let coverage = (matched / terms.len() as f64).powi(2);
            let on_path = |t: &String| paths[i].iter().any(|p| p.contains(t.as_str()));
            let path_bonus: Vec<f64> = terms
                .iter()
                .zip(&weight)
                .map(|(t, w)| if on_path(t) { PATH * w } else { 0.0 })
                .collect();
            let per_term: Vec<f64> = tiers.iter().zip(&weight).map(|(t, w)| t * w).collect();
            let mut total: f64 = tiers
                .iter()
                .zip(&weight)
                .zip(&path_bonus)
                .map(|((&t, w), p)| {
                    let scaled = if t >= PREFIX { t * coverage } else { t };
                    scaled * w + p
                })
                .sum();
            if terms.len() > 1 || matched > 0.0 {
                if lowers[i] == joined || words[i].join("_") == joined {
                    total += 10.0 * EXACT * max_weight;
                } else if lowers[i].starts_with(&joined) {
                    total += 10.0 * PREFIX * max_weight;
                }
            }
            // The file path only adds to a name that already matches; it never makes a start symbol.
            if matched == 0.0 {
                total = 0.0;
            }
            Scored { total, per_term }
        })
        .collect()
}

const MAX_SEEDS: usize = 3;
const MAX_LOCATIONS: usize = 3;
/// A question longer than this many distinct words is cut (cost is words x symbols).
const MAX_TERMS: usize = 12;
const GAP_RATIO: f64 = 0.2;

/// Top nodes within 20% of the best score (at most 3), then the best node of every term.
fn pick_seeds(graph: &Graph, scored: &[Scored], terms: &[String]) -> Vec<usize> {
    let rank = |a: &usize, b: &usize| {
        scored[*b]
            .total
            .total_cmp(&scored[*a].total)
            .then_with(|| graph.names[*a].len().cmp(&graph.names[*b].len()))
            .then_with(|| graph.names[*a].cmp(&graph.names[*b]))
    };
    let mut ranked: Vec<usize> = (0..scored.len())
        .filter(|&i| scored[i].total > 0.0)
        .collect();
    ranked.sort_by(rank);
    let Some(&best) = ranked.first() else {
        return Vec::new();
    };
    let floor = scored[best].total * GAP_RATIO;
    let mut seeds: Vec<usize> = ranked
        .iter()
        .copied()
        .take(MAX_SEEDS)
        .take_while(|&i| scored[i].total >= floor)
        .collect();
    for t in 0..terms.len() {
        let top = (0..scored.len())
            .filter(|&i| scored[i].per_term[t] > 0.0)
            .max_by(|a, b| {
                scored[*a].per_term[t]
                    .total_cmp(&scored[*b].per_term[t])
                    .then_with(|| graph.adj[*a].len().cmp(&graph.adj[*b].len()))
                    .then_with(|| graph.names[*b].len().cmp(&graph.names[*a].len()))
                    .then_with(|| graph.names[*b].cmp(&graph.names[*a]))
            });
        if let Some(i) = top.filter(|i| !seeds.contains(i)) {
            seeds.push(i);
        }
    }
    seeds
}

/// A non-seed node with at least this many neighbours is shown but not expanded.
fn hub_limit(graph: &Graph) -> usize {
    let mut degrees: Vec<usize> = graph.adj.iter().map(BTreeSet::len).collect();
    degrees.sort_unstable();
    let p99 = degrees
        .get((degrees.len() * 99).div_ceil(100).saturating_sub(1))
        .copied()
        .unwrap_or(0);
    p99.max(50)
}

/// Visited nodes with their hop distance, walking breadth-first or depth-first.
fn walk(graph: &Graph, seeds: &[usize], depth: usize, dfs: bool) -> BTreeMap<usize, usize> {
    let limit = hub_limit(graph);
    let mut hops: BTreeMap<usize, usize> = seeds.iter().map(|&s| (s, 0)).collect();
    let mut todo: VecDeque<usize> = seeds.iter().copied().collect();
    while let Some(node) = if dfs {
        todo.pop_back()
    } else {
        todo.pop_front()
    } {
        let hop = hops[&node];
        if hop >= depth || (!seeds.contains(&node) && graph.adj[node].len() >= limit) {
            continue;
        }
        for &next in &graph.adj[node] {
            if let Entry::Vacant(slot) = hops.entry(next) {
                slot.insert(hop + 1);
                todo.push_back(next);
            }
        }
    }
    hops
}

pub fn query(index: &Index, question: &str, opts: &Options) -> Answer {
    let graph = build(index);
    let terms = terms(question);
    let scored = score_all(&graph, &terms);
    let seeds = if terms.is_empty() {
        Vec::new()
    } else {
        pick_seeds(&graph, &scored, &terms)
    };
    if seeds.is_empty() {
        return Answer {
            seeds: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            notice: None,
        };
    }
    let hops = walk(&graph, &seeds, opts.depth.clamp(1, MAX_DEPTH), opts.dfs);
    let mut order: Vec<usize> = seeds.clone();
    let mut rest: Vec<usize> = hops
        .keys()
        .copied()
        .filter(|i| !seeds.contains(i))
        .collect();
    let limit = hub_limit(&graph);
    let is_hub = |i: &usize| graph.adj[*i].len() >= limit;
    rest.sort_by(|a, b| {
        is_hub(a)
            .cmp(&is_hub(b))
            .then_with(|| hops[a].cmp(&hops[b]))
            .then_with(|| graph.adj[*b].len().cmp(&graph.adj[*a].len()))
            .then_with(|| graph.names[*a].cmp(&graph.names[*b]))
    });
    order.extend(rest);
    fit(&graph, &seeds, &order, &hops, opts.budget)
}

/// Apply the budget: seeds always stay, further nodes and edges only while they fit.
fn fit(
    graph: &Graph,
    seeds: &[usize],
    order: &[usize],
    hops: &BTreeMap<usize, usize>,
    budget: usize,
) -> Answer {
    let limit = budget.saturating_mul(CHARS_PER_TOKEN);
    let hit = |i: usize| NodeHit {
        name: graph.names[i].clone(),
        locations: graph.locations[i]
            .iter()
            .take(MAX_LOCATIONS)
            .cloned()
            .collect(),
        more: graph.locations[i].len().saturating_sub(MAX_LOCATIONS),
        hop: hops[&i],
    };
    let seed_names: Vec<String> = seeds.iter().map(|&i| graph.names[i].clone()).collect();
    let mut used = format!("start: {}", seed_names.join(", ")).len() + 1;
    let mut shown: Vec<usize> = Vec::new();
    let mut full = used;
    for (n, &i) in order.iter().enumerate() {
        let size = node_line(&hit(i)).len() + 1;
        full += size;
        if n < seeds.len() || (shown.len() == n && used + size <= limit) {
            used += size;
            shown.push(i);
        }
    }
    let edge_pairs = |among: &dyn Fn(usize) -> bool| -> Vec<(usize, usize)> {
        let mut pairs = Vec::new();
        for &from in order.iter().filter(|&&i| among(i)) {
            pairs.extend(
                graph.out[from]
                    .iter()
                    .filter(|&&to| among(to))
                    .map(|&to| (from, to)),
            );
        }
        pairs.sort_by(|a, b| {
            (&graph.names[a.0], &graph.names[a.1]).cmp(&(&graph.names[b.0], &graph.names[b.1]))
        });
        pairs
    };
    let all_edges = edge_pairs(&|i| hops.contains_key(&i));
    full += all_edges
        .iter()
        .map(|&(a, b)| edge_line(&graph.names[a], &graph.names[b]).len() + 1)
        .sum::<usize>();
    let mut edges = Vec::new();
    for (a, b) in edge_pairs(&|i| shown.contains(&i)) {
        let size = edge_line(&graph.names[a], &graph.names[b]).len() + 1;
        if used + size > limit {
            break;
        }
        used += size;
        edges.push((graph.names[a].clone(), graph.names[b].clone()));
    }
    let hidden_nodes = order.len() - shown.len();
    let hidden_edges = all_edges.len() - edges.len();
    let notice = (hidden_nodes + hidden_edges > 0).then(|| {
        format!(
            "... {hidden_nodes} more nodes, {hidden_edges} more edges (--budget {} shows all)",
            full.div_ceil(CHARS_PER_TOKEN)
        )
    });
    Answer {
        seeds: seed_names,
        nodes: shown.iter().map(|&i| hit(i)).collect(),
        edges,
        notice,
    }
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

    fn seeds(src: &str, question: &str) -> Vec<String> {
        query(&index_of(&[("a.rs", src)]), question, &Options::default()).seeds
    }

    #[test]
    fn exact_beats_prefix_beats_substring_and_the_gap_rule_drops_weak_seeds() {
        let src = "fn parse() {}\nfn parser_new() {}\nfn reparse() {}\n";
        assert_eq!(seeds(src, "pars"), ["parse", "parser_new"]);
        assert_eq!(seeds(src, "parse")[0], "parse");
    }

    #[test]
    fn a_rare_term_outweighs_a_common_one() {
        let src = "fn common_a() {}\nfn common_b() {}\nfn common_c() {}\nfn rare_x() {}\n";
        assert_eq!(seeds(src, "common rare")[0], "rare_x");
    }

    #[test]
    fn covering_more_of_the_question_ranks_higher() {
        let src = "fn alpha_only() {}\nfn alpha_beta() {}\n";
        assert_eq!(seeds(src, "alpha beta")[0], "alpha_beta");
    }

    #[test]
    fn every_term_gets_a_seed_even_below_the_gap() {
        let src = "fn heavy_light_x() {}\nfn light() {}\n";
        assert!(seeds(src, "heavy light").contains(&"light".to_string()));
    }

    #[test]
    fn stopwords_and_relational_verbs_do_not_seed() {
        assert_eq!(
            seeds("fn zebra() {}\nfn calls() {}\n", "who calls the zebra"),
            ["zebra"]
        );
    }

    #[test]
    fn camel_and_snake_case_are_split() {
        let idx = index_of(&[("a.ts", "export function notifyWaiters() {}\n")]);
        let a = query(&idx, "notify waiters", &Options::default());
        assert_eq!(a.seeds, ["notifyWaiters"]);
    }

    #[test]
    fn no_match_says_so() {
        let a = query(
            &index_of(&[("a.rs", "fn a() {}\n")]),
            "zzzz",
            &Options::default(),
        );
        assert_eq!(a.lines(), ["no matching symbols"]);
    }

    #[test]
    fn same_named_definitions_are_one_node_with_every_location() {
        let idx = index_of(&[("a.rs", "fn dup() {}\n"), ("b.rs", "fn dup() {}\n")]);
        let a = query(&idx, "dup", &Options::default());
        assert_eq!(a.nodes.len(), 1);
        assert_eq!(a.nodes[0].locations, ["a.rs:1", "b.rs:1"]);
    }

    #[test]
    fn the_walk_follows_calls_both_ways_and_orders_by_hop() {
        let src = "fn top() { mid(); }\nfn mid() { leaf(); }\nfn leaf() {}\n";
        let idx = index_of(&[("a.rs", src)]);
        let a = query(&idx, "mid", &Options::default());
        let names: Vec<_> = a.nodes.iter().map(|n| (n.name.as_str(), n.hop)).collect();
        assert_eq!(names, [("mid", 0), ("leaf", 1), ("top", 1)]);
        assert!(a.edges.contains(&("top".into(), "mid".into())));
        let shallow = query(
            &idx,
            "mid",
            &Options {
                depth: 1,
                ..Options::default()
            },
        );
        assert_eq!(shallow.nodes.len(), 3);
        let dfs = query(
            &idx,
            "top",
            &Options {
                dfs: true,
                depth: 1,
                ..Options::default()
            },
        );
        assert_eq!(dfs.nodes.len(), 2);
    }

    #[test]
    fn a_hub_is_shown_but_not_expanded() {
        let mut src = String::from("fn hub() { deep(); }\nfn deep() {}\n");
        for n in 0..60 {
            src.push_str(&format!("fn f{n}() {{ hub(); }}\n"));
        }
        let a = seeds_answer(&src, "f0");
        let names: Vec<_> = a.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"hub"));
        assert!(
            !names.contains(&"deep") && !names.contains(&"f1"),
            "{names:?}"
        );
    }

    fn seeds_answer(src: &str, q: &str) -> Answer {
        query(
            &index_of(&[("a.rs", src)]),
            q,
            &Options {
                depth: 3,
                ..Options::default()
            },
        )
    }

    #[test]
    fn the_budget_cuts_nodes_but_never_the_seeds_and_names_the_budget_that_fits_all() {
        let mut src = String::from("fn start() {\n");
        for n in 0..30 {
            src.push_str(&format!("    callee_number_{n}();\n"));
        }
        src.push_str("}\n");
        for n in 0..30 {
            src.push_str(&format!("fn callee_number_{n}() {{}}\n"));
        }
        let idx = index_of(&[("a.rs", &src)]);
        let cut = query(
            &idx,
            "start",
            &Options {
                budget: 30,
                ..Options::default()
            },
        );
        assert_eq!(cut.seeds, ["start"]);
        assert!(cut.nodes.len() < 31 && !cut.nodes.is_empty());
        let notice = cut.notice.clone().expect("a notice");
        assert!(notice.contains("--budget"), "{notice}");
        let n: usize = notice
            .split("--budget ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let all = query(
            &idx,
            "start",
            &Options {
                budget: n,
                ..Options::default()
            },
        );
        assert!(all.notice.is_none(), "{n}: {:?}", all.notice);
        assert_eq!(all.nodes.len(), 31);
    }

    #[test]
    fn lines_have_a_start_line_nodes_then_edges() {
        let idx = index_of(&[("a.rs", "fn top() { leaf(); }\nfn leaf() {}\n")]);
        let lines = query(&idx, "top", &Options::default()).lines();
        assert_eq!(
            lines,
            ["start: top", "top  a.rs:1", "leaf  a.rs:2", "top -> leaf"]
        );
    }

    #[test]
    fn hubs_come_last_so_the_budget_cuts_them_first() {
        let mut src = String::from(
            "fn start() { hub(); special(); }
fn special() {}
",
        );
        for n in 0..60 {
            src.push_str(&format!("fn f{n}() {{ hub(); }}\n"));
        }
        src.push_str("fn hub() {}\n");
        let a = seeds_answer(&src, "start");
        let names: Vec<_> = a.nodes.iter().map(|n| n.name.as_str()).collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("special") < pos("hub"), "{names:?}");
    }

    #[test]
    fn a_long_question_is_capped_and_a_blank_one_matches_nothing() {
        let words: Vec<String> = (0..50).map(|n| format!("fn_{n}")).collect();
        let src: String = words.iter().map(|w| format!("fn {w}() {{}}\n")).collect();
        let a = seeds_answer(&src, &words.join(" "));
        assert!(a.seeds.len() <= MAX_SEEDS + MAX_TERMS, "{}", a.seeds.len());
        assert_eq!(seeds_answer(&src, "?!").lines(), ["no matching symbols"]);
        assert_eq!(seeds_answer(&src, "").lines(), ["no matching symbols"]);
    }

    #[test]
    fn a_path_only_match_does_not_seed() {
        let idx = index_of(&[("zebra/a.rs", "fn unrelated() {}\n")]);
        assert!(query(&idx, "zebra", &Options::default()).seeds.is_empty());
    }

    #[test]
    fn many_definitions_show_three_locations_in_text_and_json() {
        let files: Vec<(String, &str)> = (0..5)
            .map(|n| (format!("f{n}.rs"), "fn dup() {}\n"))
            .collect();
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, s)| (p.as_str(), *s)).collect();
        let a = query(&index_of(&refs), "dup", &Options::default());
        assert_eq!((a.nodes[0].locations.len(), a.nodes[0].more), (3, 2));
        assert!(a.lines()[1].ends_with("+2 more"), "{:?}", a.lines());
    }

    #[test]
    fn a_tiny_budget_still_shows_every_seed_and_a_huge_one_does_not_overflow() {
        let idx = index_of(&[("a.rs", "fn top() { leaf(); }\nfn leaf() {}\n")]);
        let tiny = query(
            &idx,
            "top",
            &Options {
                budget: 1,
                ..Options::default()
            },
        );
        assert_eq!(tiny.nodes.len(), 1);
        assert!(tiny.notice.is_some());
        let huge = query(
            &idx,
            "top",
            &Options {
                budget: usize::MAX,
                ..Options::default()
            },
        );
        assert!(huge.notice.is_none());
    }

    #[test]
    fn dfs_hops_are_first_discovery_not_shortest() {
        // top calls a and b; a calls b. DFS pops the last pushed (b) first, so a is found at hop 1
        // and b at hop 1; walking on, nothing is deeper than the depth limit.
        let src = "fn top() { a(); b(); }\nfn a() { b(); }\nfn b() {}\n";
        let idx = index_of(&[("a.rs", src)]);
        let a = query(
            &idx,
            "top",
            &Options {
                dfs: true,
                depth: 2,
                ..Options::default()
            },
        );
        assert_eq!(a.nodes.len(), 3);
        assert!(a.nodes.iter().all(|n| n.hop <= 2));
    }
}
