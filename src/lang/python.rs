//! Python extraction: `def`, `class`, imports and calls, through the generic walker.

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, field_text, text};
use crate::model::{Form, Kind};

const CONFIG: Config = Config {
    items: &[
        ("function_definition", Kind::Fn),
        ("class_definition", Kind::Class),
    ],
    calls: &["call"],
    scopes: &[],
    containers: &["class_definition"],
    form: call_form,
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

/// `f()` Bare, `self.f()` / `cls.f()` SelfRef, `Foo.f()` Type (a capitalised name is a class);
/// `x.f()` is not narrowed: `x` may be a module, an instance of anything, or a class.
fn call_form(node: Node, src: &str) -> Form {
    let Some(f) = node.child_by_field_name("function") else {
        return Form::Unknown;
    };
    match f.kind() {
        "identifier" => Form::Bare,
        "attribute" => match f.child_by_field_name("object") {
            Some(o) if o.kind() == "identifier" => match text(o, src) {
                "self" | "cls" => Form::SelfRef,
                // A class name has a capital first letter and some lowercase (`CACHE` is a constant, `T` a class).
                name if name.starts_with(|c: char| c.is_ascii_uppercase())
                    && (name.len() == 1 || name.chars().any(|c| c.is_ascii_lowercase())) =>
                {
                    Form::Type(name.to_string())
                }
                _ => Form::Unknown,
            },
            _ => Form::Unknown,
        },
        _ => Form::Unknown,
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
    use crate::model::{Form, Kind};

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

    #[test]
    fn methods_know_their_class_and_nested_functions_do_not() {
        let src = "class A:\n    def run(self):\n        def inner():\n            pass\ndef free():\n    pass\n";
        let facts = extract_file(Lang::Python, src).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.container.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("A", None),
                ("run", Some("A")),
                ("inner", None),
                ("free", None)
            ]
        );
    }

    #[test]
    fn calls_record_how_they_are_written() {
        let src = "class A:
    def run(self):
        self.go()
        helper()
        B.make()
        x.y()
        super().init()
top()
";
        let facts = extract_file(Lang::Python, src).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.form.clone(), c.owner.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("go", Form::SelfRef, Some("A")),
                ("helper", Form::Bare, Some("A")),
                ("make", Form::Type("B".into()), Some("A")),
                ("y", Form::Unknown, Some("A")),
                ("init", Form::Unknown, Some("A")),
                ("super", Form::Bare, Some("A")),
                ("top", Form::Bare, None),
            ]
        );
    }

    fn walk(src: &str) -> crate::lang::Facts {
        extract_file(Lang::Python, src).unwrap()
    }

    #[test]
    fn constants_in_capitals_are_not_types() {
        let facts = walk("CACHE.get(1)\nDEFAULTS.update(2)\nMyClass.build()\n");
        let forms: Vec<_> = facts.calls.iter().map(|c| c.form.clone()).collect();
        assert_eq!(
            forms,
            vec![Form::Unknown, Form::Unknown, Form::Type("MyClass".into())]
        );
    }

    #[test]
    fn nested_classes_decorators_lambdas_and_class_bodies() {
        let src = "class A:
    x = make()
    class B:
        def m(self):
            self.go()
            f = lambda: self.stop()
    @staticmethod
    def s():
        helper()
    @deco()
    @property
    def p(self):
        pass
    def n(self):
        class C:
            def q(self): pass
";
        let facts = walk(src);
        let sym: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.container.as_deref()))
            .collect();
        assert_eq!(
            sym,
            vec![
                ("A", None),
                ("B", Some("A")),
                ("m", Some("B")),
                ("s", Some("A")),
                ("p", Some("A")),
                ("n", Some("A")),
                ("C", None),
                ("q", Some("C")),
            ]
        );
        let call = |name: &str| {
            let c = facts.calls.iter().find(|c| c.callee == name).unwrap();
            (c.caller.as_deref().map(|s| s.to_string()), c.owner.clone())
        };
        assert_eq!(call("make"), (None, Some("A".into())));
        assert_eq!(call("go"), (Some("m".into()), Some("B".into())));
        assert_eq!(call("stop"), (Some("m".into()), Some("B".into())));
        assert_eq!(call("helper"), (Some("s".into()), Some("A".into())));
        assert_eq!(call("deco"), (None, Some("A".into())));
    }
}
