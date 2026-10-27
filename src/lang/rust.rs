//! Rust extraction: walks the syntax tree and records items, `use`/`mod` imports and calls.

use tree_sitter::{Node, Parser};

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Kind, Symbol};

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = Facts::default();
    visit(root, src, &mut facts, None);
    facts
}

/// Depth-first walk; `enclosing` is the name of the innermost `fn` we are inside.
fn visit(node: Node, src: &str, facts: &mut Facts, enclosing: Option<&str>) {
    let mut own = None;
    match node.kind() {
        "function_item" => own = add_item(node, Kind::Fn, src, facts).map(|s| caller_name(&s)),
        "struct_item" => _ = add_item(node, Kind::Struct, src, facts),
        "enum_item" => _ = add_item(node, Kind::Enum, src, facts),
        "trait_item" => _ = add_item(node, Kind::Trait, src, facts),
        "type_item" => _ = add_item(node, Kind::Type, src, facts),
        "mod_item" => {
            add_item(node, Kind::Mod, src, facts);
            if node.child_by_field_name("body").is_none() {
                facts
                    .imports
                    .extend(field_text(node, "name", src).map(String::from));
            }
        }
        "impl_item" => add_impl(node, src, facts),
        "const_item" if is_pub(node) => _ = add_item(node, Kind::Const, src, facts),
        "use_declaration" => {
            facts.imports.extend(
                node.child_by_field_name("argument")
                    .and_then(|a| use_root(text(a, src))),
            );
        }
        "call_expression" => {
            let callee = node
                .child_by_field_name("function")
                .and_then(|f| callee_name(f, src));
            add_call(node, callee, facts, enclosing);
        }
        "macro_invocation" => {
            let callee = node
                .child_by_field_name("macro")
                .and_then(|f| callee_name(f, src));
            add_call(node, callee, facts, enclosing);
            let before = (facts.symbols.len(), facts.calls.len());
            if callee.is_some_and(|name| name.starts_with("cfg_")) {
                visit_macro_items(node, src, facts, enclosing);
            }
            // Not items (e.g. `cfg_if!`) or not a cfg macro: scan the raw tokens instead.
            if before == (facts.symbols.len(), facts.calls.len())
                && let Some(args) = macro_args(node)
            {
                visit_token_calls(args, src, facts, enclosing);
            }
        }
        _ => {}
    }
    let enclosing = own.as_deref().or(enclosing);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, facts, enclosing);
    }
}

/// Macros like tokio's `cfg_net! { ... }` wrap whole items, which tree-sitter only sees as
/// raw tokens. Re-parse the macro body as ordinary Rust and shift its line numbers back.
fn visit_macro_items(node: Node, src: &str, facts: &mut Facts, enclosing: Option<&str>) {
    let mut cursor = node.walk();
    let Some(body) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "token_tree")
    else {
        return;
    };
    // Skip the opening and closing delimiter.
    let Some(inner) = src.get(body.start_byte() + 1..body.end_byte().saturating_sub(1)) else {
        return;
    };
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return;
    }
    let Some(tree) = parser.parse(inner, None) else {
        return;
    };
    let (symbols, calls) = (facts.symbols.len(), facts.calls.len());
    visit(tree.root_node(), inner, facts, enclosing);
    let shift = u32::try_from(body.start_position().row).unwrap_or(0);
    facts.symbols[symbols..]
        .iter_mut()
        .for_each(|s| s.line += shift);
    facts.calls[calls..]
        .iter_mut()
        .for_each(|c| c.line += shift);
}

/// The `( ... )` / `[ ... ]` / `{ ... }` part of a macro call, as raw tokens.
fn macro_args(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| c.kind() == "token_tree")
}

/// Words that can be followed by `(` without being a call.
const NOT_CALLS: [&str; 15] = [
    "fn", "if", "while", "match", "for", "return", "loop", "move", "async", "unsafe", "in", "else",
    "dyn", "ref", "yield",
];

/// Macro arguments are raw tokens, not a syntax tree, so look for the shapes of a call:
/// `name (...)` is a function or method call, `name ! (...)` a nested macro call.
fn visit_token_calls(tokens: Node, src: &str, facts: &mut Facts, enclosing: Option<&str>) {
    let mut cursor = tokens.walk();
    let kids: Vec<Node> = tokens.children(&mut cursor).collect();
    for (i, kid) in kids.iter().enumerate() {
        if kid.kind() == "token_tree" {
            visit_token_calls(*kid, src, facts, enclosing);
            continue;
        }
        let name = text(*kid, src);
        let after_fn = i > 0 && text(kids[i - 1], src) == "fn";
        if kid.kind() != "identifier" || after_fn || NOT_CALLS.contains(&name) {
            continue;
        }
        let paren_next = kids
            .get(i + 1)
            .is_some_and(|n| n.kind() == "token_tree" && text(*n, src).starts_with('('));
        let macro_next = kids.get(i + 1).is_some_and(|n| n.kind() == "!")
            && kids.get(i + 2).is_some_and(|n| n.kind() == "token_tree");
        if paren_next || macro_next {
            add_call(*kid, Some(name), facts, enclosing);
        }
    }
}

/// Record a named item; returns its signature.
fn add_item(node: Node, kind: Kind, src: &str, facts: &mut Facts) -> Option<String> {
    let name = field_text(node, "name", src)?;
    let sig = signature(node, node.child_by_field_name("body"), src);
    facts.symbols.push(Symbol {
        name: name.to_string(),
        kind,
        line: line_of(node),
        signature: sig.clone(),
    });
    Some(sig)
}

/// An `impl` has no name of its own, so it is named after the type it implements.
fn add_impl(node: Node, src: &str, facts: &mut Facts) {
    let Some(ty) = field_text(node, "type", src) else {
        return;
    };
    facts.symbols.push(Symbol {
        name: ty.to_string(),
        kind: Kind::Impl,
        line: line_of(node),
        signature: signature(node, node.child_by_field_name("body"), src),
    });
}

fn add_call(node: Node, callee: Option<&str>, facts: &mut Facts, enclosing: Option<&str>) {
    if let Some(callee) = callee {
        facts.calls.push(Call {
            callee: callee.to_string(),
            line: line_of(node),
            caller: enclosing.map(String::from),
        });
    }
}

/// The called name: `f()` -> f, `a::f()` -> f, `x.f()` -> f, `f::<T>()` -> f.
fn callee_name<'a>(f: Node, src: &'a str) -> Option<&'a str> {
    match f.kind() {
        "identifier" => Some(text(f, src)),
        "scoped_identifier" => field_text(f, "name", src),
        "field_expression" => field_text(f, "field", src),
        "generic_function" => callee_name(f.child_by_field_name("function")?, src),
        _ => None,
    }
}

fn is_pub(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|c| c.kind() == "visibility_modifier")
}

/// Local-crate name behind a `use` path: `crate::util::x` -> util, `serde::X` -> serde, `std::..` -> none.
fn use_root(path: &str) -> Option<String> {
    let mut parts = path.trim_start_matches("::").split("::");
    let mut first = parts.next()?;
    if matches!(first, "crate" | "super" | "self") {
        first = parts.next()?;
    }
    let name: String = first
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || *c == '_')
        .collect();
    let skip = matches!(
        name.as_str(),
        "" | "std" | "core" | "alloc" | "self" | "super"
    );
    (!skip && name.len() == first.len()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "use std::fmt;
use crate::util::helper;
use serde::Serialize;
mod net;
pub mod tests_helper { }
pub struct Greeter;
pub enum Color { Red }
pub trait Speak { fn speak(&self); }
impl Speak for Greeter {
    fn speak(&self) {
        helper();
        self.log(\"x\");
        println!(\"hi\");
    }
}
pub type Id = u32;
pub const MAX: usize = 3;
const PRIVATE: usize = 1;
fn outer() {
    fn inner() { deep(); }
    Greeter::new();
}
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Rust, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        let want = vec![
            ("net", Kind::Mod, 4),
            ("tests_helper", Kind::Mod, 5),
            ("Greeter", Kind::Struct, 6),
            ("Color", Kind::Enum, 7),
            ("Speak", Kind::Trait, 8),
            ("Greeter", Kind::Impl, 9),
            ("speak", Kind::Fn, 10),
            ("Id", Kind::Type, 16),
            ("MAX", Kind::Const, 17),
            ("outer", Kind::Fn, 19),
            ("inner", Kind::Fn, 20),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn signatures() {
        let facts = extract_file(
            Lang::Rust,
            "pub fn parse(input: &str)\n    -> Option<u8> {\n    None\n}\nimpl Speak for Greeter {}\n",
        )
        .unwrap();
        assert_eq!(
            facts.symbols[0].signature,
            "pub fn parse(input: &str) -> Option<u8>"
        );
        assert_eq!(facts.symbols[1].signature, "impl Speak for Greeter");
    }

    #[test]
    fn imports_skip_std_and_dedupe() {
        let facts = extract_file(Lang::Rust, SRC).unwrap();
        assert_eq!(facts.imports, vec!["net", "serde", "util"]);
    }

    #[test]
    fn calls_know_their_enclosing_fn() {
        let facts = extract_file(Lang::Rust, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
            .collect();
        let want = vec![
            ("helper", 11, Some("fn speak")),
            ("log", 12, Some("fn speak")),
            ("println", 13, Some("fn speak")),
            ("deep", 20, Some("fn inner")),
            ("new", 21, Some("fn outer")),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn items_inside_cfg_macros_are_found() {
        let src = "cfg_net! {\n    pub struct Foo {}\n    impl Foo { fn a() { b(); } }\n}\n";
        let facts = extract_file(Lang::Rust, src).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Foo", Kind::Struct, 2),
                ("Foo", Kind::Impl, 3),
                ("a", Kind::Fn, 3)
            ]
        );
        assert_eq!(facts.calls[1].callee, "b");
        assert_eq!(facts.calls[1].line, 3);
        assert_eq!(facts.calls[1].caller.as_deref(), Some("fn a"));
    }

    #[test]
    fn nested_cfg_macros_keep_line_numbers() {
        let src =
            "cfg_a! {\n    cfg_b! {\n        fn deep() { x(); }\n    }\n    fn after() {}\n}\n";
        let facts = extract_file(Lang::Rust, src).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.line))
            .collect();
        assert_eq!(got, vec![("deep", 3), ("after", 5)]);
        let x = facts.calls.iter().find(|c| c.callee == "x").unwrap();
        assert_eq!(x.line, 3);
    }

    #[test]
    fn calls_inside_macro_arguments_are_found() {
        let src = "fn t() {
    assert!(f(), \"m\");
    let v = vec![g(1), x.h()];
    println!(\"{}\", nested!(k()));
}
";
        let facts = extract_file(Lang::Rust, src).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
            .collect();
        let t = Some("fn t");
        let want = vec![
            ("assert", 2, t),
            ("f", 2, t),
            ("vec", 3, t),
            ("g", 3, t),
            ("h", 3, t),
            ("println", 4, t),
            ("nested", 4, t),
            ("k", 4, t),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn macro_arguments_do_not_count_fn_names_or_keywords() {
        let src = "m! { fn local(a: u8) {} if (x) { y } for i in (0..3) {} }\n";
        let facts = extract_file(Lang::Rust, src).unwrap();
        let names: Vec<_> = facts.calls.iter().map(|c| c.callee.as_str()).collect();
        assert_eq!(names, vec!["m"]);
    }

    #[test]
    fn cfg_macros_without_items_still_get_their_calls() {
        let src = "cfg_if! {\n    if #[cfg(unix)] {\n        f();\n    } else {\n        g();\n    }\n}\n";
        let facts = extract_file(Lang::Rust, src).unwrap();
        let names: Vec<_> = facts.calls.iter().map(|c| c.callee.as_str()).collect();
        assert!(names.contains(&"f") && names.contains(&"g"), "{names:?}");
    }

    #[test]
    fn restricted_visibility_does_not_cut_the_caller_name() {
        let src = "pub(crate) fn a() { x(); }\npub(in crate::m) fn b<T: Fn(u8)>() { y(); }\n";
        let facts = extract_file(Lang::Rust, src).unwrap();
        let callers: Vec<_> = facts.calls.iter().map(|c| c.caller.as_deref()).collect();
        assert_eq!(
            callers,
            vec![Some("pub(crate) fn a"), Some("pub(in crate::m) fn b")]
        );
    }
}
