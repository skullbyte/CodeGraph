//! Answers def, callers and path questions over an `Index`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::lang::Lang;
use crate::model::{FileEntry, Index, Kind, Resolution};

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
    /// Definition this call goes to (`path:line`), shown only when SCIP told several apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// A mention in a markdown doc (the heading it sits under), not a call.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub doc: bool,
}

impl CallerHit {
    pub fn text(&self) -> String {
        let caller = self.caller.as_deref().unwrap_or("(top level)");
        let base = format!("{}:{}  in {caller}", self.path, self.line);
        match (&self.target, self.doc) {
            (Some(target), _) => format!("{base}  -> {target}"),
            (None, true) => format!("{base}  [doc]"),
            (None, false) => base,
        }
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
                    .filter(move |s| {
                        s.name == name && s.kind != Kind::Heading && (s.kind == Kind::Impl) == impls
                    })
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
    let mut hits: Vec<CallerHit> = index
        .files
        .values()
        .flat_map(|f| {
            f.graph_calls()
                .filter(move |(c, _)| c.callee == name)
                .map(|(c, target)| CallerHit {
                    path: f.path.clone(),
                    line: c.line,
                    caller: c.caller.clone(),
                    target: match target {
                        Some(Resolution::Def { path, line }) => Some(format!("{path}:{line}")),
                        _ => None,
                    },
                    doc: false,
                })
        })
        .collect();
    // With one definition there is nothing to tell apart; with several, say which each hit calls.
    let definitions: BTreeSet<_> = hits.iter().filter_map(|h| h.target.as_deref()).collect();
    if definitions.len() < 2 {
        hits.iter_mut().for_each(|h| h.target = None);
    }
    hits.extend(doc_mentions(index, name));
    hits
}

/// Where docs mention `name` in `code`, listed only when the code defines it exactly once
/// (Graphify's unique-match rule): an ambiguous name could point at any of its definitions.
fn doc_mentions(index: &Index, name: &str) -> Vec<CallerHit> {
    if definition_counts(index).get(name).copied() != Some(1) {
        return Vec::new();
    }
    index
        .files
        .values()
        .filter(|f| f.is_doc())
        .flat_map(|f| {
            f.calls
                .iter()
                .filter(|c| c.callee == name)
                .map(|c| CallerHit {
                    path: f.path.clone(),
                    line: c.line,
                    caller: c.caller.clone(),
                    target: None,
                    doc: true,
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
    for (call, _) in index.files.values().flat_map(|f| f.graph_calls()) {
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

/// Imports worth showing for a file: all of a Rust file's; for TypeScript/JavaScript only relative
/// paths and aliases (packages like `react` have no file in the project); for the other languages
/// only imports that resolve to a project file.
pub fn local_imports<'a>(index: &Index, entry: &'a FileEntry) -> Vec<&'a str> {
    let lang = Lang::from_path(&entry.path);
    let web = matches!(lang, Some(Lang::TypeScript | Lang::Tsx | Lang::JavaScript));
    entry
        .imports
        .iter()
        .map(String::as_str)
        .filter(|imp| match lang {
            Some(Lang::Rust) => true,
            _ if web => {
                imp.starts_with('.') || index.aliases.keys().any(|p| imp.starts_with(p.as_str()))
            }
            _ => resolve_import(&index.files, &index.aliases, &entry.path, imp).is_some(),
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
        Some(Lang::Python) => resolve_python_module(files, from, imp),
        Some(Lang::Java) => resolve_java_class(files, imp),
        Some(Lang::Kotlin) => resolve_kotlin_class(files, imp),
        Some(Lang::Php) => resolve_php_class(files, aliases, imp),
        Some(Lang::C | Lang::Cpp | Lang::Header | Lang::Markdown) => {
            resolve_include(files, from, imp)
        }
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

/// Python module `imp`: `.x` / `..x` count folders up from the file; `a.b` is `a/b.py` or
/// `a/b/__init__.py`, from the project root or, failing that, a single project-wide match.
fn resolve_python_module<V>(files: &BTreeMap<String, V>, from: &str, imp: &str) -> Option<String> {
    let dots = imp.chars().take_while(|&c| c == '.').count();
    let rest = imp[dots..].replace('.', "/");
    if rest.is_empty() {
        return None;
    }
    let candidates = |base: &str| {
        let join = |tail: &str| match (base.is_empty(), rest.is_empty()) {
            (true, _) => format!("{rest}{tail}"),
            (false, true) => format!("{base}{tail}"),
            (false, false) => format!("{base}/{rest}{tail}"),
        };
        [join(".py"), join("/__init__.py")]
    };
    if dots > 0 {
        let mut base = from.rsplit_once('/').map_or("", |(d, _)| d);
        for _ in 1..dots {
            // Climbing above the project root has no file to find.
            base = base
                .rsplit_once('/')
                .map_or_else(|| (!base.is_empty()).then_some(""), |(d, _)| Some(d))?;
        }
        return candidates(base).into_iter().find(|p| files.contains_key(p));
    }
    if let Some(found) = candidates("").into_iter().find(|p| files.contains_key(p)) {
        return Some(found);
    }
    single_suffix_match(files, &candidates(""))
}

/// Java class `a.b.C` -> the file `a/b/C.java`, wherever the source root is (`src/main/java/...`).
fn resolve_java_class<V>(files: &BTreeMap<String, V>, imp: &str) -> Option<String> {
    let path = format!("{}.java", imp.replace('.', "/"));
    single_suffix_match(files, &[path])
}

/// Kotlin class `a.b.C` -> the file `a/b/C.kt`, wherever the source root is (a `.kts` script too).
fn resolve_kotlin_class<V>(files: &BTreeMap<String, V>, imp: &str) -> Option<String> {
    let base = imp.replace('.', "/");
    single_suffix_match(files, &[format!("{base}.kt"), format!("{base}.kts")])
}

/// PHP class `App\Util\Fmt`. With composer's PSR-4 roots (`aliases` keys holding a backslash) it
/// follows Composer: the longest matching root first, then shorter ones, the first file that
/// exists wins, and a namespace under no root (a vendor package) makes no edge. Without any root
/// it guesses `App/Util/Fmt.php`, or with leading namespace parts dropped (`src/Util/Fmt.php`),
/// never below two parts; only a candidate matching exactly one file counts. That guess can link
/// a vendor class to a project file with the same last two parts.
fn resolve_php_class<V>(
    files: &BTreeMap<String, V>,
    aliases: &BTreeMap<String, String>,
    imp: &str,
) -> Option<String> {
    let mut roots: Vec<(&String, &String)> =
        aliases.iter().filter(|(p, _)| p.contains('\\')).collect();
    if roots.is_empty() {
        return guess_php_class(files, imp);
    }
    roots.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    roots
        .into_iter()
        .filter(|(prefix, _)| imp.starts_with(prefix.as_str()))
        .find_map(|(prefix, folders)| {
            let rest = imp[prefix.len()..].replace('\\', "/");
            folders
                .split('|')
                .map(|folder| format!("{folder}{rest}.php"))
                .find(|path| files.contains_key(path))
        })
}

fn guess_php_class<V>(files: &BTreeMap<String, V>, imp: &str) -> Option<String> {
    let parts: Vec<&str> = imp.split('\\').filter(|p| !p.is_empty()).collect();
    (2..=parts.len()).rev().find_map(|n| {
        let path = format!("{}.php", parts[parts.len() - n..].join("/"));
        single_suffix_match(files, &[path])
    })
}

/// `#include "x.h"`: next to the including file first, else the one project file with that path.
fn resolve_include<V>(files: &BTreeMap<String, V>, from: &str, imp: &str) -> Option<String> {
    let dir = from.rsplit_once('/').map_or("", |(d, _)| d);
    let near = normalize(&format!("{dir}/{imp}"));
    if files.contains_key(&near) {
        return Some(near);
    }
    single_suffix_match(files, &[imp.to_string()])
}

/// The one file that is, or sits below a folder ending in, one of `paths`; none when several match.
fn single_suffix_match<V>(files: &BTreeMap<String, V>, paths: &[String]) -> Option<String> {
    // Built once per call, not per file: this runs for every import of every file.
    let tails: Vec<String> = paths.iter().map(|p| format!("/{p}")).collect();
    let mut hits = files.keys().filter(|f| {
        paths
            .iter()
            .zip(&tails)
            .any(|(p, tail)| f.as_str() == p || f.ends_with(tail.as_str()))
    });
    let first = hits.next()?;
    hits.next().is_none().then(|| first.clone())
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
        self.lines_with(|_| Vec::new())
    }

    /// Like `lines`, with `under(def)` printed right below each `def` line.
    pub fn lines_with(&self, under: impl Fn(&DefHit) -> Vec<String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in &self.defs {
            out.push(format!("def {}", d.text()));
            out.extend(under(d));
        }
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
        for s in file.symbols.iter().filter(|s| s.kind.is_code_def()) {
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
        .flat_map(|f| f.graph_calls())
        .map(|(call, _)| call)
        .filter(|c| c.callee == name)
    {
        let Some(caller) = call.caller.as_deref().map(bare_name) else {
            continue; // top-level code is not a symbol
        };
        *incoming.entry(caller).or_default() += 1;
    }
    let mut outgoing: BTreeMap<String, usize> = BTreeMap::new();
    for file in defs.iter().filter_map(|d| index.files.get(&d.path)) {
        for (call, _) in file.graph_calls() {
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

/// A hub candidate: a name, plus its definition when the name has several and SCIP told them apart.
type HubKey = (String, Option<(String, u32)>);

/// The `top` most-connected project symbols (distinct project callers + distinct project callees).
/// Names defined nowhere in the project, like `println` or `new`, are left out. Once `codegraph scip`
/// has run, same-named definitions (every `new`) are separate hubs; a call SCIP did not resolve to
/// such a name cannot be attributed and is skipped.
pub fn hubs(index: &Index, top: usize) -> Vec<Hub> {
    let locs = locations(index);
    let defined = definition_counts(index);
    // Languages for which `codegraph scip` has run; others keep counting by name.
    let scip_languages: HashSet<String> = index
        .files
        .values()
        .filter(|f| !f.resolved.is_empty())
        .map(|f| scip_family(&f.path))
        .collect();
    let key =
        |file: &FileEntry, name: &str, def: Option<(&str, u32)>, caller: bool| -> Option<HubKey> {
            let by_name = (name.to_string(), None);
            if defined.get(name).copied().unwrap_or(0) < 2
                || !scip_languages.contains(&scip_family(&file.path))
            {
                return Some(by_name);
            }
            let attributed = def.and_then(|(path, line)| {
                let own = if path == file.path {
                    file
                } else {
                    index.files.get(path)?
                };
                // SCIP gives the identifier's line; our symbol starts at its declaration.
                let at = own
                    .symbols
                    .iter()
                    .filter(|s| s.name == name && s.kind.is_code_def() && s.line <= line)
                    .map(|s| s.line)
                    .max()?;
                Some((name.to_string(), Some((own.path.clone(), at))))
            });
            // A caller we cannot place (a nested fn the extractor named but did not list) keeps its
            // name; a call we cannot attribute to one of several definitions is left out.
            attributed.or(caller.then_some(by_name))
        };
    let mut fwd: HashMap<HubKey, HashSet<HubKey>> = HashMap::new();
    let mut rev: HashMap<HubKey, HashSet<HubKey>> = HashMap::new();
    for file in index.files.values() {
        for (call, target) in file.graph_calls() {
            let Some(caller) = call.caller.as_deref().map(bare_name) else {
                continue;
            };
            let callee_def = match target {
                Some(Resolution::Def { path, line }) => Some((path.as_str(), *line)),
                _ => None,
            };
            let from = key(file, &caller, Some((file.path.as_str(), call.line)), true);
            let to = key(file, &call.callee, callee_def, false);
            let (Some(from), Some(to)) = (from, to) else {
                continue;
            };
            if from != to {
                fwd.entry(from.clone()).or_default().insert(to.clone());
                rev.entry(to).or_default().insert(from);
            }
        }
    }
    let count = |edges: &HashMap<HubKey, HashSet<HubKey>>, node: &HubKey| {
        edges.get(node).map_or(0, |set| {
            set.iter()
                .filter(|n| locs.contains_key(n.0.as_str()))
                .count()
        })
    };
    let nodes: HashSet<&HubKey> = fwd.keys().chain(rev.keys()).collect();
    let mut hubs: Vec<Hub> = nodes
        .into_iter()
        .filter_map(|node| {
            let many = defined.get(node.0.as_str()).copied().unwrap_or(0);
            let location = match &node.1 {
                Some((path, line)) => format!("{path}:{line}"),
                // Calls kept by name next to per-definition rows: no single place to point at.
                None if many > 1 && !scip_languages.is_empty() => {
                    format!("(by name, {many} definitions)")
                }
                None => locs.get(node.0.as_str())?.clone(),
            };
            Some(Hub {
                name: node.0.clone(),
                location,
                incoming: count(&rev, node),
                outgoing: count(&fwd, node),
            })
        })
        .filter(|h| h.incoming + h.outgoing > 0)
        .collect();
    hubs.sort_by(|a, b| {
        (b.incoming + b.outgoing)
            .cmp(&(a.incoming + a.outgoing))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.location.cmp(&b.location))
    });
    hubs.truncate(top);
    hubs
}

/// Files one SCIP indexer covers together (scip-typescript reads .ts, .tsx and .js; clang, C and C++).
fn scip_family(path: &str) -> String {
    match Lang::from_path(path) {
        Some(Lang::TypeScript | Lang::Tsx | Lang::JavaScript) => "ts".to_string(),
        Some(Lang::C | Lang::Cpp | Lang::Header) => "c".to_string(),
        other => format!("{other:?}"),
    }
}

/// Name -> how many project definitions it has (`impl` blocks don't count).
fn definition_counts(index: &Index) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for file in index.files.values() {
        for s in file.symbols.iter().filter(|s| s.kind.is_code_def()) {
            *counts.entry(s.name.as_str()).or_default() += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{self, Lang};
    use crate::model::{FileEntry, ResolvedCall};

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
    fn docs_never_count_as_code() {
        let index = index_of(&[
            ("a.rs", "pub fn foo() {}\npub fn bar() { foo(); }\n"),
            (
                "README.md",
                "# foo\n\n## Usage\nCall `foo()` and `nothing_here`.\nSee `bar`.\n",
            ),
        ]);
        // A heading is not a definition.
        assert_eq!(def(&index, "foo").len(), 1);
        assert_eq!(definition_counts(&index)["foo"], 1);
        // A doc mention is listed and marked, only for a name with one code definition.
        let got: Vec<_> = callers(&index, "foo").iter().map(CallerHit::text).collect();
        assert_eq!(
            got,
            vec!["a.rs:2  in pub fn bar", "README.md:4  in Usage  [doc]"]
        );
        assert!(callers(&index, "nothing_here").is_empty());
        // And it stays out of the code graph.
        assert!(!call_edges(&index).contains_key("Usage"));
        let hit: Vec<_> = affected(&index, "foo", None)
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(hit, vec!["bar"]);
        assert!(hubs(&index, 10).iter().all(|h| h.name != "Usage"));
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
    fn python_imports_resolve_relative_dotted_and_package_modules() {
        let index = index_of(&[
            (
                "app/main.py",
                "from .util import u\nfrom . import pkg\nfrom ..top import t\nimport app.models.user\nimport os\n",
            ),
            ("app/util.py", "def u(): pass\n"),
            ("app/pkg/__init__.py", "def p(): pass\n"),
            ("top.py", "def t(): pass\n"),
            ("app/models/user.py", "def user(): pass\n"),
        ]);
        for target in [
            "app/util.py",
            "app/pkg/__init__.py",
            "top.py",
            "app/models/user.py",
        ] {
            assert_eq!(path(&index, "app/main.py", target).len(), 2, "{target}");
        }
    }

    #[test]
    fn python_relative_imports_cannot_climb_above_the_root() {
        let index = index_of(&[
            ("app/main.py", "from ...x import y\nfrom . import z\n"),
            ("x.py", "def y(): pass\n"),
            ("app/z.py", "def z(): pass\n"),
        ]);
        assert!(path(&index, "app/main.py", "x.py").is_empty());
        assert_eq!(path(&index, "app/main.py", "app/z.py").len(), 2);
    }

    #[test]
    fn python_local_imports_show_only_project_files() {
        let index = index_of(&[
            (
                "app/main.py",
                "import os\nfrom .util import u\nfrom pkg import sub\n",
            ),
            ("app/util.py", "def u(): pass\n"),
            ("pkg/sub.py", "def s(): pass\n"),
        ]);
        let entry = &index.files["app/main.py"];
        assert_eq!(local_imports(&index, entry), vec![".util", "pkg.sub"]);
        assert_eq!(path(&index, "app/main.py", "pkg/sub.py").len(), 2);
    }

    #[test]
    fn java_imports_resolve_under_any_source_root() {
        let index = index_of(&[
            (
                "src/main/java/app/Store.java",
                "package app;\nimport com.shop.Cart;\nimport static com.shop.Util.fmt;\nimport java.util.List;\nclass Store {}\n",
            ),
            (
                "src/main/java/com/shop/Cart.java",
                "package com.shop;\nclass Cart {}\n",
            ),
            (
                "src/main/java/com/shop/Util.java",
                "package com.shop;\nclass Util {}\n",
            ),
        ]);
        let from = "src/main/java/app/Store.java";
        assert_eq!(
            path(&index, from, "src/main/java/com/shop/Cart.java").len(),
            2
        );
        assert_eq!(
            path(&index, from, "src/main/java/com/shop/Util.java").len(),
            2
        );
        assert_eq!(local_imports(&index, &index.files[from]).len(), 2);
    }

    #[test]
    fn kotlin_imports_resolve_under_any_source_root() {
        let index = index_of(&[
            (
                "src/main/kotlin/app/Store.kt",
                "package app\nimport com.shop.Cart\nimport com.shop.*\nimport kotlin.collections.List\nclass Store\n",
            ),
            (
                "src/main/kotlin/com/shop/Cart.kt",
                "package com.shop\nclass Cart\n",
            ),
        ]);
        let from = "src/main/kotlin/app/Store.kt";
        assert_eq!(
            path(&index, from, "src/main/kotlin/com/shop/Cart.kt").len(),
            2
        );
        assert_eq!(local_imports(&index, &index.files[from]).len(), 1);
    }

    #[test]
    fn php_imports_resolve_with_or_without_the_root_namespace_folder() {
        let index = index_of(&[
            (
                "src/Controller/Home.php",
                "<?php\nnamespace App\\Controller;\nuse App\\Model\\Cart;\nuse App\\Model\\Missing;\nuse Vendor\\Lib\\Thing;\nclass Home {}\n",
            ),
            (
                "src/Model/Cart.php",
                "<?php\nnamespace App\\Model;\nclass Cart {}\n",
            ),
        ]);
        let from = "src/Controller/Home.php";
        assert_eq!(path(&index, from, "src/Model/Cart.php").len(), 2);
        assert_eq!(local_imports(&index, &index.files[from]).len(), 1);
    }

    #[test]
    fn php_ambiguous_single_part_and_function_imports_make_no_edge() {
        let index = index_of(&[
            ("a/Model/Cart.php", "<?php\nclass Cart {}\n"),
            ("b/Model/Cart.php", "<?php\nclass Cart {}\n"),
            ("lib/Foo.php", "<?php\nclass Foo {}\n"),
            (
                "main.php",
                "<?php\nuse App\\Model\\Cart;\nuse Foo;\nuse function App\\Lib\\Foo;\nuse App\\{Foo as F};\n",
            ),
        ]);
        assert!(local_imports(&index, &index.files["main.php"]).is_empty());
    }

    #[test]
    fn php_composer_roots_decide_what_is_a_project_class() {
        let files: BTreeMap<String, ()> = [
            "src/Model/Cart.php",
            "lib/Thing.php",
            "src/Illuminate/Support/Facades/Cache.php",
            "src/Illuminate/Collections/Collection.php",
        ]
        .iter()
        .map(|p| (p.to_string(), ()))
        .collect();
        let roots = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let aliases = roots(&[
            ("App\\", "src/"),
            ("Illuminate\\", "src/Illuminate/"),
            (
                "Illuminate\\Support\\",
                "src/Illuminate/Macroable/|src/Illuminate/Collections/",
            ),
            ("@/", "src/"),
        ]);
        let hit = |imp: &str| resolve_php_class(&files, &aliases, imp);
        assert_eq!(
            hit("App\\Model\\Cart").as_deref(),
            Some("src/Model/Cart.php")
        );
        // The vendor class shares its last two parts with a project file, but no root owns it.
        assert_eq!(hit("Vendor\\Lib\\Thing"), None);
        // The longest root has no such file; the shorter one does (Composer tries them in turn).
        assert_eq!(
            hit("Illuminate\\Support\\Facades\\Cache").as_deref(),
            Some("src/Illuminate/Support/Facades/Cache.php")
        );
        // Several folders under one root.
        assert_eq!(
            hit("Illuminate\\Support\\Collection").as_deref(),
            Some("src/Illuminate/Collections/Collection.php")
        );
        assert_eq!(hit("App\\Missing"), None);
    }

    #[test]
    fn java_ambiguous_and_missing_classes_make_no_edge() {
        let index = index_of(&[
            ("x/a/Foo.java", "class Foo {}\n"),
            ("y/a/Foo.java", "class Foo {}\n"),
            (
                "Main.java",
                "import a.Foo;\nimport java.util.List;\nimport a.*;\nclass Main {}\n",
            ),
        ]);
        assert!(path(&index, "Main.java", "x/a/Foo.java").is_empty());
        assert!(local_imports(&index, &index.files["Main.java"]).is_empty());
    }

    #[test]
    fn c_includes_resolve_near_then_by_unique_path() {
        let index = index_of(&[
            (
                "src/main.c",
                "#include \"util.h\"\n#include \"net/sock.h\"\n#include <stdio.h>\nint main(void) { return 0; }\n",
            ),
            ("src/util.h", "int u(void);\n"),
            ("include/net/sock.h", "int s(void);\n"),
        ]);
        assert_eq!(path(&index, "src/main.c", "src/util.h").len(), 2);
        assert_eq!(path(&index, "src/main.c", "include/net/sock.h").len(), 2);
        assert_eq!(local_imports(&index, &index.files["src/main.c"]).len(), 2);
    }

    #[test]
    fn c_includes_with_dotdot_work_and_ambiguous_names_make_no_edge() {
        let index = index_of(&[
            (
                "src/a/main.c",
                "#include \"../x.h\"\n#include \"util.h\"\nint main(void) { return 0; }\n",
            ),
            ("src/x.h", "int x(void);\n"),
            ("one/util.h", "int u(void);\n"),
            ("two/util.h", "int v(void);\n"),
        ]);
        assert_eq!(path(&index, "src/a/main.c", "src/x.h").len(), 2);
        assert!(path(&index, "src/a/main.c", "one/util.h").is_empty());
    }

    fn resolve(index: &mut Index, file: &str, call: u32, target: Resolution) {
        let entry = index.files.get_mut(file).unwrap();
        entry.resolved.push(ResolvedCall { call, target });
    }

    fn def_at(path: &str, line: u32) -> Resolution {
        Resolution::Def {
            path: path.into(),
            line,
        }
    }

    #[test]
    fn scip_resolutions_drop_external_calls_and_disambiguate_callers() {
        let mut index = index_of(&[
            (
                "a.rs",
                "fn new() {}\nfn run() { new(); }\nfn run2() { new(); }\n",
            ),
            (
                "b.rs",
                "fn new() {}\nfn go() { new(); }\nfn plain() { new(); }\n",
            ),
        ]);
        resolve(&mut index, "a.rs", 0, def_at("a.rs", 1));
        resolve(&mut index, "a.rs", 1, Resolution::External);
        resolve(&mut index, "b.rs", 0, def_at("b.rs", 1));
        let hits: Vec<_> = callers(&index, "new").iter().map(CallerHit::text).collect();
        assert_eq!(
            hits,
            vec![
                "a.rs:2  in fn run  -> a.rs:1",
                "b.rs:2  in fn go  -> b.rs:1",
                "b.rs:3  in fn plain",
            ]
        );
        // One definition among the hits: nothing to disambiguate, no arrow.
        let mut single = index_of(&[("a.rs", "fn new() {}\nfn run() { new(); }\n")]);
        resolve(&mut single, "a.rs", 0, def_at("a.rs", 1));
        assert_eq!(callers(&single, "new")[0].text(), "a.rs:2  in fn run");
    }

    #[test]
    fn external_calls_are_not_graph_edges() {
        let mut index = index_of(&[("a.rs", "fn b() {}\nfn run() { b(); }\nfn two() { b(); }\n")]);
        resolve(&mut index, "a.rs", 0, Resolution::External);
        let who: Vec<_> = affected(&index, "b", None)
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(who, vec!["two"]);
        assert_eq!(explain(&index, "b").incoming.len(), 1);
    }

    fn hub_rows(index: &Index) -> Vec<(String, String, usize, usize)> {
        hubs(index, 20)
            .into_iter()
            .map(|h| (h.name, h.location, h.incoming, h.outgoing))
            .collect()
    }

    fn two_news() -> Index {
        index_of(&[
            ("a.rs", "fn new() {}\nfn run_a() { new(); }\n"),
            ("b.rs", "fn new() {}\nfn run_b() { new(); }\n"),
            ("c.rs", "fn run_c() { new(); }\n"),
        ])
    }

    #[test]
    fn hubs_split_same_named_definitions_once_scip_knows_which_is_which() {
        let mut index = two_news();
        // Without SCIP data the three `new` calls are one name: legacy behaviour.
        let legacy = hub_rows(&index);
        assert_eq!(legacy[0], ("new".into(), "a.rs:1".into(), 3, 0));
        resolve(&mut index, "a.rs", 0, def_at("a.rs", 1));
        resolve(&mut index, "b.rs", 0, def_at("b.rs", 1));
        // run_c's call is unresolved and `new` has two definitions: ambiguous, left out.
        assert_eq!(
            hub_rows(&index),
            vec![
                ("new".into(), "a.rs:1".into(), 1, 0),
                ("new".into(), "b.rs:1".into(), 1, 0),
                ("run_a".into(), "a.rs:2".into(), 0, 1),
                ("run_b".into(), "b.rs:2".into(), 0, 1),
            ]
        );
    }

    #[test]
    fn a_language_without_scip_data_keeps_name_level_hubs_in_a_mixed_project() {
        let mut index = two_news();
        resolve(&mut index, "a.rs", 0, def_at("a.rs", 1));
        resolve(&mut index, "b.rs", 0, def_at("b.rs", 1));
        let py = index_of(&[("p.py", "def go():\n    new()\n")]);
        index.files.insert("p.py".into(), py.files["p.py"].clone());
        let rows = hub_rows(&index);
        // Rust has SCIP data, so run_c's unresolved call is still left out ...
        assert!(rows.iter().all(|r| r.0 != "run_c"), "{rows:?}");
        // ... but the Python caller of the same-named `new` is counted by name, as before.
        assert!(
            rows.contains(&("go".into(), "p.py:1".into(), 0, 1)),
            "{rows:?}"
        );
    }

    #[test]
    fn hubs_keep_counting_unresolved_calls_to_a_name_with_one_definition() {
        let mut index = index_of(&[(
            "a.rs",
            "fn helper() {}\nfn go() { helper(); }\nfn other() { x(); }\nfn x() {}\n",
        )]);
        resolve(&mut index, "a.rs", 1, def_at("a.rs", 4));
        let rows = hub_rows(&index);
        assert!(
            rows.contains(&("helper".into(), "a.rs:1".into(), 1, 0)),
            "{rows:?}"
        );
    }

    #[test]
    fn ambiguous_python_module_names_make_no_edge() {
        let index = index_of(&[
            ("main.py", "import util\n"),
            ("a/util.py", "def u(): pass\n"),
            ("b/util.py", "def u(): pass\n"),
        ]);
        assert!(path(&index, "main.py", "a/util.py").is_empty());
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
