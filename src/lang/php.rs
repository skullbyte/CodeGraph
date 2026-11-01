//! PHP extraction: classes, interfaces, traits, enums, functions, methods, imports and calls.
//! Ported idea: Graphify's PHP `LanguageConfig` in `extract.py` (Apache-2.0 / MIT, see NOTICE).

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("class_declaration", Kind::Class),
        ("interface_declaration", Kind::Interface),
        ("trait_declaration", Kind::Trait),
        ("enum_declaration", Kind::Enum),
        ("function_definition", Kind::Fn),
        ("method_declaration", Kind::Fn),
    ],
    calls: &[
        "function_call_expression",
        "member_call_expression",
        "nullsafe_member_call_expression",
        "scoped_call_expression",
        "object_creation_expression",
    ],
    scopes: &[],
    name: name_field,
    callee,
    imports,
};

/// Language constructs the grammar parses like function calls; they are no project function.
const CONSTRUCTS: [&str; 6] = ["isset", "empty", "unset", "exit", "die", "list"];

/// `new self`, `new static`, `new parent`: the class itself or its parent, not a project name.
const RELATIVE: [&str; 3] = ["self", "static", "parent"];

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// `f()` and `\a\f()` -> f; `$o->f()`, `$o?->f()` and `A::f()` -> f; `new A()` and `new \a\A()`
/// -> A. `new self`/`static`/`parent` and calls through a variable (`$f()`) are skipped.
fn callee(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "function_call_expression" => last_name(node.child_by_field_name("function")?, src)
            .filter(|name| !CONSTRUCTS.iter().any(|c| c.eq_ignore_ascii_case(name))),
        "object_creation_expression" => {
            let mut cursor = node.walk();
            let class = node
                .named_children(&mut cursor)
                .find(|c| matches!(c.kind(), "name" | "qualified_name"))?;
            last_name(class, src)
                .filter(|name| !RELATIVE.iter().any(|r| r.eq_ignore_ascii_case(name)))
        }
        _ => last_name(node.child_by_field_name("name")?, src),
    }
}

/// Text of a `name`, or of the last `name` of a `qualified_name` (`\a\b\C` -> C); None for
/// anything else (a variable, an expression).
fn last_name(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "name" => non_empty(node, src),
        "qualified_name" => {
            let mut cursor = node.walk();
            let last = node.named_children(&mut cursor).last()?;
            (last.kind() == "name")
                .then(|| non_empty(last, src))
                .flatten()
        }
        _ => None,
    }
}

/// `use A\B\C;` -> `A\B\C` (also with `as D`); `use A\{B, C};` -> `A\B`, `A\C`.
/// `use function ...` and `use const ...` name no class file.
fn imports(node: Node, src: &str, out: &mut Vec<String>) {
    if node.kind() != "namespace_use_clause" || node.child_by_field_name("type").is_some() {
        return;
    }
    let mut cursor = node.walk();
    let Some(name) = node
        .named_children(&mut cursor)
        .find(|c| matches!(c.kind(), "name" | "qualified_name"))
    else {
        return;
    };
    let path = text(name, src).trim_start_matches('\\');
    if path.is_empty() {
        return;
    }
    match group_prefix(node, src) {
        Some(prefix) => out.push(format!("{prefix}\\{path}")),
        None => out.push(path.to_string()),
    }
}

/// The `A\B` of `use A\B\{C, D};` for a clause inside the braces; None for a plain `use`.
/// A `use function A\{f}` group is skipped by its own `type` field.
fn group_prefix(clause: Node, src: &str) -> Option<String> {
    let declaration = clause.parent()?.parent()?;
    if declaration.kind() != "namespace_use_declaration"
        || declaration.child_by_field_name("type").is_some()
    {
        return None;
    }
    let mut cursor = declaration.walk();
    let prefix = declaration
        .named_children(&mut cursor)
        .find(|c| c.kind() == "namespace_name")?;
    Some(text(prefix, src).trim_start_matches('\\').to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "<?php
namespace App\\Shop;

use App\\Util\\Fmt;
use App\\Model\\Item as Thing;
use App\\Lib\\{Cart, Box};
use function App\\Util\\helper;

interface Named { public function id(): int; }

trait Loud { public function shout() { echo strtoupper('x'); } }

enum Suit { case Hearts; }

class Store extends Base implements Named {
    public function add(int $x): void {
        echo Fmt::format($x);
        $this->save($x);
        $y = new Thing();
        $z = new \\App\\Lib\\Cart();
        if (isset($y)) { helper($x); }
        parent::add($x);
        $a = new static();
        $b = new self;
        $c = new parent();
        $f = function () { return other(); };
        $y?->touch();
    }
}

function run() { return (new Store())->add(1); }
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Php, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Named", Kind::Interface, 9),
                ("id", Kind::Fn, 9),
                ("Loud", Kind::Trait, 11),
                ("shout", Kind::Fn, 11),
                ("Suit", Kind::Enum, 13),
                ("Store", Kind::Class, 15),
                ("add", Kind::Fn, 16),
                ("run", Kind::Fn, 31),
            ]
        );
        assert_eq!(
            facts.symbols[6].signature,
            "public function add(int $x): void"
        );
    }

    #[test]
    fn calls_know_their_enclosing_function() {
        let facts = extract_file(Lang::Php, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("strtoupper", Some("shout")),
                ("format", Some("add")),
                ("save", Some("add")),
                ("Thing", Some("add")),
                ("Cart", Some("add")),
                ("helper", Some("add")),
                ("add", Some("add")),
                ("other", Some("add")),
                ("touch", Some("add")),
                ("add", Some("run")),
                ("Store", Some("run")),
            ]
        );
    }

    #[test]
    fn imports_use_the_real_name_and_skip_functions() {
        let facts = extract_file(Lang::Php, SRC).unwrap();
        assert_eq!(
            facts.imports,
            vec![
                "App\\Lib\\Box",
                "App\\Lib\\Cart",
                "App\\Model\\Item",
                "App\\Util\\Fmt"
            ]
        );
    }

    #[test]
    fn extensions_are_picked_up() {
        assert_eq!(Lang::from_path("src/Store.php"), Some(Lang::Php));
    }
}
