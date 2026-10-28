//! Python extraction: `def`, `class`, imports and calls, through the generic walker.

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, field_text, text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("function_definition", Kind::Fn),
        ("class_definition", Kind::Class),
    ],
    calls: &["call"],
    name: name_field,
    callee,
    imports,
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// `f()` -> f, `a.f()` -> f; anything fancier is skipped.
fn callee(node: Node, src: &str) -> Option<String> {
    let f = node.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => non_empty(f, src),
        "attribute" => field_text(f, "attribute", src).map(String::from),
        _ => None,
    }
}

/// `import a.b` -> `a.b`; `from .x import y` -> `.x`; `from . import y` -> `.y` (y may be a module);
/// `from p import y` -> `p` and `p.y`.
fn imports(node: Node, src: &str, out: &mut Vec<String>) {
    match node.kind() {
        "import_statement" => {
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                out.extend(module_name(name, src));
            }
        }
        "import_from_statement" => {
            let Some(module) = node.child_by_field_name("module_name") else {
                return;
            };
            let module = text(module, src);
            if module.chars().all(|c| c == '.') {
                let mut cursor = node.walk();
                for name in node.children_by_field_name("name", &mut cursor) {
                    out.extend(module_name(name, src).map(|n| format!("{module}{n}")));
                }
            } else {
                out.push(module.to_string());
                // `from pkg import sub` may import the module `pkg.sub`; unresolved ones are ignored later.
                let mut cursor = node.walk();
                for name in node.children_by_field_name("name", &mut cursor) {
                    out.extend(module_name(name, src).map(|n| format!("{module}.{n}")));
                }
            }
        }
        _ => {}
    }
}

/// `a.b` or `a.b as c` -> `a.b`.
fn module_name(node: Node, src: &str) -> Option<String> {
    let dotted = if node.kind() == "aliased_import" {
        node.child_by_field_name("name")?
    } else {
        node
    };
    non_empty(dotted, src)
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "import os, a.b as ab
from .sibling import helper
from . import pkg
from ..up import thing
class Cart(Base):
    def add(self, x):
        self.items.append(x)
        return helper(x)
def show(n):
    return Cart().add(n)
";

    #[test]
    fn symbols_and_signatures() {
        let facts = extract_file(Lang::Python, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line, s.signature.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Cart", Kind::Class, 5, "class Cart(Base)"),
                ("add", Kind::Fn, 6, "def add(self, x)"),
                ("show", Kind::Fn, 9, "def show(n)"),
            ]
        );
    }

    #[test]
    fn imports_keep_dotted_and_relative_modules() {
        let facts = extract_file(Lang::Python, SRC).unwrap();
        assert_eq!(
            facts.imports,
            vec![
                "..up",
                "..up.thing",
                ".pkg",
                ".sibling",
                ".sibling.helper",
                "a.b",
                "os"
            ]
        );
    }

    #[test]
    fn decorated_async_and_nested_definitions() {
        let facts = extract_file(
            Lang::Python,
            "@route('/')\nasync def handler():\n    def inner():\n        deep()\n    outer()\n",
        )
        .unwrap();
        let names: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.line))
            .collect();
        assert_eq!(names, vec![("handler", 2), ("inner", 3)]);
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("route", None),
                ("deep", Some("inner")),
                ("outer", Some("handler"))
            ]
        );
    }

    #[test]
    fn calls_know_their_enclosing_function() {
        let facts = extract_file(Lang::Python, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("append", 7, Some("add")),
                ("helper", 8, Some("add")),
                ("add", 10, Some("show")),
                ("Cart", 10, Some("show")),
            ]
        );
    }
}
