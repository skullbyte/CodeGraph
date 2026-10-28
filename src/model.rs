//! Plain data types shared by every module; `Serialize`/`Deserialize` make them JSON-ready.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    pub line: u32,
    /// Trimmed declaration line, at most 110 chars, without a trailing `{`.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub callee: String,
    pub line: u32,
    /// Signature of the enclosing function; `None` means top level.
    pub caller: Option<String>,
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
    pub fn graph_calls(&self) -> impl Iterator<Item = (&Call, Option<&Resolution>)> {
        self.calls.iter().enumerate().filter_map(|(i, call)| {
            let target = self.resolution(i);
            (target != Some(&Resolution::External)).then_some((call, target))
        })
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
