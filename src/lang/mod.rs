//! Language support: one file per language, chosen by file extension.

pub mod generic;
pub mod java;
pub mod python;
pub mod rust;
pub mod typescript;

use anyhow::{Result, anyhow};
use tree_sitter::{Language, Node, Parser};

use crate::model::{Call, Symbol};

/// What one file contributes to the graph, before hashing and line counting.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    pub calls: Vec<Call>,
}

/// Supported languages; adding one means a new variant, a grammar and a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    /// Parsed with the TSX grammar: JavaScript (and JSX) is a subset of it.
    JavaScript,
    Python,
    Java,
}

impl Lang {
    pub fn from_path(path: &str) -> Option<Lang> {
        if path.ends_with(".d.ts") {
            return None;
        }
        match path.rsplit('.').next()? {
            "rs" => Some(Lang::Rust),
            "ts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "py" => Some(Lang::Python),
            "java" => Some(Lang::Java),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
            _ => None,
        }
    }

    fn grammar(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::Tsx | Lang::JavaScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

/// Parse `src` and pull out its symbols, imports and calls.
pub fn extract(lang: Lang, src: &str) -> Result<Facts> {
    let mut parser = Parser::new();
    parser.set_language(&lang.grammar())?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| anyhow!("parser gave no tree"))?;
    let mut facts = match lang {
        Lang::Rust => rust::extract(tree.root_node(), src),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            typescript::extract(tree.root_node(), src)
        }
        Lang::Python => python::extract(tree.root_node(), src),
        Lang::Java => java::extract(tree.root_node(), src),
    };
    facts.imports.sort();
    facts.imports.dedup();
    Ok(facts)
}

/// Source text of a node ("" if the byte range is somehow invalid).
pub(crate) fn text<'a>(node: Node, src: &'a str) -> &'a str {
    src.get(node.byte_range()).unwrap_or("")
}

/// Text of a named child field, e.g. `name` of a function.
pub(crate) fn field_text<'a>(node: Node, field: &str, src: &'a str) -> Option<&'a str> {
    node.child_by_field_name(field).map(|n| text(n, src))
}

/// One-line declaration text from `start` up to `body` (or the first line when there is no body).
pub(crate) fn signature(start: Node, body: Option<Node>, src: &str) -> String {
    let end = body.map_or(start.end_byte(), |b| b.start_byte());
    let raw = src.get(start.start_byte()..end).unwrap_or("");
    let raw = if body.is_some() {
        raw
    } else {
        raw.lines().next().unwrap_or("")
    };
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = joined.trim_end_matches(['{', ' ']);
    trimmed.chars().take(110).collect()
}

/// Name shown after "in" for callers: the signature up to its first `(` or `<`
/// (so generics like `fn f<T: Fn(u8)>` don't leak into the name). The parentheses of a
/// restricted visibility such as `pub(crate)` are part of the name, not its end.
pub(crate) fn caller_name(signature: &str) -> String {
    let skip = signature
        .strip_prefix("pub(")
        .and_then(|rest| rest.find(')'))
        .map_or(0, |close| "pub(".len() + close + 1);
    let end = signature[skip..]
        .find(['(', '<'])
        .map_or(signature.len(), |i| skip + i);
    signature[..end]
        .trim_end_matches([' ', '='])
        .trim()
        .to_string()
}

/// 1-based line number, as editors and `rg -n` show it.
pub(crate) fn line_of(node: Node) -> u32 {
    u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX)
}
