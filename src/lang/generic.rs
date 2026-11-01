//! One syntax-tree walker for the languages that need only a table of node kinds.
//! Ported idea: Graphify's `LanguageConfig` in `extract.py` (Apache-2.0 / MIT, see NOTICE).

use tree_sitter::Node;

use super::{Facts, field_text, line_of, signature, text};
use crate::model::{Call, Form, Kind, Symbol};

/// What a language contributes: which node kinds are declarations, calls and imports.
pub struct Config {
    /// Declaration node kinds. A `Kind::Fn` one also becomes the caller of the calls inside it.
    pub items: &'static [(&'static str, Kind)],
    /// Node kinds that call something.
    pub calls: &'static [&'static str],
    /// Node kinds that name the caller of the calls inside them but are not listed as symbols
    /// (a C# property: its accessors' calls belong to it).
    pub scopes: &'static [&'static str],
    /// Node kinds whose name is the container of the members declared inside (class, struct,
    /// interface...): those members get it as `container`, and calls in them as `owner`.
    pub containers: &'static [&'static str],
    /// How a call node is written (`f()`, `self.f()`, `Foo.f()`); `no_form` when not read yet.
    pub form: fn(Node, &str) -> Form,
    /// Names a declaration introduces (most grammars: just its `name` field; `typedef A, *B;` gives two).
    pub name: fn(Node, &str) -> Vec<String>,
    /// Name of what a call node calls.
    pub callee: fn(Node, &str) -> Option<String>,
    /// Called on every node; pushes the module paths an import node names.
    pub imports: fn(Node, &str, &mut Vec<String>),
}

pub fn extract(root: Node, src: &str, config: &Config) -> Facts {
    let mut facts = Facts::default();
    visit(root, src, config, &mut facts, Ctx::default());
    facts
}

/// Where the walk is: the innermost function (`enclosing`), the class-like item whose members we
/// are reading (`container`: none inside a method's body) and the class we are inside at any
/// depth (`owner`: what `self` means, also in a nested function).
#[derive(Clone, Copy, Default)]
struct Ctx<'a> {
    enclosing: Option<&'a str>,
    container: Option<&'a str>,
    owner: Option<&'a str>,
}

/// Depth-first walk.
fn visit(node: Node, src: &str, config: &Config, facts: &mut Facts, ctx: Ctx) {
    let (container, owner) = (ctx.container, ctx.owner);
    let mut own = None;
    let mut inner_container = container.map(String::from);
    let mut inner_owner = owner.map(String::from);
    let kind = node.kind();
    if let Some(&(_, item)) = config.items.iter().find(|(k, _)| *k == kind) {
        let names = add_item(node, item, src, config, facts, container);
        if config.containers.contains(&kind) {
            inner_container = names.first().cloned();
            inner_owner = inner_container.clone();
        } else if item == Kind::Fn {
            inner_container = None; // a function nested in a method is not a member
        }
        own = names.into_iter().next().filter(|_| item == Kind::Fn);
    } else if config.scopes.contains(&kind) {
        own = (config.name)(node, src).into_iter().next();
    }
    if config.calls.contains(&kind)
        && let Some(callee) = (config.callee)(node, src)
    {
        facts.calls.push(Call {
            callee,
            line: line_of(node),
            caller: ctx.enclosing.map(String::from),
            form: (config.form)(node, src),
            owner: owner.map(String::from),
        });
    }
    (config.imports)(node, src, &mut facts.imports);
    let inner = Ctx {
        enclosing: own.as_deref().or(ctx.enclosing),
        container: inner_container.as_deref(),
        owner: inner_owner.as_deref(),
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, config, facts, inner);
    }
}

/// Record a declaration; returns the names it introduces.
fn add_item(
    node: Node,
    kind: Kind,
    src: &str,
    config: &Config,
    facts: &mut Facts,
    container: Option<&str>,
) -> Vec<String> {
    let names = (config.name)(node, src);
    let sig = signature(node, node.child_by_field_name("body"), src);
    for name in &names {
        facts.symbols.push(Symbol {
            name: name.clone(),
            kind,
            line: line_of(node),
            signature: sig.trim_end_matches([':', ';', ' ']).to_string(),
            container: container.map(String::from),
        });
    }
    names
}

/// For languages whose call forms are not read yet.
pub fn no_form(_: Node, _: &str) -> Form {
    Form::Unknown
}

/// The `name` field of a node: right for most grammars.
pub fn name_field(node: Node, src: &str) -> Vec<String> {
    field_text(node, "name", src)
        .map(String::from)
        .into_iter()
        .collect()
}

/// Text of a node, or None when it is empty.
pub fn non_empty(node: Node, src: &str) -> Option<String> {
    Some(text(node, src))
        .filter(|t| !t.is_empty())
        .map(String::from)
}
