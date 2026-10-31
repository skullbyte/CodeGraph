//! Self-contained interactive graph page: file nodes coloured by community, no network needed.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::community;
use crate::model::{Index, Kind};
use crate::query;

/// The page skeleton; `/*STYLE*/` and `/*SCRIPT*/` mark where the style sheet and the script go.
const TEMPLATE: &str = include_str!("html/template.html");
const STYLE: &str = include_str!("html/style.css");
/// The page script, in the order it runs: top-level `let`/`const` must come before their first use.
const SCRIPT_PARTS: [&str; 10] = [
    include_str!("html/js/01-data.js"),
    include_str!("html/js/02-layout.js"),
    include_str!("html/js/03-sprites.js"),
    include_str!("html/js/04-backdrop.js"),
    include_str!("html/js/05-scene.js"),
    include_str!("html/js/06-draw.js"),
    include_str!("html/js/07-dropdown.js"),
    include_str!("html/js/08-controls.js"),
    include_str!("html/js/09-input.js"),
    include_str!("html/js/10-loop.js"),
];

/// The whole page with style and script inlined, still holding `/*DATA*/` and `{{TITLE}}`.
fn assemble() -> String {
    TEMPLATE
        .replacen("/*STYLE*/", STYLE, 1)
        .replacen("/*SCRIPT*/", &SCRIPT_PARTS.concat(), 1)
}

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
    /// `[a, b, weight, direction]`, `a < b` indexes into `nodes`; direction bit 1 = a points at b
    /// (imports or calls it), bit 2 = b points at a.
    edges: Vec<[usize; 4]>,
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
                    .filter(|s| s.kind.is_code_def())
                    .map(|s| SymbolData {
                        name: &s.name,
                        kind: s.kind,
                        line: s.line,
                    })
                    .collect()
            }),
        })
        .collect();
    let pointing = directed_pairs(index, &graph.files);
    let edges = graph
        .edges
        .iter()
        .map(|(&(a, b), &weight)| {
            let direction = usize::from(pointing.contains(&(a, b)))
                | usize::from(pointing.contains(&(b, a))) << 1;
            [a, b, weight as usize, direction]
        })
        .collect();
    let count = nodes.len();
    let data = GraphData { nodes, edges };
    // A `<` inside a string could end the <script> tag (or start `<!--`); `\u003c` means the same.
    let json = serde_json::to_string(&data)
        .unwrap_or_else(|_| "{\"nodes\":[],\"edges\":[]}".to_string())
        .replace('<', "\\u003c");
    // Fill the title in each half separately: neither the project name nor the data can then
    // contain a placeholder that gets rewritten.
    let template = assemble();
    let (head, tail) = template
        .split_once("/*DATA*/")
        .unwrap_or((template.as_str(), ""));
    let title = escape(project);
    let page = format!(
        "{}{json}{}",
        head.replace("{{TITLE}}", &title),
        tail.replace("{{TITLE}}", &title)
    );
    (page, count)
}

/// `(from, to)` file indexes: `from` imports `to` or calls something defined only in `to`.
fn directed_pairs(index: &Index, files: &[String]) -> HashSet<(usize, usize)> {
    let pos: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let mut pairs: HashSet<(usize, usize)> = community::call_sites(index, files)
        .iter()
        .map(|site| (site.from, site.to))
        .collect();
    for (from, targets) in query::import_edges(index) {
        let Some(&a) = pos.get(from.as_str()) else {
            continue;
        };
        pairs.extend(
            targets
                .iter()
                .filter_map(|t| pos.get(t.as_str()).map(|&b| (a, b))),
        );
    }
    pairs
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
        assert_eq!(d["edges"], serde_json::json!([[0, 1, 1, 1]]));
    }

    #[test]
    fn edge_direction_bits_say_who_points_at_whom() {
        let reverse = index_of(&[("x/a.rs", "fn a() {}\n"), ("y/b.rs", "fn b() { a(); }\n")]);
        assert_eq!(data(&render("d", &reverse).0)["edges"][0][3], 2);
        let both = index_of(&[
            ("x/a.rs", "fn a() { b(); }\n"),
            ("y/b.rs", "fn b() { a(); }\n"),
        ]);
        assert_eq!(data(&render("d", &both).0)["edges"][0][3], 3);
    }

    #[test]
    fn import_without_calls_points_from_the_importer() {
        let imports = index_of(&[
            ("x/a.ts", "import { b } from '../y/b';\n"),
            ("y/b.ts", "export function b() {}\n"),
        ]);
        let d = data(&render("d", &imports).0);
        assert_eq!(d["edges"], serde_json::json!([[0, 1, 1, 1]]));
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
    fn page_is_assembled_from_its_source_parts() {
        let page = render("demo", &Index::default()).0;
        assert!(!page.contains("/*STYLE*/") && !page.contains("/*SCRIPT*/"));
        assert!(page.contains("<style>\n") && page.contains("</style>"));
        for part in SCRIPT_PARTS {
            // the data marker is replaced by the real data, so compare up to it
            let first = part
                .lines()
                .next()
                .unwrap()
                .split("/*DATA*/")
                .next()
                .unwrap();
            assert!(page.contains(first), "script part missing: {first}");
        }
    }

    #[test]
    fn source_parts_do_not_contain_build_placeholders() {
        let data_marks: usize = SCRIPT_PARTS
            .iter()
            .chain([&STYLE, &TEMPLATE])
            .map(|p| p.matches("/*DATA*/").count())
            .sum();
        assert_eq!(
            data_marks, 1,
            "exactly one /*DATA*/ across all the page sources"
        );
        for part in SCRIPT_PARTS.iter().chain([&STYLE]) {
            assert!(!part.contains("/*STYLE*/") && !part.contains("/*SCRIPT*/"));
            assert!(!part.contains("{{TITLE}}"));
        }
    }

    #[test]
    fn empty_index_still_renders() {
        let (page, count) = render("demo", &Index::default());
        assert_eq!(count, 0);
        assert_eq!(data(&page)["nodes"], serde_json::json!([]));
    }
}
