//! Answers def, callers and path questions over an `Index`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::lang::Lang;
use crate::model::{FileEntry, Index, Kind, Resolution};
use crate::scope::{self, Resolver};
use crate::tour::is_side_path;

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
    let mut hits: Vec<CallerHit> = call_sites(index, name)
        .into_iter()
        .map(|(hit, _)| hit)
        .collect();
    hits.extend(doc_mentions(index, name));
    hits
}

/// A call site, and the indexes (into the name's definitions) it can reach; empty = not narrowed.
type Site = (CallerHit, Vec<usize>);

/// Every call site of `name` with what it can reach. With several definitions each hit says
/// which one it calls when that is certain (SCIP's answer, else the form resolver's).
fn call_sites(index: &Index, name: &str) -> Vec<Site> {
    let defs = scope::definitions_of(index, name);
    // The resolver reads every import: build it only when there is something to tell apart.
    let resolver = (defs.len() >= 2).then(|| Resolver::new(index));
    let mut sites: Vec<Site> = index
        .files
        .values()
        .flat_map(|f| {
            let (defs, resolver) = (&defs, &resolver);
            f.graph_calls()
                .filter(move |(c, _)| c.callee == name)
                .map(move |(c, scip)| {
                    let reach = resolver
                        .as_ref()
                        .map(|r| r.candidates(&f.path, c, scip, defs))
                        .unwrap_or_default();
                    // Only a definition that exists now is shown: an old SCIP run may point at a
                    // line that has since moved.
                    let target = match &reach[..] {
                        [one] => Some(format!("{}:{}", defs[*one].path, defs[*one].line)),
                        _ => None,
                    };
                    let hit = CallerHit {
                        path: f.path.clone(),
                        line: c.line,
                        caller: c.caller.clone(),
                        target,
                        doc: false,
                    };
                    (hit, reach)
                })
        })
        .collect();
    // With one definition there is nothing to tell apart; with several, say which each hit calls.
    let distinct: BTreeSet<_> = sites.iter().filter_map(|(h, _)| h.target.clone()).collect();
    if distinct.len() < 2 {
        sites.iter_mut().for_each(|(h, _)| h.target = None);
    }
    sites
}

/// The definition of `name` that `selector` (`PATH` or `PATH:LINE`) names, or an error that lists
/// the choices.
pub fn select_def(index: &Index, name: &str, selector: &str) -> Result<scope::Def, String> {
    let defs = scope::definitions_of(index, name);
    let selector = selector.strip_prefix("./").unwrap_or(selector);
    let (path, line) = match selector.rsplit_once(':') {
        Some((p, l)) if l.parse::<u32>().is_ok() => (p, l.parse::<u32>().ok()),
        _ => (selector, None),
    };
    let mut found = defs
        .iter()
        .filter(|d| d.path == path && line.is_none_or(|l| d.line == l));
    match (found.next(), found.next()) {
        (Some(d), None) => Ok(d.clone()),
        _ => {
            let choices: Vec<_> = defs
                .iter()
                .map(|d| format!("{}:{}", d.path, d.line))
                .collect();
            Err(format!(
                "--def {selector} does not pick one definition of {name}; use one of: {}",
                if choices.is_empty() {
                    "(none)".into()
                } else {
                    choices.join(", ")
                }
            ))
        }
    }
}

/// Call sites that can reach `only`: those pinned to it and those that could be any definition.
/// Sites pinned to another definition are left out.
pub fn callers_of(index: &Index, name: &str, only: &scope::Def) -> Vec<CallerHit> {
    let defs = scope::definitions_of(index, name);
    let Some(at) = defs.iter().position(|d| d == only) else {
        return Vec::new();
    };
    call_sites(index, name)
        .into_iter()
        .filter(|(_, reach)| reach.is_empty() || reach.contains(&at))
        .map(|(hit, _)| hit)
        .collect()
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
pub(crate) fn resolve_ts_import<V>(
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
    // The longest matching prefix wins, so `@components/` beats `@/`; an alias to a file (a
    // package entry such as `zod/v4`) matches only that exact name.
    let to_folder = |f: &str| f.is_empty() || f.ends_with('/') || f.contains('|');
    let (prefix, folder) = aliases
        .iter()
        .filter(|(p, f)| imp.starts_with(p.as_str()) && (to_folder(f) || imp == p.as_str()))
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

impl Link {
    /// Like `text`, without the site count when it is one.
    fn brief_text(&self) -> String {
        let mut out = match &self.location {
            Some(loc) => format!("{}  {loc}", self.name),
            None => self.name.clone(),
        };
        if self.sites > 1 {
            out.push_str(&format!(" ({} sites)", self.sites));
        }
        out
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

/// How much of an `explain` answer to print.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// A few definitions and neighbours, tests last: the default, cheap in tokens.
    #[default]
    Brief,
    /// Every definition and up to `LIST_LIMIT` neighbours per direction.
    Full,
    Callers,
    Callees,
    Defs,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(text: &str) -> Result<Mode, String> {
        match text {
            "brief" => Ok(Mode::Brief),
            "full" => Ok(Mode::Full),
            "callers" => Ok(Mode::Callers),
            "callees" => Ok(Mode::Callees),
            "defs" => Ok(Mode::Defs),
            other => Err(format!(
                "unknown mode `{other}`: use brief, full, callers, callees or defs"
            )),
        }
    }
}

/// What brief mode shows: definitions, and neighbours per direction.
const BRIEF_LIMIT: usize = 5;
/// Longest signature brief mode prints.
const BRIEF_SIGNATURE: usize = 80;

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

    /// The answer in `mode`, with `under(def)` right below each printed `def` line.
    pub fn lines_in(&self, mode: Mode, under: impl Fn(&DefHit) -> Vec<String>) -> Vec<String> {
        match mode {
            Mode::Full => self.lines_with(under),
            Mode::Brief => self.brief(under),
            Mode::Defs => self.defs_lines(&self.defs, under),
            Mode::Callers => Self::link_lines("<-", &self.incoming),
            Mode::Callees => Self::link_lines("->", &self.outgoing),
        }
    }

    fn defs_lines(&self, defs: &[DefHit], under: impl Fn(&DefHit) -> Vec<String>) -> Vec<String> {
        let mut out = Vec::new();
        for d in defs {
            out.push(format!("def {}", d.text()));
            out.extend(under(d));
        }
        out
    }

    fn link_lines(arrow: &str, links: &[Link]) -> Vec<String> {
        if links.is_empty() {
            return vec![format!("{arrow} (none)")]; // so "nobody" is not mistaken for an error
        }
        let mut out: Vec<String> = links
            .iter()
            .take(LIST_LIMIT)
            .map(|l| format!("{arrow} {}", l.text()))
            .collect();
        if links.len() > LIST_LIMIT {
            out.push(format!("{arrow} ... {} more", links.len() - LIST_LIMIT));
        }
        out
    }

    fn brief(&self, under: impl Fn(&DefHit) -> Vec<String>) -> Vec<String> {
        let mut out = Vec::new();
        for d in self.defs.iter().take(BRIEF_LIMIT) {
            let signature = cut(&d.signature, BRIEF_SIGNATURE);
            out.push(format!("def {}:{} {signature}", d.path, d.line));
            out.extend(under(d));
        }
        if self.defs.len() > BRIEF_LIMIT {
            out.push(format!(
                "def ... +{} more (mode=defs)",
                self.defs.len() - BRIEF_LIMIT
            ));
        }
        for (arrow, links, more) in [
            ("<-", &self.incoming, "callers"),
            ("->", &self.outgoing, "callees"),
        ] {
            let mut ordered: Vec<&Link> = links.iter().collect();
            ordered.sort_by_key(|l| l.location.as_deref().is_some_and(is_side_path));
            out.extend(
                ordered
                    .iter()
                    .take(BRIEF_LIMIT)
                    .map(|l| format!("{arrow} {}", l.brief_text())),
            );
            if links.len() > BRIEF_LIMIT {
                out.push(format!(
                    "{arrow} ... +{} more (mode={more})",
                    links.len() - BRIEF_LIMIT
                ));
            }
        }
        out
    }
}

/// `text` cut to `max` characters, with `...` when something was dropped.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_string(),
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

/// `explain` for one definition: its own callers (see `callers_of`) and the calls made by its
/// body, found by file and owner type.
pub fn explain_of(index: &Index, name: &str, only: &scope::Def) -> Explain {
    let locs = locations(index);
    let mut incoming: BTreeMap<String, usize> = BTreeMap::new();
    for hit in callers_of(index, name, only) {
        if let Some(caller) = hit.caller.as_deref().map(bare_name) {
            *incoming.entry(caller).or_default() += 1;
        }
    }
    let mut outgoing: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(file) = index.files.get(&only.path) {
        for (call, _) in file.graph_calls() {
            let mine = call.caller.as_deref().map(bare_name).as_deref() == Some(name)
                && call.owner == only.container;
            if mine {
                *outgoing.entry(call.callee.clone()).or_default() += 1;
            }
        }
    }
    let defs = def(index, name)
        .into_iter()
        .filter(|d| d.path == only.path && d.line == only.line)
        .collect();
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
mod tests;
