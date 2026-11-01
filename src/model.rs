//! Plain data types shared by every module; `Serialize`/`Deserialize` make them JSON-ready.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::lang::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Fn,
    Struct,
    Enum,
    Trait,
    Impl,
    Mod,
    Type,
    Class,
    Interface,
    Const,
    /// A markdown heading (a doc, not code; appended last so older graph.bin files still decode).
    Heading,
}

impl Kind {
    /// A definition in code: not an `impl` block (only a container) and not a doc heading.
    pub fn is_code_def(self) -> bool {
        !matches!(self, Kind::Impl | Kind::Heading)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    pub line: u32,
    /// Trimmed declaration line, at most 110 chars, without a trailing `{`.
    pub signature: String,
    /// Name of the type, trait or class this is declared in (`None` for free functions and top
    /// level items). Tells methods from functions when several definitions share a name.
    #[serde(default)]
    pub container: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub callee: String,
    pub line: u32,
    /// Signature of the enclosing function; `None` means top level.
    pub caller: Option<String>,
    /// How the call is written, which says what kind of definition it can reach.
    #[serde(default)]
    pub form: Form,
    /// The type, trait or class the enclosing function is declared in.
    #[serde(default)]
    pub owner: Option<String>,
}

/// How a call is written. Without types, this is all the code says about where it goes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Form {
    /// Not recorded, or a shape we do not read (macros, token scans): any definition may match.
    #[default]
    Unknown,
    /// `f()`: a free function, or a method of the enclosing type in languages with an implicit `this`.
    Bare,
    /// `self.f()`, `this.f()`, `Self::f()`, `super.f()`: a method of the enclosing type.
    SelfRef,
    /// `Foo::f()`, `Foo.f()`: a member of the named type (or a free function of a module `Foo`).
    Type(String),
    /// `module::f()`, `pkg.f()`: a free function reached through a lowercase path (its last segment).
    Module(String),
    /// `value.f()`: a method of a type we cannot see.
    Method,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub lines: u32,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    pub calls: Vec<Call>,
    /// What a compiler-grade index (SCIP) says some of `calls` refer to; sorted by call position.
    /// Empty until `codegraph scip` runs; a re-parsed file starts empty again.
    #[serde(default)]
    pub resolved: Vec<ResolvedCall>,
}

/// Where the call at position `call` of `FileEntry::calls` really goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCall {
    pub call: u32,
    pub target: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    /// Defined in the project, at this file and 1-based line.
    Def { path: String, line: u32 },
    /// Defined outside the project (standard library, a dependency): not an edge of this graph.
    External,
}

impl FileEntry {
    /// The calls that belong in the graph, each with SCIP's answer if there is one. Calls the
    /// compiler put outside the project (standard library, dependencies) are left out.
    /// A doc's mentions are not calls: the code graph (hubs, paths, communities) never sees them.
    pub fn graph_calls(&self) -> impl Iterator<Item = (&Call, Option<&Resolution>)> {
        let doc = self.is_doc();
        self.calls.iter().enumerate().filter_map(move |(i, call)| {
            let target = self.resolution(i);
            (!doc && target != Some(&Resolution::External)).then_some((call, target))
        })
    }

    /// A markdown file: headings and mentions, no code.
    pub fn is_doc(&self) -> bool {
        Lang::from_path(&self.path) == Some(Lang::Markdown)
    }

    /// SCIP's answer for the call at position `call`, if `codegraph scip` gave one.
    pub fn resolution(&self, call: usize) -> Option<&Resolution> {
        let call = u32::try_from(call).ok()?;
        let at = self.resolved.binary_search_by_key(&call, |r| r.call).ok()?;
        Some(&self.resolved[at].target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_lookup_by_call_position() {
        let entry = FileEntry {
            resolved: vec![
                ResolvedCall {
                    call: 1,
                    target: Resolution::External,
                },
                ResolvedCall {
                    call: 4,
                    target: Resolution::Def {
                        path: "a.rs".into(),
                        line: 7,
                    },
                },
            ],
            ..FileEntry::default()
        };
        assert_eq!(entry.resolution(0), None);
        assert_eq!(entry.resolution(1), Some(&Resolution::External));
        assert_eq!(
            entry.resolution(4),
            Some(&Resolution::Def {
                path: "a.rs".into(),
                line: 7
            })
        );
    }

    #[test]
    fn code_totals_leave_out_docs() {
        let doc = crate::lang::extract(Lang::Markdown, "# T\n`run()`\n").unwrap();
        let code = crate::lang::extract(Lang::Rust, "fn run() { run(); }\n").unwrap();
        let mut index = Index::default();
        for (path, facts) in [("a.rs", &code), ("R.md", &doc)] {
            index.files.insert(
                path.into(),
                FileEntry {
                    path: path.into(),
                    symbols: facts.symbols.clone(),
                    calls: facts.calls.clone(),
                    ..FileEntry::default()
                },
            );
        }
        assert_eq!(index.code_totals(), (1, 1));
    }

    #[test]
    fn graphs_without_resolutions_still_load() {
        let old = r#"{"path":"a.rs","hash":"h","lines":1,"symbols":[],"imports":[],"calls":[]}"#;
        let entry: FileEntry = serde_json::from_str(old).unwrap();
        assert!(entry.resolved.is_empty());
        let bytes = postcard::to_stdvec(&entry).unwrap();
        assert_eq!(postcard::from_bytes::<FileEntry>(&bytes).unwrap(), entry);
    }
}

/// The whole graph; saved as `graph.json` and reused as the incremental cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    /// Identifies the binary that built this graph; a rebuilt binary may extract differently.
    pub extractor: String,
    /// TypeScript import aliases from tsconfig.json: prefix -> folder, e.g. `"@/"` -> `"src/"`.
    pub aliases: BTreeMap<String, String>,
    pub files: BTreeMap<String, FileEntry>,
}

impl Index {
    /// `(symbols, calls)` of the code only: doc headings and mentions are not counted.
    pub fn code_totals(&self) -> (usize, usize) {
        let mut totals = (0, 0);
        for file in self.files.values() {
            totals.0 += file
                .symbols
                .iter()
                .filter(|s| s.kind != Kind::Heading)
                .count();
            if !file.is_doc() {
                totals.1 += file.calls.len();
            }
        }
        totals
    }
}
