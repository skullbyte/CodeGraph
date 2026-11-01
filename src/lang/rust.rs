//! Rust extraction: walks the syntax tree and records items, `use`/`mod` imports and calls.

use tree_sitter::{Node, Parser};

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Form, Kind, Symbol};

/// Where a walk is: the innermost `fn` (the caller of what it finds), the type or trait whose
/// method that `fn` is (the `owner` of its calls) and the type or trait that items declared here
/// are members of (their `container`). A `fn` nested in a method has neither: it has no `self`.
#[derive(Clone, Copy, Default)]
struct Ctx<'a> {
    enclosing: Option<&'a str>,
    owner: Option<&'a str>,
    container: Option<&'a str>,
    /// The nearest enclosing body is an `impl` or `trait` (not a function): its `fn`s are methods.
    in_type_body: bool,
}

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = Facts::default();
    visit(root, src, &mut facts, Ctx::default());
    facts
}

/// Depth-first walk.
fn visit(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut own = None;
    let mut inner = ctx;
    let impl_type = (node.kind() == "impl_item")
        .then(|| field_text(node, "type", src).map(type_name))
        .flatten();
    match node.kind() {
        "function_item" => {
            own = add_item(node, Kind::Fn, src, facts, ctx.container).map(|s| caller_name(&s));
            // What is declared inside a function belongs to no type; a nested `fn` has no `self`.
            inner.container = None;
            if !ctx.in_type_body {
                inner.owner = None;
            }
            inner.in_type_body = false;
        }
        "struct_item" => _ = add_item(node, Kind::Struct, src, facts, ctx.container),
        "enum_item" => _ = add_item(node, Kind::Enum, src, facts, ctx.container),
        "trait_item" => {
            add_item(node, Kind::Trait, src, facts, ctx.container);
            let name = field_text(node, "name", src);
            (inner.owner, inner.container) = (name, name);
            inner.in_type_body = true;
        }
        "type_item" => _ = add_item(node, Kind::Type, src, facts, ctx.container),
        "mod_item" => {
            add_item(node, Kind::Mod, src, facts, ctx.container);
            if node.child_by_field_name("body").is_none() {
                facts
                    .imports
                    .extend(field_text(node, "name", src).map(String::from));
            }
        }
        "impl_item" => {
            add_impl(node, src, facts);
            (inner.owner, inner.container) = (impl_type, impl_type);
            inner.in_type_body = true;
        }
        "const_item" if is_pub(node) => {
            _ = add_item(node, Kind::Const, src, facts, ctx.container);
        }
        "use_declaration" => {
            facts.imports.extend(
                node.child_by_field_name("argument")
                    .and_then(|a| use_root(text(a, src))),
            );
        }
        "call_expression" => {
            let function = node.child_by_field_name("function");
            let callee = function.and_then(|f| callee_name(f, src));
            let form = function.map_or(Form::Unknown, |f| form_of(f, src));
            add_call(node, callee, form, facts, ctx);
        }
        "macro_invocation" => {
            let callee = node
                .child_by_field_name("macro")
                .and_then(|f| callee_name(f, src));
            add_call(node, callee, Form::Unknown, facts, ctx);
            let before = (facts.symbols.len(), facts.calls.len());
            if callee.is_some_and(|name| name.starts_with("cfg_")) {
                visit_macro_items(node, src, facts, ctx);
            }
            // Not items (e.g. `cfg_if!`) or not a cfg macro: scan the raw tokens instead.
            if before == (facts.symbols.len(), facts.calls.len())
                && let Some(args) = macro_args(node)
            {
                visit_token_calls(args, src, facts, ctx);
            }
        }
        _ => {}
    }
    inner.enclosing = own.as_deref().or(ctx.enclosing);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, facts, inner);
    }
}

/// Macros like tokio's `cfg_net! { ... }` wrap whole items, which tree-sitter only sees as
/// raw tokens. Re-parse the macro body as ordinary Rust and shift its line numbers back.
fn visit_macro_items(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
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
    visit(tree.root_node(), inner, facts, ctx);
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
fn visit_token_calls(tokens: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut cursor = tokens.walk();
    let kids: Vec<Node> = tokens.children(&mut cursor).collect();
    for (i, kid) in kids.iter().enumerate() {
        if kid.kind() == "token_tree" {
            visit_token_calls(*kid, src, facts, ctx);
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
            let form = if macro_next {
                Form::Unknown
            } else {
                token_form(&kids, i, src)
            };
            add_call(*kid, Some(name), form, facts, ctx);
        }
    }
}

/// The form of a call found among raw macro tokens, from the tokens just before the name:
/// `. name` is a method (`self . name` an own method), `Foo :: name` a member of Foo.
fn token_form(kids: &[Node], at: usize, src: &str) -> Form {
    let before = |back: usize| at.checked_sub(back).map(|i| text(kids[i], src));
    match before(1) {
        Some(".") if before(2) == Some("self") => Form::SelfRef,
        Some(".") => Form::Method,
        Some("::") => qualified(before(2).filter(|q| !q.is_empty())),
        _ => Form::Bare,
    }
}

/// Record a named item; returns its signature.
fn add_item(
    node: Node,
    kind: Kind,
    src: &str,
    facts: &mut Facts,
    container: Option<&str>,
) -> Option<String> {
    let name = field_text(node, "name", src)?;
    let sig = signature(node, node.child_by_field_name("body"), src);
    facts.symbols.push(Symbol {
        name: name.to_string(),
        kind,
        line: line_of(node),
        signature: sig.clone(),
        container: container.map(String::from),
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
        container: None,
    });
}

fn add_call(node: Node, callee: Option<&str>, form: Form, facts: &mut Facts, ctx: Ctx) {
    if let Some(callee) = callee {
        facts.calls.push(Call {
            callee: callee.to_string(),
            line: line_of(node),
            caller: ctx.enclosing.map(String::from),
            form,
            owner: ctx.owner.map(String::from),
        });
    }
}

/// `Foo<T>`, `&'a mut Foo`, `dyn Foo`, `a::b::Foo<T>`, `Vec::<u8>` -> `Foo`, `Foo`, `Foo`, `Foo`, `Vec`.
fn type_name(ty: &str) -> &str {
    let mut ty = ty.trim();
    while let Some(rest) = ty
        .strip_prefix('&')
        .or_else(|| ty.strip_prefix("mut "))
        .or_else(|| ty.strip_prefix("dyn "))
        .or_else(|| ty.strip_prefix("impl "))
        .or_else(|| {
            ty.starts_with('\'')
                .then(|| ty.split_once(char::is_whitespace).map(|(_, rest)| rest))
                .flatten()
        })
    {
        ty = rest.trim_start();
    }
    let ty = ty.split('<').next().unwrap_or(ty).trim_end_matches("::");
    ty.rsplit("::").next().unwrap_or(ty).trim()
}

/// Primitive types: `u64::from(x)` is a method of a std type, not a function of a module `u64`.
const PRIMITIVES: [&str; 17] = [
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32",
    "f64", "bool", "char", "str",
];

/// `Self::f` own method, `Foo::f` member of Foo, `crate::f` / `self::f` / `super::f` a plain call,
/// `m::f` a function of module m. A qualifier that is no plain name (`<T as Tr>`, `>` after a
/// turbofish) or a primitive type says nothing we can use.
fn qualified(qualifier: Option<&str>) -> Form {
    let word = |q: &str| q.starts_with(|c: char| c.is_alphabetic() || c == '_');
    match qualifier {
        Some("Self") => Form::SelfRef,
        Some("crate" | "super" | "self") => Form::Bare,
        Some(q) if !word(q) || PRIMITIVES.contains(&q) => Form::Unknown,
        Some(q) if q.starts_with(|c: char| c.is_uppercase()) => Form::Type(q.to_string()),
        Some(q) => Form::Module(q.to_string()),
        None => Form::Unknown,
    }
}

/// How a call is written: `f()` bare, `self.f()` / `Self::f()` own method, `Foo::f()` a member of
/// type Foo, `m::f()` a function of module m, `x.f()` a method of an unseen type.
fn form_of(f: Node, src: &str) -> Form {
    match f.kind() {
        "identifier" => Form::Bare,
        "generic_function" => f
            .child_by_field_name("function")
            .map_or(Form::Unknown, |inner| form_of(inner, src)),
        "field_expression" => match f.child_by_field_name("value").map(|v| text(v, src)) {
            Some("self") => Form::SelfRef,
            Some(_) => Form::Method,
            None => Form::Unknown,
        },
        "scoped_identifier" => {
            let path = f.child_by_field_name("path");
            if path.is_some_and(|p| p.kind() == "bracketed_type") {
                return Form::Unknown; // `<T as Trait>::f`, `<Foo>::f`
            }
            let qualifier = path.map(|p| type_name(text(p, src)));
            qualified(qualifier)
        }
        _ => Form::Unknown,
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
    use crate::model::{Form, Kind};

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
    fn items_know_their_container_and_calls_know_their_form() {
        let facts = extract_file(
            Lang::Rust,
            "struct Foo;\nimpl<T> Foo<T> {\n    fn a(&self) {\n        self.b();\n        Self::c();\n        Bar::d();\n        m::e();\n        x.f();\n        g();\n        Vec::<u8>::new();\n        a::b::Baz::q();\n    }\n}\ntrait Tr {\n    fn t(&self) { self.u(); }\n}\nimpl Tr for &Foo {}\nfn free() { h(); }\n",
        )
        .unwrap();
        let containers: Vec<_> = facts
            .symbols
            .iter()
            .filter(|s| s.kind != Kind::Impl)
            .map(|s| (s.name.as_str(), s.container.as_deref()))
            .collect();
        assert_eq!(
            containers,
            vec![
                ("Foo", None),
                ("a", Some("Foo")),
                ("Tr", None),
                ("t", Some("Tr")),
                ("free", None)
            ]
        );
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.form.clone(), c.owner.as_deref()))
            .collect();
        let ty = |t: &str| Form::Type(t.to_string());
        assert_eq!(
            calls,
            vec![
                ("b", Form::SelfRef, Some("Foo")),
                ("c", Form::SelfRef, Some("Foo")),
                ("d", ty("Bar"), Some("Foo")),
                ("e", Form::Module("m".into()), Some("Foo")),
                ("f", Form::Method, Some("Foo")),
                ("g", Form::Bare, Some("Foo")),
                ("new", ty("Vec"), Some("Foo")),
                ("q", ty("Baz"), Some("Foo")),
                ("u", Form::SelfRef, Some("Tr")),
                ("h", Form::Bare, None),
            ]
        );
    }

    #[test]
    fn odd_paths_never_claim_a_module_or_a_method_they_cannot_know() {
        let facts = extract_file(
            Lang::Rust,
            "fn a() {\n    self::b();\n    crate::m::c();\n    <T as Tr>::d();\n    <Foo>::e();\n    u64::from(1);\n    str::len(x);\n    super::f();\n    (self.cb)();\n}\n",
        )
        .unwrap();
        let forms: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.form.clone()))
            .collect();
        assert_eq!(
            forms,
            vec![
                ("b", Form::Bare),
                ("c", Form::Module("m".into())),
                ("d", Form::Unknown),
                ("e", Form::Unknown),
                ("from", Form::Unknown),
                ("len", Form::Unknown),
                ("f", Form::Bare),
            ]
        );
    }

    #[test]
    fn a_nested_fn_belongs_to_no_type_and_a_methods_own_calls_keep_theirs() {
        let facts = extract_file(
            Lang::Rust,
            "impl Foo {\n    fn m(&self) {\n        fn helper() { deep(); }\n        struct Local;\n        self.own();\n    }\n}\n",
        )
        .unwrap();
        let containers: Vec<_> = facts
            .symbols
            .iter()
            .filter(|s| s.kind != Kind::Impl)
            .map(|s| (s.name.as_str(), s.container.as_deref()))
            .collect();
        assert_eq!(
            containers,
            vec![("m", Some("Foo")), ("helper", None), ("Local", None)]
        );
        let owners: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.owner.as_deref()))
            .collect();
        assert_eq!(owners, vec![("deep", None), ("own", Some("Foo"))]);
    }

    #[test]
    fn an_impl_inside_a_function_still_gives_its_methods_an_owner() {
        let facts = extract_file(
            Lang::Rust,
            "fn t() {\n    impl Foo {\n        fn a(&self) { self.b(); fn inner() { deep(); } }\n    }\n    trait Tr { fn d(&self) { self.e(); } }\n    free();\n}\n",
        )
        .unwrap();
        let owners: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.owner.as_deref()))
            .collect();
        assert_eq!(
            owners,
            vec![
                ("b", Some("Foo")),
                ("deep", None),
                ("e", Some("Tr")),
                ("free", None)
            ]
        );
    }

    #[test]
    fn impl_targets_lose_references_lifetimes_dyn_and_generics() {
        let facts = extract_file(
            Lang::Rust,
            "impl<'a> Tr for &'a mut Foo<'a> { fn a(&self) {} }\nimpl Tr for dyn Bar { fn b(&self) {} }\nimpl<T> Tr for a::Baz<T> { fn c(&self) {} }\n",
        )
        .unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .filter(|s| s.kind == Kind::Fn)
            .map(|s| (s.name.as_str(), s.container.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![("a", Some("Foo")), ("b", Some("Bar")), ("c", Some("Baz"))]
        );
    }

    #[test]
    fn calls_inside_macro_arguments_read_their_form_from_the_tokens() {
        let facts = extract_file(
            Lang::Rust,
            "impl Foo {\n    fn a(&self) { vec![f(), self.g(), x.h(), Bar::i(), m::j(), crate::k(), Self::l(), n!()]; }\n}\n",
        )
        .unwrap();
        let forms: Vec<_> = facts
            .calls
            .iter()
            .skip(1)
            .map(|c| (c.callee.as_str(), c.form.clone()))
            .collect();
        assert_eq!(
            forms,
            vec![
                ("f", Form::Bare),
                ("g", Form::SelfRef),
                ("h", Form::Method),
                ("i", Form::Type("Bar".into())),
                ("j", Form::Module("m".into())),
                ("k", Form::Bare),
                ("l", Form::SelfRef),
                ("n", Form::Unknown),
            ]
        );
        let turbofish = extract_file(Lang::Rust, "fn a() { vec![Vec::<u8>::new()]; }\n").unwrap();
        let forms: Vec<_> = turbofish
            .calls
            .iter()
            .skip(1)
            .map(|c| (c.callee.as_str(), c.form.clone()))
            .collect();
        assert_eq!(
            forms,
            vec![("new", Form::Unknown)],
            "`>` before `::` is no qualifier"
        );
    }

    #[test]
    fn macro_calls_have_no_form_but_keep_their_owner() {
        let facts = extract_file(
            Lang::Rust,
            "impl Foo {\n    fn a(&self) { println!(\"x\"); vec![helper(1)]; }\n}\n",
        )
        .unwrap();
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.form.clone(), c.owner.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("println", Form::Unknown, Some("Foo")),
                ("vec", Form::Unknown, Some("Foo")),
                ("helper", Form::Bare, Some("Foo")),
            ]
        );
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
