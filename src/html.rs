//! Self-contained interactive graph page: file nodes coloured by community, no network needed.

use std::collections::HashMap;

use serde::Serialize;

use crate::community;
use crate::model::{Index, Kind};

const TEMPLATE: &str = include_str!("html/template.html");

#[derive(Serialize)]
struct SymbolData<'a> {
    name: &'a str,
    kind: Kind,
    line: u32,
}

#[derive(Serialize)]
struct NodeData<'a> {
    path: &'a str,
    /// Top-level directory; `.` for files in the root.
    folder: &'a str,
    community: usize,
    symbols: Vec<SymbolData<'a>>,
}

#[derive(Serialize)]
struct GraphData<'a> {
    nodes: Vec<NodeData<'a>>,
    /// `[from, to, weight]`, indexes into `nodes`.
    edges: Vec<[usize; 3]>,
}

/// Returns the page and the number of files drawn.
pub fn render(project: &str, index: &Index) -> (String, usize) {
    let graph = community::file_graph(index);
    let found = community::detect(&graph);
    let community_of: HashMap<&str, usize> = found
        .iter()
        .flat_map(|c| c.files.iter().map(|f| (f.as_str(), c.id)))
        .collect();
    let nodes: Vec<NodeData> = graph
        .files
        .iter()
        .map(|path| NodeData {
            path,
            folder: path.split_once('/').map_or(".", |(dir, _)| dir),
            community: community_of.get(path.as_str()).copied().unwrap_or(0),
            symbols: index.files.get(path).map_or_else(Vec::new, |file| {
                file.symbols
                    .iter()
                    .filter(|s| s.kind != Kind::Impl)
                    .map(|s| SymbolData {
                        name: &s.name,
                        kind: s.kind,
                        line: s.line,
                    })
                    .collect()
            }),
        })
        .collect();
    let edges = graph
        .edges
        .iter()
        .map(|(&(a, b), &weight)| [a, b, weight as usize])
        .collect();
    let count = nodes.len();
    let data = GraphData { nodes, edges };
    // A `<` inside a string could end the <script> tag (or start `<!--`); `\u003c` means the same.
    let json = serde_json::to_string(&data)
        .unwrap_or_else(|_| "{\"nodes\":[],\"edges\":[]}".to_string())
        .replace('<', "\\u003c");
    // Fill the title in each half separately: neither the project name nor the data can then
    // contain a placeholder that gets rewritten.
    let (head, tail) = TEMPLATE.split_once("/*DATA*/").unwrap_or((TEMPLATE, ""));
    let title = escape(project);
    let page = format!(
        "{}{json}{}",
        head.replace("{{TITLE}}", &title),
        tail.replace("{{TITLE}}", &title)
    );
    (page, count)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

    /// The JSON object the page reads: everything between `const DATA = ` and the closing `;`.
    fn data(page: &str) -> serde_json::Value {
        let start = page.find("const DATA = ").unwrap() + "const DATA = ".len();
        let end = start + page[start..].find(";\n").unwrap();
        serde_json::from_str(&page[start..end]).unwrap()
    }

    fn two_files() -> Index {
        index_of(&[
            ("x/a.rs", "fn a() { b(); }\n"),
            ("y/b.rs", "pub struct S;\nfn b() {}\n"),
        ])
    }

    #[test]
    fn data_lists_files_symbols_and_edges() {
        let (page, count) = render("demo", &two_files());
        assert_eq!(count, 2);
        let d = data(&page);
        let nodes = d["nodes"].as_array().unwrap();
        assert_eq!(nodes[0]["path"], "x/a.rs");
        assert_eq!(nodes[0]["folder"], "x");
        assert_eq!(nodes[1]["symbols"][0]["name"], "S");
        assert_eq!(nodes[1]["symbols"][0]["kind"], "Struct");
        assert_eq!(d["edges"], serde_json::json!([[0, 1, 1]]));
    }

    #[test]
    fn nodes_carry_their_community() {
        let (page, _) = render("demo", &two_files());
        let d = data(&page);
        assert!(d["nodes"][0]["community"].is_u64());
        assert_eq!(d["nodes"][0]["community"], d["nodes"][1]["community"]);
    }

    #[test]
    fn script_closing_tags_in_names_are_escaped() {
        let mut index = index_of(&[("x/a.rs", "fn a() {}\n")]);
        index.files.get_mut("x/a.rs").unwrap().symbols[0].name = "</script><b>".into();
        let (page, _) = render("demo", &index);
        assert!(!page.contains("</script><b>"));
        assert_eq!(
            data(&page)["nodes"][0]["symbols"][0]["name"],
            "</script><b>"
        );
    }

    #[test]
    fn project_name_is_html_escaped_in_title() {
        let (page, _) = render("a<b&c", &Index::default());
        assert!(page.contains("<title>a&lt;b&amp;c - codegraph</title>"));
    }

    #[test]
    fn impl_blocks_are_not_listed_as_symbols() {
        let index = index_of(&[("x/a.rs", "struct S;\nimpl S { fn go() {} }\n")]);
        let (page, _) = render("demo", &index);
        let names: Vec<_> = data(&page)["nodes"][0]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["kind"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&"Impl".to_string()), "{names:?}");
    }

    #[test]
    fn data_placeholder_in_project_name_stays_in_the_title() {
        let (page, _) = render("/*DATA*/", &Index::default());
        assert_eq!(data(&page)["nodes"], serde_json::json!([]));
        assert!(page.contains("<title>/*DATA*/ - codegraph</title>"));
    }

    #[test]
    fn title_placeholder_in_a_symbol_name_stays_in_the_data() {
        let mut index = index_of(&[("x/a.rs", "fn a() {}\n")]);
        index.files.get_mut("x/a.rs").unwrap().symbols[0].name = "{{TITLE}}".into();
        let (page, _) = render("demo", &index);
        assert_eq!(data(&page)["nodes"][0]["symbols"][0]["name"], "{{TITLE}}");
    }

    #[test]
    fn empty_index_still_renders() {
        let (page, count) = render("demo", &Index::default());
        assert_eq!(count, 0);
        assert_eq!(data(&page)["nodes"], serde_json::json!([]));
    }
}
