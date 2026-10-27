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
