//! Mermaid call-flow export: a text diagram that GitHub and Obsidian draw as a picture.
//! Graphify's version is `callflow_html.py`; ours prints plain Mermaid, no page and no CDN.

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::model::Index;
use crate::query::{bare_name, call_edges};

/// Most edges drawn; a hub with more calls ends in a `more…` node.
const MAX_EDGES: usize = 60;

/// `flowchart LR` of what `name` calls, up to `depth` hops. Names defined nowhere in the project
/// (`println`, `new`) are left out. Empty string when `name` calls nothing.
pub fn callflow(index: &Index, name: &str, depth: usize) -> String {
    let defined: BTreeSet<&str> = index
        .files
        .values()
        .flat_map(|f| &f.symbols)
        .map(|s| s.name.as_str())
        .collect();
    let calls = call_edges(index);
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut lines = Vec::new();
    let mut truncated = None;
    let mut queue = VecDeque::from([(name.to_string(), 0)]);
    let mut seen = BTreeSet::from([name.to_string()]);
    'walk: while let Some((from, hops)) = queue.pop_front() {
        if hops >= depth {
            continue;
        }
        for to in calls.get(&from).into_iter().flatten() {
            if !defined.contains(bare_name(to).as_str()) {
                continue;
            }
            if lines.len() == MAX_EDGES {
                truncated = Some(node(&mut ids, &from));
                break 'walk;
            }
            let (a, b) = (node(&mut ids, &from), node(&mut ids, to));
            lines.push(format!("    {a} --> {b}"));
            if seen.insert(to.clone()) {
                queue.push_back((to.clone(), hops + 1));
            }
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    if let Some(last) = truncated {
        lines.push(format!("    {last} -.-> more[\"more…\"]"));
    }
    let mut labels: Vec<_> = ids.into_iter().collect();
    labels.sort_by_key(|(_, id)| *id);
    let mut out = vec!["flowchart LR".to_string()];
    out.extend(labels.iter().map(|(n, id)| {
        format!(
            "    n{id}[\"{}\"]",
            n.replace('#', "#35;").replace('"', "#quot;")
        )
    }));
    out.extend(lines);
    out.join("\n") + "\n"
}

/// Safe Mermaid id (`n0`, `n1`…) for a name; names go in quoted labels, never in ids.
fn node(ids: &mut HashMap<String, usize>, name: &str) -> String {
    let next = ids.len();
    format!("n{}", ids.entry(name.to_string()).or_insert(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{self, Lang};
    use crate::model::FileEntry;

    fn index_of(src: &str) -> Index {
        let facts = lang::extract(Lang::Rust, src).unwrap();
        let mut index = Index::default();
        index.files.insert(
            "a.rs".into(),
            FileEntry {
                path: "a.rs".into(),
                symbols: facts.symbols,
                calls: facts.calls,
                ..FileEntry::default()
            },
        );
        index
    }

    const CHAIN: &str = "fn a() { b(); }\nfn b() { c(); }\nfn c() { println!(\"x\"); }\n";

    #[test]
    fn chain_is_cut_at_depth() {
        let index = index_of(CHAIN);
        let one = callflow(&index, "a", 1);
        assert!(one.starts_with("flowchart LR\n"));
        assert!(one.contains("n0[\"a\"]") && one.contains("n1[\"b\"]"));
        assert!(!one.contains("\"c\""));
        let two = callflow(&index, "a", 2);
        assert!(two.contains("n1 --> n2") && two.contains("n2[\"c\"]"));
    }

    #[test]
    fn quotes_in_labels_are_escaped() {
        let mut index = index_of("fn a() { b(); }\nfn b() {}\n");
        let file = index.files.get_mut("a.rs").unwrap();
        file.calls[0].callee = "b\"x#y".into();
        file.symbols[1].name = "b\"x#y".into();
        assert!(callflow(&index, "a", 1).contains("[\"b#quot;x#35;y\"]"));
    }

    #[test]
    fn cycle_terminates() {
        let index = index_of("fn a() { b(); }\nfn b() { a(); }\n");
        let out = callflow(&index, "a", 10);
        assert!(out.contains("n0 --> n1") && out.contains("n1 --> n0"));
    }

    #[test]
    fn unknown_or_leaf_name_gives_nothing() {
        let index = index_of(CHAIN);
        assert!(callflow(&index, "nope", 2).is_empty());
        assert!(callflow(&index, "c", 2).is_empty());
    }

    #[test]
    fn big_hub_is_capped_with_more_node() {
        let callees: String = (0..70).map(|i| format!("fn f{i}() {{}}\n")).collect();
        let body: String = (0..70).map(|i| format!("f{i}();")).collect();
        let index = index_of(&format!("fn hub() {{ {body} }}\n{callees}"));
        let out = callflow(&index, "hub", 1);
        assert_eq!(out.matches("-->").count(), MAX_EDGES);
        assert!(out.contains("-.-> more[\"more…\"]"));
    }
}
