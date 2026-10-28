//! Answers def, callers and path questions over an `Index`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::Serialize;

use crate::lang::Lang;
use crate::model::{FileEntry, Index, Kind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefHit {
    pub path: String,
    pub line: u32,
    pub signature: String,
}

impl DefHit {
    pub fn text(&self) -> String {
        format!("{}:{} {}", self.path, self.line, self.signature)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallerHit {
    pub path: String,
    pub line: u32,
    /// Enclosing function; `None` means top level.
    pub caller: Option<String>,
}

impl CallerHit {
    pub fn text(&self) -> String {
        let caller = self.caller.as_deref().unwrap_or("(top level)");
        format!("{}:{}  in {caller}", self.path, self.line)
    }
}

/// One step of a `path` answer: a function (with its definition site, if known) or a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hop {
    pub name: String,
    pub location: Option<String>,
}

impl Hop {
    pub fn text(&self) -> String {
        match &self.location {
            Some(loc) => format!("{}  {loc}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Every definition of `name`. `impl` blocks only count when nothing else matches,
/// so `def Foo` points at the type, not at each `impl Foo`.
pub fn def(index: &Index, name: &str) -> Vec<DefHit> {
    let hits = |impls: bool| -> Vec<DefHit> {
        index
            .files
            .values()
            .flat_map(|f| {
                f.symbols
                    .iter()
                    .filter(move |s| s.name == name && (s.kind == Kind::Impl) == impls)
                    .map(|s| DefHit {
                        path: f.path.clone(),
                        line: s.line,
                        signature: s.signature.clone(),
                    })
            })
            .collect()
    };
    let found = hits(false);
    if found.is_empty() { hits(true) } else { found }
}

/// Every call site of `name`, with the function it sits in.
pub fn callers(index: &Index, name: &str) -> Vec<CallerHit> {
    index
        .files
        .values()
        .flat_map(|f| {
            f.calls
                .iter()
                .filter(move |c| c.callee == name)
                .map(|c| CallerHit {
                    path: f.path.clone(),
                    line: c.line,
                    caller: c.caller.clone(),
                })
        })
        .collect()
}

/// Shortest chain from `from` to `to`. Two indexed file paths are linked by imports;
/// anything else is treated as function names linked by calls. Empty means no path.
pub fn path(index: &Index, from: &str, to: &str) -> Vec<Hop> {
    if index.files.contains_key(from) && index.files.contains_key(to) {
        let edges = import_edges(index);
        return bfs(&edges, from, to)
            .into_iter()
            .map(|name| Hop {
                name,
                location: None,
            })
            .collect();
    }
    let edges = call_edges(index);
    if from == to && !edges.contains_key(from) && def(index, from).is_empty() {
        return Vec::new();
    }
    bfs(&edges, from, to)
        .into_iter()
        .map(|name| {
            let location = def(index, &name)
                .first()
                .map(|d| format!("{}:{}", d.path, d.line));
            Hop { name, location }
        })
        .collect()
}

type Edges = HashMap<String, BTreeSet<String>>;

/// Breadth-first search: visit neighbours level by level, so the first time we reach
/// `to` is along a shortest chain.
fn bfs(edges: &Edges, from: &str, to: &str) -> Vec<String> {
    let mut parent: HashMap<&str, &str> = HashMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(node) = queue.pop_front() {
        if node == to {
            let mut chain = vec![node.to_string()];
            let mut cur = node;
            while let Some(&p) = parent.get(cur) {
                chain.push(p.to_string());
                cur = p;
            }
            chain.reverse();
            return chain;
        }
        for next in edges.get(node).into_iter().flatten() {
            if seen.insert(next) {
                parent.insert(next, node);
                queue.push_back(next);
            }
        }
    }
    Vec::new()
}

/// Function name -> names it calls. A caller like `pub async fn foo<T>` becomes `foo`.
pub(crate) fn call_edges(index: &Index) -> Edges {
    let mut edges = Edges::new();
    for call in index.files.values().flat_map(|f| &f.calls) {
        if let Some(caller) = call.caller.as_deref() {
            edges
                .entry(bare_name(caller))
                .or_default()
                .insert(call.callee.clone());
        }
    }
    edges
}

/// `pub async fn foo` -> `foo`, `export const Cart` -> `Cart`.
pub(crate) fn bare_name(caller: &str) -> String {
    caller
        .split_whitespace()
        .last()
        .unwrap_or(caller)
        .to_string()
}

/// File -> files it imports (only imports that resolve to an indexed file).
pub(crate) fn import_edges(index: &Index) -> Edges {
    let mut edges = Edges::new();
    for file in index.files.values() {
        for imp in &file.imports {
            if let Some(target) = resolve_import(&index.files, &index.aliases, &file.path, imp) {
                edges.entry(file.path.clone()).or_default().insert(target);
            }
        }
    }
    edges
}

/// Imports worth showing for a file: all of a Rust file's, but for TypeScript only relative paths
/// and aliases (packages like `react` have no file in the project).
pub fn local_imports<'a>(index: &Index, entry: &'a FileEntry) -> Vec<&'a str> {
    let is_rust = Lang::from_path(&entry.path) == Some(Lang::Rust);
    entry
        .imports
        .iter()
        .map(String::as_str)
        .filter(|imp| {
            is_rust
                || imp.starts_with('.')
                || index.aliases.keys().any(|p| imp.starts_with(p.as_str()))
        })
        .collect()
}

fn resolve_import<V>(
    files: &BTreeMap<String, V>,
    aliases: &BTreeMap<String, String>,
    from: &str,
    imp: &str,
) -> Option<String> {
    match Lang::from_path(from) {
        Some(Lang::Rust) => resolve_rust_module(files, from, imp),
        _ => resolve_ts_import(files, aliases, from, imp),
    }
}

/// Relative path (`./x`, `../x`) or tsconfig alias (`~/x`); anything else is a package.
fn resolve_ts_import<V>(
    files: &BTreeMap<String, V>,
    aliases: &BTreeMap<String, String>,
    from: &str,
    imp: &str,
) -> Option<String> {
    let dir = from.rsplit_once('/').map_or("", |(d, _)| d);
    // TS writes `./x.js` for `x.ts`, so a TS importer tries the stripped name first; a JS importer
    // means the file itself, so it tries the name as written first.
    let stripped = [".js", ".jsx", ".ts", ".tsx"]
        .iter()
        .find_map(|ext| imp.strip_suffix(ext))
        .unwrap_or(imp);
    let from_ts = matches!(Lang::from_path(from), Some(Lang::TypeScript | Lang::Tsx));
    let order = if from_ts {
        [stripped, imp]
    } else {
        [imp, stripped]
    };
    order.iter().find_map(|name| {
        let base = import_base(aliases, dir, name)?;
        ["", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
            .iter()
            .map(|ext| format!("{base}{ext}"))
            .chain(
                ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"]
                    .iter()
                    .map(|index| format!("{base}{index}")),
            )
            .find(|p| files.contains_key(p))
    })
}

/// Folder-relative path of an import: `./x` next to the file, or an alias replaced by its folder.
fn import_base(aliases: &BTreeMap<String, String>, dir: &str, imp: &str) -> Option<String> {
    if imp.starts_with('.') {
        return Some(normalize(&format!("{dir}/{imp}")));
    }
    // The longest matching prefix wins, so `@components/` beats `@/`.
    let (prefix, folder) = aliases
        .iter()
        .filter(|(p, _)| imp.starts_with(p.as_str()))
        .max_by_key(|(p, _)| p.len())?;
    Some(format!("{folder}{}", &imp[prefix.len()..]))
}

/// Rust module `imp` declared in `from`. `mod a;` in `foo.rs` lives in `foo/a.rs`; in `mod.rs`,
/// `lib.rs` or `main.rs` it lives next to them. Failing that, accept a project-wide match only
/// when it is the single candidate.
fn resolve_rust_module<V>(files: &BTreeMap<String, V>, from: &str, imp: &str) -> Option<String> {
    let (dir, file) = from.rsplit_once('/').unwrap_or(("", from));
    let stem = file
        .strip_suffix(".rs")
        .filter(|s| !matches!(*s, "mod" | "lib" | "main"));
    let folder = match (stem, dir) {
        (Some(s), "") => s.to_string(),
        (Some(s), d) => format!("{d}/{s}"),
        (None, d) => d.to_string(),
    };
    let join = |tail: &str| {
        if folder.is_empty() {
            format!("{imp}{tail}")
        } else {
            format!("{folder}/{imp}{tail}")
        }
    };
    [join(".rs"), join("/mod.rs")]
        .into_iter()
        .find(|p| files.contains_key(p))
        .or_else(|| {
            let tails = [format!("{imp}.rs"), format!("{imp}/mod.rs")];
            let suffixes = tails.clone().map(|tail| format!("/{tail}"));
            let mut matches = files.keys().filter(|p| {
                tails.iter().any(|tail| *p == tail) || suffixes.iter().any(|sfx| p.ends_with(sfx))
            });
            match (matches.next(), matches.next()) {
                (Some(only), None) => Some(only.clone()),
                _ => None,
            }
        })
}

/// Collapse `a/b/../c` and `./` segments.
pub(crate) fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => _ = parts.pop(),
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// A neighbour of a symbol in the call graph, with how many call sites connect them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    pub name: String,
    pub location: Option<String>,
    pub sites: usize,
}

impl Link {
    pub fn text(&self) -> String {
        let unit = if self.sites == 1 { "site" } else { "sites" };
        match &self.location {
            Some(loc) => format!("{}  {loc} ({} {unit})", self.name, self.sites),
            None => format!("{} ({} {unit})", self.name, self.sites),
        }
    }
}

/// Everything about one name: where it is defined, who calls it, what it calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Explain {
    pub defs: Vec<DefHit>,
    pub incoming: Vec<Link>,
    pub outgoing: Vec<Link>,
}

/// Longest list printed in text mode; `--json` always has everything.
const LIST_LIMIT: usize = 20;

impl Explain {
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .defs
            .iter()
            .map(|d| format!("def {}", d.text()))
            .collect();
        for (arrow, links) in [("<-", &self.incoming), ("->", &self.outgoing)] {
            out.extend(
                links
                    .iter()
                    .take(LIST_LIMIT)
                    .map(|l| format!("{arrow} {}", l.text())),
            );
            if links.len() > LIST_LIMIT {
                out.push(format!("{arrow} ... {} more", links.len() - LIST_LIMIT));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Affected {
    pub name: String,
    pub location: Option<String>,
    /// 1 = calls the target directly, 2 = calls a direct caller, and so on.
    pub depth: usize,
}

impl Affected {
    pub fn text(&self) -> String {
        match &self.location {
            Some(loc) => format!("{}  {loc}  depth {}", self.name, self.depth),
            None => format!("{}  depth {}", self.name, self.depth),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hub {
    pub name: String,
    pub location: String,
    pub incoming: usize,
    pub outgoing: usize,
}

impl Hub {
    pub fn text(&self) -> String {
        format!(
            "{}  {}  in={} out={}",
            self.name, self.location, self.incoming, self.outgoing
        )
    }
}

/// Name -> `path:line` of its first definition (`impl` blocks don't count).
fn locations(index: &Index) -> HashMap<&str, String> {
    let mut map = HashMap::new();
    for file in index.files.values() {
        for s in file.symbols.iter().filter(|s| s.kind != Kind::Impl) {
            map.entry(s.name.as_str())
                .or_insert_with(|| format!("{}:{}", file.path, s.line));
        }
    }
    map
}

/// Callee name -> the functions that call it (the call graph turned around).
fn reverse(edges: &Edges) -> Edges {
    let mut rev = Edges::new();
    for (caller, callees) in edges {
        for callee in callees {
            rev.entry(callee.clone())
                .or_default()
                .insert(caller.clone());
        }
    }
    rev
}

/// Neighbour counts as links, most call sites first.
fn links(counts: BTreeMap<String, usize>, locs: &HashMap<&str, String>) -> Vec<Link> {
    let mut links: Vec<Link> = counts
        .into_iter()
        .map(|(name, sites)| Link {
            location: locs.get(name.as_str()).cloned(),
            name,
            sites,
        })
        .collect();
    links.sort_by(|a, b| b.sites.cmp(&a.sites).then_with(|| a.name.cmp(&b.name)));
    links
}

/// Definition, callers and callees of `name`. Callees are the calls made by functions of that
/// name inside the files that define it.
pub fn explain(index: &Index, name: &str) -> Explain {
    let defs = def(index, name);
    let locs = locations(index);
    let mut incoming: BTreeMap<String, usize> = BTreeMap::new();
    for call in index
        .files
        .values()
        .flat_map(|f| &f.calls)
        .filter(|c| c.callee == name)
    {
        let Some(caller) = call.caller.as_deref().map(bare_name) else {
            continue; // top-level code is not a symbol
        };
        *incoming.entry(caller).or_default() += 1;
    }
    let mut outgoing: BTreeMap<String, usize> = BTreeMap::new();
    for file in defs.iter().filter_map(|d| index.files.get(&d.path)) {
        for call in &file.calls {
            if call.caller.as_deref().map(bare_name).as_deref() == Some(name) {
                *outgoing.entry(call.callee.clone()).or_default() += 1;
            }
        }
    }
    Explain {
        defs,
        incoming: links(incoming, &locs),
        outgoing: links(outgoing, &locs),
    }
}

/// Everything that depends on `name`: its callers, their callers, and so on, nearest first.
/// `max_depth` limits how far to walk.
pub fn affected(index: &Index, name: &str, max_depth: Option<usize>) -> Vec<Affected> {
    let rev = reverse(&call_edges(index));
    let locs = locations(index);
    let mut seen = BTreeSet::from([name.to_string()]);
    let mut frontier = vec![name.to_string()];
    let mut out = Vec::new();
    let mut depth = 0;
    while !frontier.is_empty() && max_depth.is_none_or(|max| depth < max) {
        depth += 1;
        let mut next = BTreeSet::new();
        for caller in frontier.iter().filter_map(|n| rev.get(n)).flatten() {
            if seen.insert(caller.clone()) {
                next.insert(caller.clone());
            }
        }
        for caller in &next {
            out.push(Affected {
                name: caller.clone(),
                location: locs.get(caller.as_str()).cloned(),
                depth,
            });
        }
        frontier = next.into_iter().collect();
    }
    out
}

/// The `top` most-connected project symbols (distinct project callers + distinct project callees).
/// Names defined nowhere in the project, like `println` or `new`, are left out.
pub fn hubs(index: &Index, top: usize) -> Vec<Hub> {
    let locs = locations(index);
    let fwd = call_edges(index);
    let rev = reverse(&fwd);
    let count = |edges: &Edges, name: &str| {
        edges.get(name).map_or(0, |set| {
            set.iter()
                .filter(|n| n.as_str() != name && locs.contains_key(n.as_str()))
                .count()
        })
    };
    let mut hubs: Vec<Hub> = locs
        .iter()
        .map(|(name, loc)| Hub {
            name: name.to_string(),
            location: loc.clone(),
            incoming: count(&rev, name),
            outgoing: count(&fwd, name),
        })
        .filter(|h| h.incoming + h.outgoing > 0)
        .collect();
    hubs.sort_by(|a, b| {
        (b.incoming + b.outgoing)
            .cmp(&(a.incoming + a.outgoing))
            .then_with(|| a.name.cmp(&b.name))
    });
    hubs.truncate(top);
    hubs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{self, Lang};
    use crate::model::FileEntry;

    fn index_of(files: &[(&str, &str)]) -> Index {
        let mut index = Index {
            aliases: BTreeMap::from([("@/".to_string(), "src/".to_string())]),
            ..Index::default()
        };
        for (path, src) in files {
            let lang = Lang::from_path(path).unwrap();
            let facts = lang::extract(lang, src).unwrap();
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

    #[test]
    fn rust_module_fallback_needs_exactly_one_suffix_match() {
        let files: BTreeMap<String, ()> = ["a/x.rs", "b/y/mod.rs", "c/z.rs", "d/z.rs", "main.rs"]
            .iter()
            .map(|p| (p.to_string(), ()))
            .collect();
        let find = |imp: &str| resolve_rust_module(&files, "main.rs", imp);
        assert_eq!(find("x"), Some("a/x.rs".to_string()));
        assert_eq!(find("y"), Some("b/y/mod.rs".to_string()));
        assert_eq!(find("z"), None);
        assert_eq!(find("w"), None);
    }

    #[test]
    fn def_lists_all_candidates_and_hides_impls() {
        let index = index_of(&[
            ("a.rs", "pub struct Foo;\nimpl Foo { fn new() {} }\n"),
            ("b.ts", "export class Foo {}\n"),
        ]);
        let got: Vec<_> = def(&index, "Foo").iter().map(DefHit::text).collect();
        assert_eq!(
            got,
            vec!["a.rs:1 pub struct Foo;", "b.ts:1 export class Foo"]
        );
    }

    #[test]
    fn def_falls_back_to_impl() {
        let index = index_of(&[("a.rs", "impl Foo {}\n")]);
        assert_eq!(def(&index, "Foo")[0].text(), "a.rs:1 impl Foo");
    }

    #[test]
    fn callers_report_enclosing_fn() {
        let index = index_of(&[(
            "a.rs",
            "fn a() {}\nfn b() {\n    a();\n}\nfn c() { x.a(); }\n",
        )]);
        let got: Vec<_> = callers(&index, "a").iter().map(CallerHit::text).collect();
        assert_eq!(got, vec!["a.rs:3  in fn b", "a.rs:5  in fn c"]);
    }

    #[test]
    fn path_follows_calls() {
        let index = index_of(&[(
            "a.rs",
            "fn a() { b(); }\nfn b() { c(); }\nfn c() {}\nfn d() {}\n",
        )]);
        let names: Vec<_> = path(&index, "a", "c").into_iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert_eq!(path(&index, "a", "c")[2].text(), "c  a.rs:3");
        assert!(path(&index, "a", "d").is_empty());
    }

    #[test]
    fn path_follows_imports() {
        let index = index_of(&[
            ("src/app.ts", "import { x } from './lib/x';\n"),
            ("src/lib/x.ts", "import { y } from '../util';\n"),
            ("src/util.ts", "export const y = 1;\n"),
            ("src/lone.ts", "export const z = 1;\n"),
        ]);
        let got: Vec<_> = path(&index, "src/app.ts", "src/util.ts")
            .into_iter()
            .map(|h| h.name)
            .collect();
        assert_eq!(got, vec!["src/app.ts", "src/lib/x.ts", "src/util.ts"]);
        assert!(path(&index, "src/app.ts", "src/lone.ts").is_empty());
    }

    #[test]
    fn path_follows_rust_module_imports() {
        let index = index_of(&[
            ("src/main.rs", "mod walk;\n"),
            ("src/walk.rs", "fn w() {}\n"),
        ]);
        let got: Vec<_> = path(&index, "src/main.rs", "src/walk.rs")
            .into_iter()
            .map(|h| h.name)
            .collect();
        assert_eq!(got, vec!["src/main.rs", "src/walk.rs"]);
    }

    #[test]
    fn path_survives_bounded_generics() {
        let index = index_of(&[(
            "a.rs",
            "pub async fn a<T: Into<String>>(x: T) { b(); }\nfn b<F: Fn(u8)>(f: F) { c(); }\nfn c() {}\n",
        )]);
        let names: Vec<_> = path(&index, "a", "c").into_iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn path_to_unknown_name_is_empty() {
        let index = index_of(&[("a.rs", "fn a() {}\n")]);
        assert!(path(&index, "nope", "nope").is_empty());
    }

    #[test]
    fn ts_imports_resolve_js_extension_and_alias() {
        let index = index_of(&[
            (
                "src/app.ts",
                "import { x } from './x.js';\nimport { y } from '@/lib/y';\n",
            ),
            ("src/x.ts", "export const x = 1;\n"),
            ("src/lib/y.ts", "export const y = 1;\n"),
        ]);
        assert_eq!(path(&index, "src/app.ts", "src/x.ts").len(), 2);
        assert_eq!(path(&index, "src/app.ts", "src/lib/y.ts").len(), 2);
    }

    #[test]
    fn js_files_are_parsed_and_their_imports_resolve() {
        let index = index_of(&[
            (
                "src/app.js",
                "import { x } from './x.js';\nimport y from './y';\nimport z from './z.mjs';\nexport function run() { x(); }\n",
            ),
            ("src/x.js", "export function x() {}\n"),
            ("src/y/index.jsx", "export default function y() {}\n"),
            ("src/z.mjs", "export default function z() {}\n"),
        ]);
        assert_eq!(def(&index, "run").len(), 1);
        for target in ["src/x.js", "src/y/index.jsx", "src/z.mjs"] {
            assert_eq!(path(&index, "src/app.js", target).len(), 2, "{target}");
        }
    }

    #[test]
    fn js_suffix_order_depends_on_the_importer() {
        let files = [
            ("src/x.ts", "export const x = 1;\n"),
            ("src/x.js", "export const x = 2;\n"),
            ("src/lib/index.js", "export const l = 1;\n"),
        ];
        let ts = index_of(
            &[
                &files[..],
                &[("src/a.ts", "import './x.js';\nimport '@/lib';\n")],
            ]
            .concat(),
        );
        assert_eq!(path(&ts, "src/a.ts", "src/x.ts").len(), 2);
        assert!(path(&ts, "src/a.ts", "src/x.js").is_empty());
        assert_eq!(path(&ts, "src/a.ts", "src/lib/index.js").len(), 2);
        let js = index_of(
            &[
                &files[..],
                &[("src/a.js", "import './x.js';\nimport './x.config';\n")],
            ]
            .concat(),
        );
        assert_eq!(path(&js, "src/a.js", "src/x.js").len(), 2);
    }

    #[test]
    fn ambiguous_rust_module_names_make_no_edge() {
        let index = index_of(&[
            ("src/main.rs", "mod error;\n"),
            ("a/error.rs", "fn e() {}\n"),
            ("b/error.rs", "fn e() {}\n"),
        ]);
        assert!(path(&index, "src/main.rs", "a/error.rs").is_empty());
    }

    #[test]
    fn alias_folder_comes_from_the_index() {
        let mut index = index_of(&[
            ("app/page.ts", "import { y } from '@/lib/y';\n"),
            ("app/lib/y.ts", "export const y = 1;\n"),
        ]);
        index.aliases = BTreeMap::from([("@/".to_string(), "app/".to_string())]);
        assert_eq!(path(&index, "app/page.ts", "app/lib/y.ts").len(), 2);
    }

    #[test]
    fn every_configured_alias_resolves_and_packages_do_not() {
        let mut index = index_of(&[
            (
                "src/app.ts",
                "import a from '~/util';\nimport b from '@components/Btn';\nimport r from 'react';\n",
            ),
            ("src/util.ts", "export const a = 1;\n"),
            ("src/components/Btn.tsx", "export const b = 1;\n"),
            ("react.ts", "export const r = 1;\n"),
        ]);
        index.aliases = BTreeMap::from([
            ("~/".to_string(), "src/".to_string()),
            ("@components/".to_string(), "src/components/".to_string()),
        ]);
        assert_eq!(path(&index, "src/app.ts", "src/util.ts").len(), 2);
        assert_eq!(
            path(&index, "src/app.ts", "src/components/Btn.tsx").len(),
            2
        );
        // `react` is an external package, not the local file that happens to share its name.
        assert!(path(&index, "src/app.ts", "react.ts").is_empty());
    }

    #[test]
    fn local_imports_hide_packages_in_ts_but_not_in_rust() {
        let index = index_of(&[
            (
                "a.ts",
                "import r from 'react';\nimport x from './x';\nimport y from '@/y';\n",
            ),
            ("b.rs", "use serde::Serialize;\n"),
        ]);
        assert_eq!(
            local_imports(&index, &index.files["a.ts"]),
            vec!["./x", "@/y"]
        );
        assert_eq!(local_imports(&index, &index.files["b.rs"]), vec!["serde"]);
    }

    #[test]
    fn rust_modules_resolve_by_file_style() {
        let index = index_of(&[
            ("src/main.rs", "mod foo;\nmod error;\n"),
            ("src/foo.rs", "mod a;\n"),
            ("src/foo/a.rs", "fn a() {}\n"),
            ("src/other/a.rs", "fn a() {}\n"),
            ("error.rs", "fn e() {}\n"),
        ]);
        // 2018-style nesting wins over the project-wide ambiguity of `a.rs`.
        assert_eq!(path(&index, "src/foo.rs", "src/foo/a.rs").len(), 2);
        // A root-level module is found when it is the only `error.rs`.
        assert_eq!(path(&index, "src/main.rs", "error.rs").len(), 2);
    }

    const GRAPH: &str = "fn a() { b(); c(); }\nfn b() { c(); }\nfn c() {}\nfn d() { a(); }\n";

    #[test]
    fn explain_lists_defs_callers_and_callees() {
        let index = index_of(&[("a.rs", GRAPH)]);
        let e = explain(&index, "a");
        assert_eq!(e.defs[0].text(), "a.rs:1 fn a()");
        let names = |links: &[Link]| links.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&e.incoming), vec!["d"]);
        assert_eq!(names(&e.outgoing), vec!["b", "c"]);
        assert_eq!(e.outgoing[1].text(), "c  a.rs:3 (1 site)");
        let lines = e.lines();
        assert_eq!(lines[0], "def a.rs:1 fn a()");
        assert_eq!(lines[1], "<- d  a.rs:4 (1 site)");
        assert_eq!(lines[2], "-> b  a.rs:2 (1 site)");
    }

    #[test]
    fn explain_unknown_name_is_empty() {
        let index = index_of(&[("a.rs", GRAPH)]);
        assert!(explain(&index, "nope").lines().is_empty());
    }

    #[test]
    fn affected_walks_callers_nearest_first() {
        let index = index_of(&[("a.rs", GRAPH)]);
        let all: Vec<_> = affected(&index, "c", None)
            .iter()
            .map(|a| (a.name.clone(), a.depth))
            .collect();
        assert_eq!(all, vec![("a".into(), 1), ("b".into(), 1), ("d".into(), 2)]);
        let near = affected(&index, "c", Some(1));
        assert_eq!(near.len(), 2);
        assert_eq!(near[0].text(), "a  a.rs:1  depth 1");
        assert!(affected(&index, "d", None).is_empty());
    }

    #[test]
    fn hubs_rank_project_symbols_by_connections() {
        let index = index_of(&[(
            "a.rs",
            &format!("{GRAPH}fn e() {{ println!(\"x\"); a(); }}\n"),
        )]);
        let top: Vec<_> = hubs(&index, 3)
            .iter()
            .map(|h| (h.name.clone(), h.incoming, h.outgoing))
            .collect();
        // a: callers d,e / callees b,c; println is external and never listed.
        assert_eq!(
            top,
            vec![("a".into(), 2, 2), ("b".into(), 1, 1), ("c".into(), 2, 0)]
        );
        assert_eq!(hubs(&index, 1)[0].text(), "a  a.rs:1  in=2 out=2");
    }

    #[test]
    fn hubs_ignore_self_calls() {
        let index = index_of(&[("a.rs", "fn r() { r(); }\nfn s() { r(); }\n")]);
        let r = &hubs(&index, 5)[0];
        assert_eq!((r.name.as_str(), r.incoming, r.outgoing), ("r", 1, 0));
    }

    #[test]
    fn explain_handles_ambiguous_names_and_undefined_callees() {
        let index = index_of(&[
            (
                "a.rs",
                "fn dup() {}\nfn user() { dup(); println!(\"x\"); }\n",
            ),
            ("b.ts", "export function dup() {}\n"),
        ]);
        assert_eq!(explain(&index, "dup").defs.len(), 2);
        let external = explain(&index, "println");
        assert!(external.defs.is_empty());
        assert_eq!(external.incoming[0].name, "user");
    }

    #[test]
    fn explain_leaves_out_top_level_callers() {
        let index = index_of(&[(
            "a.rs",
            "fn target() {}\nfn user() { target(); }\ntarget();\n",
        )]);
        let names: Vec<_> = explain(&index, "target")
            .incoming
            .iter()
            .map(|l| l.name.clone())
            .collect();
        assert_eq!(names, vec!["user"]);
    }
}
