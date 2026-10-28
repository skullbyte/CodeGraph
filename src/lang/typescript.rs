//! TypeScript/TSX extraction: walks the syntax tree and records declarations, imports and calls.

use tree_sitter::Node;

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Kind, Symbol};

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = Facts::default();
    visit(root, src, &mut facts, None);
    facts
}

/// Depth-first walk; `enclosing` is the name of the innermost function we are inside.
fn visit(node: Node, src: &str, facts: &mut Facts, enclosing: Option<&str>) {
    let mut own = None;
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            own = add_item(node, Kind::Fn, src, facts).map(|s| caller_name(&s));
        }
        "method_definition" => own = add_item(node, Kind::Fn, src, facts).map(|s| caller_name(&s)),
        "class_declaration" | "abstract_class_declaration" => {
            add_item(node, Kind::Class, src, facts);
        }
        "interface_declaration" => _ = add_item(node, Kind::Interface, src, facts),
        "type_alias_declaration" => _ = add_item(node, Kind::Type, src, facts),
        "enum_declaration" => _ = add_item(node, Kind::Enum, src, facts),
        "variable_declarator" => own = add_variable(node, src, facts),
        "import_statement" | "export_statement" => add_import(node, src, facts),
        "call_expression" => add_call(node, src, facts, enclosing),
        _ => {}
    }
    let enclosing = own.as_deref().or(enclosing);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, facts, enclosing);
    }
}

/// The node whose text starts the declaration: the `export ...` wrapper when there is one.
fn decl_start(node: Node) -> Node {
    match node.parent() {
        Some(p) if p.kind() == "export_statement" => p,
        _ => node,
    }
}

/// Record a named declaration; returns its signature.
fn add_item(node: Node, kind: Kind, src: &str, facts: &mut Facts) -> Option<String> {
    let name = field_text(node, "name", src)?;
    let start = decl_start(node);
    let sig = signature(start, node.child_by_field_name("body"), src);
    facts.symbols.push(Symbol {
        name: name.to_string(),
        kind,
        line: line_of(start),
        signature: sig.clone(),
    });
    Some(sig)
}

/// `const f = () => {}` is a function (even when not exported, so calls inside get a caller);
/// other consts are listed only when exported. Returns the caller name for function values.
fn add_variable(node: Node, src: &str, facts: &mut Facts) -> Option<String> {
    let name = node
        .child_by_field_name("name")
        .filter(|n| n.kind() == "identifier")?;
    let decl = node.parent()?;
    let start = decl_start(decl);
    let exported = start.kind() == "export_statement";
    let value = node.child_by_field_name("value");
    let function = value.filter(|v| matches!(v.kind(), "arrow_function" | "function_expression"));
    let sig = signature(
        start,
        function.and_then(|f| f.child_by_field_name("body")),
        src,
    );
    if exported {
        let kind = if function.is_some() {
            Kind::Fn
        } else {
            Kind::Const
        };
        facts.symbols.push(Symbol {
            name: text(name, src).to_string(),
            kind,
            line: line_of(start),
            signature: sig.clone(),
        });
    }
    function.map(|_| caller_name(&sig))
}

/// Keep every import specifier as written; whether it is a local file, an alias or a package
/// is decided later, when the graph knows the project's tsconfig aliases.
fn add_import(node: Node, src: &str, facts: &mut Facts) {
    let Some(path) = field_text(node, "source", src) else {
        return;
    };
    let path = path.trim_matches(['\'', '"', '`']);
    if !path.is_empty() {
        facts.imports.push(path.to_string());
    }
}

/// `f()` -> f, `a.f()` -> f; anything fancier (`(x)()`, `import()`) is skipped.
fn add_call(node: Node, src: &str, facts: &mut Facts, enclosing: Option<&str>) {
    let Some(f) = node.child_by_field_name("function") else {
        return;
    };
    let callee = match f.kind() {
        "identifier" => text(f, src),
        "member_expression" => field_text(f, "property", src).unwrap_or(""),
        _ => "",
    };
    if callee == "require" && f.kind() == "identifier" {
        add_require(node, src, facts);
    }
    if !callee.is_empty() {
        facts.calls.push(Call {
            callee: callee.to_string(),
            line: line_of(node),
            caller: enclosing.map(String::from),
        });
    }
}

/// CommonJS `require('./x')`: only a plain string argument counts.
fn add_require(node: Node, src: &str, facts: &mut Facts) {
    let Some(arg) = node
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0))
        .filter(|a| a.kind() == "string")
    else {
        return;
    };
    let path = text(arg, src).trim_matches(['\'', '"']);
    if !path.is_empty() {
        facts.imports.push(path.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "import { formatPrice } from './price';
import React from 'react';
import { x } from '@/lib/x';
export * from '../shared';
export function show(n: number): string {
  return formatPrice(n);
}
export const Cart = (items: Item[]) => {
  return items.map(show);
};
export const TAX = 0.2;
class Store {
  add(x: number) {
    this.items.push(x);
  }
}
export interface Item { id: number }
export type Id = string;
export enum Mode { A }
function local() { show(1); }
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        let want = vec![
            ("show", Kind::Fn, 5),
            ("Cart", Kind::Fn, 8),
            ("TAX", Kind::Const, 11),
            ("Store", Kind::Class, 12),
            ("add", Kind::Fn, 13),
            ("Item", Kind::Interface, 17),
            ("Id", Kind::Type, 18),
            ("Mode", Kind::Enum, 19),
            ("local", Kind::Fn, 20),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn signatures_keep_export() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        assert_eq!(
            facts.symbols[0].signature,
            "export function show(n: number): string"
        );
        assert_eq!(
            facts.symbols[1].signature,
            "export const Cart = (items: Item[]) =>"
        );
    }

    #[test]
    fn imports_keep_every_specifier() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        assert_eq!(
            facts.imports,
            vec!["../shared", "./price", "@/lib/x", "react"]
        );
    }

    #[test]
    fn calls_know_their_enclosing_fn() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
            .collect();
        let want = vec![
            ("formatPrice", 6, Some("export function show")),
            ("map", 9, Some("export const Cart")),
            ("push", 14, Some("add")),
            ("show", 20, Some("function local")),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn require_with_a_string_is_an_import() {
        let facts = extract_file(
            Lang::JavaScript,
            "const a = require('./a');\nconst b = require(name);\n",
        )
        .unwrap();
        assert_eq!(facts.imports, vec!["./a"]);
    }

    #[test]
    fn tsx_parses_jsx() {
        let facts = extract_file(
            Lang::Tsx,
            "export function App() { return <div>{fmt(1)}</div>; }\n",
        )
        .unwrap();
        assert_eq!(facts.symbols[0].name, "App");
        assert_eq!(facts.calls[0].callee, "fmt");
    }
}
