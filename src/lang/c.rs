//! C and C++ extraction: functions, types, `#include "x.h"` and calls. The C++ grammar reads
//! plain C too, so one table serves both. `.h` files are tried as C first, then as C++.

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, field_text, text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("function_definition", Kind::Fn),
        ("class_specifier", Kind::Class),
        ("struct_specifier", Kind::Struct),
        ("union_specifier", Kind::Struct),
        ("enum_specifier", Kind::Enum),
        ("type_definition", Kind::Type),
    ],
    calls: &["call_expression"],
    scopes: &[],
    name,
    callee,
    imports,
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// A function's name sits inside its declarator (`char *f(int)` -> pointer -> function -> `f`);
/// `A::f` -> f. A typedef gives every name it declares. A type counts only where it is defined
/// (has a body), not where it is used.
fn name(node: Node, src: &str) -> Vec<String> {
    match node.kind() {
        "function_definition" => node
            .child_by_field_name("declarator")
            .and_then(|d| declarator_name(d, src))
            .into_iter()
            .collect(),
        "type_definition" => {
            let mut cursor = node.walk();
            node.children_by_field_name("declarator", &mut cursor)
                .filter_map(|d| declarator_name(d, src))
                .collect()
        }
        _ if node.child_by_field_name("body").is_none() => Vec::new(),
        _ => name_field(node, src),
    }
}

fn declarator_name(mut node: Node, src: &str) -> Option<String> {
    // `int (*getf(int))(int)`: a parenthesized declarator has no `declarator` field, only a child.
    while let Some(inner) = node
        .child_by_field_name("declarator")
        .or_else(|| (node.kind() == "parenthesized_declarator").then(|| node.named_child(0))?)
    {
        node = inner;
    }
    if !matches!(
        node.kind(),
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "operator_name"
            | "destructor_name"
            | "qualified_identifier"
    ) {
        return None;
    }
    let name = text(node, src);
    let name = name.rsplit("::").next().unwrap_or(name).trim();
    Some(name.to_string()).filter(|n| !n.is_empty())
}

/// `f()` -> f, `a.f()` / `a->f()` -> f, `ns::f()` and `f<T>()` -> f.
fn callee(node: Node, src: &str) -> Option<String> {
    let f = node.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => non_empty(f, src),
        "field_expression" => field_text(f, "field", src).map(String::from),
        "qualified_identifier" => {
            let t = text(f, src);
            non_empty_name(t.rsplit("::").next().unwrap_or(t))
        }
        "template_function" => field_text(f, "name", src).map(String::from),
        _ => None,
    }
}

fn non_empty_name(name: &str) -> Option<String> {
    Some(name.trim())
        .filter(|n| !n.is_empty())
        .map(String::from)
}

/// `#include "x.h"` -> `x.h`; `<stdio.h>` is a system header with no project file.
fn imports(node: Node, src: &str, out: &mut Vec<String>) {
    if node.kind() != "preproc_include" {
        return;
    }
    if let Some(path) = node.child_by_field_name("path")
        && path.kind() == "string_literal"
    {
        out.extend(non_empty_name(text(path, src).trim_matches('"')));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const C_SRC: &str = "#include <stdio.h>
#include \"util.h\"
typedef struct { int a; } Pair;
struct Node { int v; };
struct Node *head;
enum Mode { A };
static int add(int a, int b) { return fmt(a) + b; }
char *name(void) { printf(\"x\"); p->run(); return 0; }
";

    #[test]
    fn c_symbols_imports_and_calls() {
        let facts = extract_file(Lang::C, C_SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Pair", Kind::Type, 3),
                ("Node", Kind::Struct, 4),
                ("Mode", Kind::Enum, 6),
                ("add", Kind::Fn, 7),
                ("name", Kind::Fn, 8),
            ]
        );
        assert_eq!(facts.symbols[3].signature, "static int add(int a, int b)");
        assert_eq!(facts.imports, vec!["util.h"]);
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("fmt", Some("add")),
                ("printf", Some("name")),
                ("run", Some("name"))
            ]
        );
    }

    #[test]
    fn cpp_special_functions_templates_and_namespaces() {
        let facts = extract_file(
            Lang::Cpp,
            "namespace n { template <class T> T twice(T x) { return x; } }
extern \"C\" { void c_api() {} }
struct S { ~S() {} bool operator==(const S&) const { return true; } };
int (*getf(int a))(int) { return 0; }
typedef struct Foo *FooPtr;
struct Node *head;
",
        )
        .unwrap();
        let names: Vec<_> = facts.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["twice", "c_api", "S", "~S", "operator==", "getf", "FooPtr"]
        );
    }

    #[test]
    fn typedefs_record_every_name_and_old_style_c_headers_parse() {
        let facts = extract_file(
            Lang::Header,
            "typedef struct { int a; } A, *B;\ntypedef int (*cb)(int);\nint old(a) int a; { return a; }\nint new = 1;\n",
        )
        .unwrap();
        let names: Vec<_> = facts.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "cb", "old"]);
        let cpp = extract_file(Lang::Header, "class Store { void add() {} };\n").unwrap();
        assert_eq!(cpp.symbols[0].name, "Store");
    }

    #[test]
    fn header_broken_in_both_grammars_keeps_the_c_symbols() {
        let facts =
            extract_file(Lang::Header, "int ok(void) { return 0; }\n@@@ ((( }}}\n").unwrap();
        assert!(facts.symbols.iter().any(|s| s.name == "ok"));
    }

    #[test]
    fn cpp_classes_methods_and_qualified_calls() {
        let facts = extract_file(
            Lang::Cpp,
            "class Store { public: void add(int x) { items.push_back(x); } };
void Store::run() { helper<int>(1); ns::go(); }
",
        )
        .unwrap();
        let symbols: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert_eq!(
            symbols,
            vec![("Store", Kind::Class), ("add", Kind::Fn), ("run", Kind::Fn)]
        );
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("push_back", Some("add")),
                ("helper", Some("run")),
                ("go", Some("run"))
            ]
        );
    }
}
