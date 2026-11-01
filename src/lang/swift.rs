//! Swift extraction: types, protocols, typealiases, functions, initializers and calls. A Swift import names a
//! module, not a file, so no imports. Ported idea: Graphify's Swift `LanguageConfig` in
//! `extract.py` (Apache-2.0 / MIT, see NOTICE).

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, field_text};
use crate::model::Kind;

// `class_declaration` is also a struct, an enum, an actor or an extension; they all become classes
// except the extension, which adds no symbol (its methods are still found). Known gap: calls in a
// computed property (`var x: Int { compute() }`) have no caller.
const CONFIG: Config = Config {
    items: &[
        ("class_declaration", Kind::Class),
        ("protocol_declaration", Kind::Interface),
        ("typealias_declaration", Kind::Type),
        ("function_declaration", Kind::Fn),
        ("protocol_function_declaration", Kind::Fn),
        ("init_declaration", Kind::Fn),
        ("deinit_declaration", Kind::Fn),
        ("subscript_declaration", Kind::Fn),
    ],
    calls: &["call_expression", "constructor_expression"],
    scopes: &[],
    name,
    callee,
    imports: |_, _, _| {},
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// The `name` field, except for `extension Foo` (it only adds to a type declared elsewhere) and
/// for `deinit` and `subscript`, which have fixed names.
fn name(node: Node, src: &str) -> Vec<String> {
    match node.kind() {
        "deinit_declaration" => vec!["deinit".to_string()],
        "subscript_declaration" => vec!["subscript".to_string()],
        "class_declaration" if field_text(node, "declaration_kind", src) == Some("extension") => {
            Vec::new()
        }
        _ => name_field(node, src),
    }
}

/// `f(x)` and `Type(x)` -> the name; `a.b.f(x)` and `f(x)!` -> f; `Foo<Int>(x)` and
/// `Outer.Inner<Int>(x)` -> Foo, Inner. `super.init(..)` and `self.init(..)` are skipped (they
/// would link to every initializer). Anything else (`f()()`, `(g)(x)`) is skipped too.
fn callee(node: Node, src: &str) -> Option<String> {
    if node.kind() == "constructor_expression" {
        return last_type_name(node.child_by_field_name("constructed_type")?, src);
    }
    callee_of(node.named_child(0)?, src)
}

fn callee_of(target: Node, src: &str) -> Option<String> {
    match target.kind() {
        "simple_identifier" => non_empty(target, src),
        "postfix_expression" => callee_of(target.child_by_field_name("target")?, src),
        "navigation_expression" => {
            let name = non_empty(
                target
                    .child_by_field_name("suffix")?
                    .child_by_field_name("suffix")?,
                src,
            )?;
            let owner = target.child_by_field_name("target")?;
            let own_init =
                name == "init" && matches!(owner.kind(), "self_expression" | "super_expression");
            (!own_init).then_some(name)
        }
        _ => None,
    }
}

/// `Outer.Inner<Int>` -> Inner.
fn last_type_name(ty: Node, src: &str) -> Option<String> {
    let mut cursor = ty.walk();
    let last = ty
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "type_identifier")
        .last()?;
    non_empty(last, src)
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "import Foundation

protocol Named { func id() -> Int }

typealias Id = Int

struct Item { let n: Int }
enum Suit { case hearts }

class Store: Named {
    init() { setup() }
    func add(_ x: Int) {
        print(format(x))
        helper.run(x)
        self.save()
        let i = Item(n: 1)
    }
}

extension Store {
    func extra() { add(1) }
}

func run() { Store().add(2) }
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Swift, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Named", Kind::Interface, 3),
                ("id", Kind::Fn, 3),
                ("Id", Kind::Type, 5),
                ("Item", Kind::Class, 7),
                ("Suit", Kind::Class, 8),
                ("Store", Kind::Class, 10),
                ("init", Kind::Fn, 11),
                ("add", Kind::Fn, 12),
                ("extra", Kind::Fn, 21),
                ("run", Kind::Fn, 24),
            ]
        );
        assert_eq!(facts.symbols[7].signature, "func add(_ x: Int)");
    }

    #[test]
    fn calls_know_their_enclosing_function() {
        let facts = extract_file(Lang::Swift, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("setup", Some("init")),
                ("print", Some("add")),
                ("format", Some("add")),
                ("run", Some("add")),
                ("save", Some("add")),
                ("Item", Some("add")),
                ("add", Some("extra")),
                ("add", Some("run")),
                ("Store", Some("run")),
            ]
        );
    }

    #[test]
    fn optional_chaining_try_await_closures_and_actors() {
        let facts = extract_file(
            Lang::Swift,
            "actor Bank {\n    func go() async throws {\n        try await load()\n        a?.touch()\n        Outer.Inner.make()\n        items.map { log($0) }\n    }\n}\n",
        )
        .unwrap();
        let symbols: Vec<_> = facts.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(symbols, vec!["Bank", "go"]);
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("load", Some("go")),
                ("touch", Some("go")),
                ("make", Some("go")),
                ("map", Some("go")),
                ("log", Some("go")),
            ]
        );
    }

    #[test]
    fn generics_postfix_deinit_subscript_and_own_initializers() {
        let facts = extract_file(
            Lang::Swift,
            "class A: B {\n    init() { super.init(); self.init(1); make() }\n    deinit { cleanup() }\n    subscript(i: Int) -> Int { get { fetch(i) } }\n    func go() { _ = Foo<Int>(); _ = Outer.Inner<Int>(); _ = load()!; x.f()?.g()! }\n}\nextension Array<Int> { func twice() { dup() } }\n",
        )
        .unwrap();
        let symbols: Vec<_> = facts.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            symbols,
            vec!["A", "init", "deinit", "subscript", "go", "twice"]
        );
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("make", Some("init")),
                ("cleanup", Some("deinit")),
                ("fetch", Some("subscript")),
                ("Foo", Some("go")),
                ("Inner", Some("go")),
                ("load", Some("go")),
                ("g", Some("go")),
                ("f", Some("go")),
                ("dup", Some("twice")),
            ]
        );
    }

    #[test]
    fn swift_has_no_file_imports() {
        assert!(extract_file(Lang::Swift, SRC).unwrap().imports.is_empty());
    }

    #[test]
    fn extensions_are_picked_up() {
        assert_eq!(Lang::from_path("Sources/Store.swift"), Some(Lang::Swift));
    }
}
