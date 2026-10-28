//! One syntax-tree walker for the languages that need only a table of node kinds.
//! Ported idea: Graphify's `LanguageConfig` in `extract.py` (Apache-2.0 / MIT, see NOTICE).

use tree_sitter::Node;

use super::{Facts, field_text, line_of, signature, text};
use crate::model::{Call, Kind, Symbol};

/// What a language contributes: which node kinds are declarations, calls and imports.
pub struct Config {
    /// Declaration node kinds. A `Kind::Fn` one also becomes the caller of the calls inside it.
    pub items: &'static [(&'static str, Kind)],
    /// Node kinds that call something.
    pub calls: &'static [&'static str],
    /// Name of a declaration (default for most grammars: its `name` field).
    pub name: fn(Node, &str) -> Option<String>,
    /// Name of what a call node calls.
    pub callee: fn(Node, &str) -> Option<String>,
    /// Called on every node; pushes the module paths an import node names.
    pub imports: fn(Node, &str, &mut Vec<String>),
}

pub fn extract(root: Node, src: &str, config: &Config) -> Facts {
    let mut facts = Facts::default();
    visit(root, src, config, &mut facts, None);
    facts
}

/// Depth-first walk; `enclosing` is the name of the innermost function we are inside.
fn visit(node: Node, src: &str, config: &Config, facts: &mut Facts, enclosing: Option<&str>) {
    let mut own = None;
    let kind = node.kind();
    if let Some(&(_, item)) = config.items.iter().find(|(k, _)| *k == kind) {
        own = add_item(node, item, src, config, facts);
    }
    if config.calls.contains(&kind)
        && let Some(callee) = (config.callee)(node, src)
    {
        facts.calls.push(Call {
            callee,
            line: line_of(node),
            caller: enclosing.map(String::from),
        });
    }
    (config.imports)(node, src, &mut facts.imports);
    let enclosing = own.as_deref().or(enclosing);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, config, facts, enclosing);
    }
}

/// Record a declaration; returns its name when it is a function (the caller of what is inside).
fn add_item(
    node: Node,
    kind: Kind,
    src: &str,
    config: &Config,
    facts: &mut Facts,
) -> Option<String> {
    let name = (config.name)(node, src)?;
    let sig = signature(node, node.child_by_field_name("body"), src);
    facts.symbols.push(Symbol {
        name: name.clone(),
        kind,
        line: line_of(node),
        signature: sig.trim_end_matches([':', ';', ' ']).to_string(),
    });
    (kind == Kind::Fn).then_some(name)
}

/// The `name` field of a node: right for most grammars.
pub fn name_field(node: Node, src: &str) -> Option<String> {
    field_text(node, "name", src).map(String::from)
}

/// Text of a node, or None when it is empty.
pub fn non_empty(node: Node, src: &str) -> Option<String> {
    Some(text(node, src))
        .filter(|t| !t.is_empty())
        .map(String::from)
}
