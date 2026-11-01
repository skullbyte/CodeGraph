//! TypeScript/TSX extraction: walks the syntax tree and records declarations, imports and calls.

use std::collections::HashMap;

use tree_sitter::Node;

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Form, Kind, Symbol};

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = Facts::default();
    let modules = module_names(root, src);
    let ctx = Ctx {
        enclosing: None,
        container: None,
        modules: &modules,
    };
    visit(root, src, &mut facts, ctx);
    facts
}

/// Where the walk is: the innermost function (`enclosing`), the class or interface whose members
/// we are reading (`container`, also the owner of the calls inside), and which local names are
/// imported from another file (name -> module name, see `module_stem`).
#[derive(Clone, Copy)]
struct Ctx<'a> {
    enclosing: Option<&'a str>,
    container: Option<&'a str>,
    modules: &'a HashMap<String, String>,
}

/// Depth-first walk.
fn visit(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut own = None;
    let mut inner = ctx;
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            own = add_item(node, Kind::Fn, src, facts, None).map(|s| caller_name(&s));
            inner.container = None; // a nested function has no `this`
        }
        "function_signature" => _ = add_item(node, Kind::Fn, src, facts, None),
        "method_definition" => {
            own = add_item(node, Kind::Fn, src, facts, ctx.container).map(|s| caller_name(&s));
        }
        "method_signature" | "abstract_method_signature" if in_member_body(node) => {
            _ = add_item(node, Kind::Fn, src, facts, ctx.container);
        }
        "class_declaration" | "abstract_class_declaration" => {
            add_item(node, Kind::Class, src, facts, None);
            inner.container = field_text(node, "name", src);
        }
        "interface_declaration" => {
            add_item(node, Kind::Interface, src, facts, None);
            inner.container = field_text(node, "name", src);
        }
        // `this` inside these is not the surrounding class.
        "object" | "function_expression" => inner.container = None,
        "class" => inner.container = declarator_name(node, src),
        "type_alias_declaration" => _ = add_item(node, Kind::Type, src, facts, None),
        "enum_declaration" => _ = add_item(node, Kind::Enum, src, facts, None),
        "variable_declarator" => own = add_variable(node, src, facts),
        "import_statement" | "export_statement" => add_import(node, src, facts),
        "call_expression" => add_call(node, src, facts, ctx),
        _ => {}
    }
    inner.enclosing = own.as_deref().or(ctx.enclosing);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, facts, inner);
    }
}

/// Local names bound by relative imports (`import * as z from './core/index.js'`,
/// `import { helper } from './util'`) -> the module name a call through them can be found in.
/// Aliased specifiers (`a as b`) are left out: the local name is not the definition's name.
/// Known gaps, on purpose: only top-level imports are read, a local variable that shadows an
/// import is not noticed, and a namespace from a package becomes a module name too (`z` from
/// `zod/v3` -> `v3`, needed to find `z.custom()`); `super.f()` stays a plain method call.
fn module_names(root: Node, src: &str) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut cursor = root.walk();
    for stmt in root.named_children(&mut cursor) {
        if stmt.kind() != "import_statement" {
            continue;
        }
        let Some(spec) = field_text(stmt, "source", src) else {
            continue;
        };
        let Some(stem) = module_stem(spec) else {
            continue;
        };
        let relative = spec.trim_matches(['\'', '"', '`']).starts_with('.');
        let mut c = stmt.walk();
        for clause in stmt
            .named_children(&mut c)
            .filter(|n| n.kind() == "import_clause")
        {
            let mut cc = clause.walk();
            for part in clause.named_children(&mut cc) {
                match part.kind() {
                    "identifier" if relative => {
                        names.insert(text(part, src).to_string(), stem.clone());
                    }
                    "namespace_import" => {
                        if let Some(id) = part.named_child(0) {
                            names.insert(text(id, src).to_string(), stem.clone());
                        }
                    }
                    "named_imports" if relative => collect_specifiers(part, src, &stem, &mut names),
                    _ => {}
                }
            }
        }
    }
    names
}

fn collect_specifiers(named: Node, src: &str, stem: &str, names: &mut HashMap<String, String>) {
    let mut cursor = named.walk();
    for spec in named.named_children(&mut cursor) {
        let plain =
            spec.kind() == "import_specifier" && spec.child_by_field_name("alias").is_none();
        if let Some(name) = field_text(spec, "name", src).filter(|_| plain) {
            names.insert(name.to_string(), stem.to_string());
        }
    }
}

/// `./price` -> `price`, `../core/index.js` -> `core`, `zod/v3` -> `v3`, `@scope/pkg` -> `pkg`:
/// the last name of the path that is not `index`.
fn module_stem(specifier: &str) -> Option<String> {
    let path = specifier.trim_matches(['\'', '"', '`']);
    let mut parts = path.rsplit('/');
    let file = parts.next()?;
    let stem = file.split('.').next().unwrap_or("");
    let name = if stem == "index" {
        parts.find(|p| !matches!(*p, "." | ".." | ""))?
    } else {
        stem
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// A signature that is a member of an interface or class (not of an inline `{ f(): void }` type).
fn in_member_body(node: Node) -> bool {
    node.parent().is_some_and(|p| match p.kind() {
        "class_body" | "interface_body" => true,
        "object_type" => p
            .parent()
            .is_some_and(|g| g.kind() == "interface_declaration"),
        _ => false,
    })
}

/// `const B = class {...}`: the name the class expression is bound to.
fn declarator_name<'a>(class: Node, src: &'a str) -> Option<&'a str> {
    let parent = class
        .parent()
        .filter(|p| p.kind() == "variable_declarator")?;
    field_text(parent, "name", src)
}

/// The node whose text starts the declaration: the `export ...` wrapper when there is one.
fn decl_start(node: Node) -> Node {
    let mut top = node;
    while let Some(p) = top
        .parent()
        .filter(|p| matches!(p.kind(), "export_statement" | "ambient_declaration"))
    {
        top = p;
    }
    top
}

/// The declaration inside `export` / `declare` wrappers.
fn unwrap_decl(mut node: Node) -> Node {
    while matches!(node.kind(), "export_statement" | "ambient_declaration") {
        let inner = node
            .child_by_field_name("declaration")
            .or_else(|| node.named_child(0));
        match inner {
            Some(i) => node = i,
            None => break,
        }
    }
    node
}

/// Record a named declaration; returns its signature.
fn add_item(
    node: Node,
    kind: Kind,
    src: &str,
    facts: &mut Facts,
    container: Option<&str>,
) -> Option<String> {
    let name = field_text(node, "name", src)?;
    let start = decl_start(node);
    let sig = signature(start, node.child_by_field_name("body"), src);
    // Overloads (`f(a): A; f(b): B; f(c) {}`) are one definition, at the first signature.
    let mut before = start.prev_named_sibling();
    while let Some(c) = before.filter(|p| p.kind() == "comment") {
        before = c.prev_named_sibling(); // a doc comment between overloads
    }
    let overload = before.map(unwrap_decl).is_some_and(|prev| {
        matches!(prev.kind(), "function_signature" | "method_signature")
            && field_text(prev, "name", src) == Some(name)
    });
    if !overload {
        facts.symbols.push(Symbol {
            name: name.to_string(),
            kind,
            line: line_of(start),
            signature: sig.clone(),
            container: container.map(String::from),
        });
    }
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
            container: None,
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
fn add_call(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
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
            caller: ctx.enclosing.map(String::from),
            form: call_form(f, src, ctx.modules),
            owner: ctx.container.map(String::from),
        });
    }
}

/// How a call is written: `f()`, `this.f()`, `Foo.f()`, `ns.f()` or `value.f()`.
fn call_form(function: Node, src: &str, modules: &HashMap<String, String>) -> Form {
    if function.kind() == "identifier" {
        return match modules.get(text(function, src)) {
            Some(m) => Form::Module(m.clone()),
            None => Form::Bare,
        };
    }
    let Some(object) = function.child_by_field_name("object") else {
        return Form::Unknown;
    };
    match object.kind() {
        "this" => Form::SelfRef,
        "identifier" => {
            let name = text(object, src);
            if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                Form::Type(name.to_string()) // a class, even when imported
            } else if let Some(m) = modules.get(name) {
                Form::Module(m.clone())
            } else {
                Form::Method
            }
        }
        // `z.util.f()`: a sub-module of an imported namespace.
        "member_expression" => match namespace_root(object, src, modules) {
            Some(sub) => Form::Module(sub.to_string()),
            None => Form::Method,
        },
        _ => Form::Method,
    }
}

/// For `ns.sub` (or `ns.a.sub`) where `ns` is an imported namespace: the last name, `sub`.
fn namespace_root<'a>(
    mut object: Node<'a>,
    src: &'a str,
    modules: &HashMap<String, String>,
) -> Option<&'a str> {
    let sub = field_text(object, "property", src)?;
    while object.kind() == "member_expression" {
        object = object.child_by_field_name("object")?;
    }
    (object.kind() == "identifier" && modules.contains_key(text(object, src))).then_some(sub)
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
    use crate::model::{Form, Kind};

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

    fn members(src: &str) -> Vec<(String, Option<String>, u32)> {
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        facts
            .symbols
            .iter()
            .map(|s| (s.name.clone(), s.container.clone(), s.line))
            .collect()
    }

    fn sym(name: &str, container: Option<&str>, line: u32) -> (String, Option<String>, u32) {
        (name.into(), container.map(String::from), line)
    }

    #[test]
    fn methods_know_their_class_and_functions_have_none() {
        let got = members("class A {\n  run() {}\n}\nfunction free() {}\n");
        assert_eq!(
            got,
            vec![
                sym("A", None, 1),
                sym("run", Some("A"), 2),
                sym("free", None, 4)
            ]
        );
    }

    #[test]
    fn interface_method_signatures_are_symbols_of_the_interface() {
        let got = members("interface T {\n  parse(x: string): number;\n  size: number;\n}\n");
        assert_eq!(got, vec![sym("T", None, 1), sym("parse", Some("T"), 2)]);
    }

    #[test]
    fn abstract_methods_are_symbols_of_their_class() {
        let got = members("abstract class B {\n  abstract go(): void;\n}\n");
        assert_eq!(got, vec![sym("B", None, 1), sym("go", Some("B"), 2)]);
    }

    #[test]
    fn overloads_are_one_symbol_at_the_first_signature() {
        let src = "export function f(a: string): A;\nexport function f(a: number): B;\nexport function f(a: any) { return a; }\n";
        assert_eq!(members(src), vec![sym("f", None, 1)]);
        let in_class =
            "class C {\n  m(a: string): void;\n  m(a: number): void;\n  m(a: any) {}\n}\n";
        assert_eq!(
            members(in_class),
            vec![sym("C", None, 1), sym("m", Some("C"), 2)]
        );
    }

    #[test]
    fn doc_comments_between_overloads_do_not_split_them() {
        let top = "/** a */\nexport function f(a: string): A;\n/** b */\nexport function f(a: number): B;\nexport function f(a: any) {}\n";
        assert_eq!(members(top), vec![sym("f", None, 2)]);
        let class = "class C {\n  /** a */\n  m(a: string): void;\n  // b\n  m(a: any) {}\n}\n";
        assert_eq!(
            members(class),
            vec![sym("C", None, 1), sym("m", Some("C"), 3)]
        );
    }

    #[test]
    fn declare_overloads_are_one_symbol_too() {
        let src = "declare function f(a: string): A;\ndeclare function f(a: number): B;\nexport declare function g(a: string): A;\nexport declare function g(a: number): B;\n";
        assert_eq!(members(src), vec![sym("f", None, 1), sym("g", None, 3)]);
    }

    #[test]
    fn the_same_name_in_two_places_stays_two_symbols() {
        let got =
            members("function f() {}\nclass C {\n  f() {}\n}\nfunction g() {}\nfunction f() {}\n");
        assert_eq!(got.iter().filter(|s| s.0 == "f").count(), 3);
    }

    #[test]
    fn calls_inside_an_overloaded_function_keep_the_implementation_as_caller() {
        let facts = extract_file(
            Lang::TypeScript,
            "function f(a: string): A;\nfunction f(a: any) { return g(a); }\n",
        )
        .unwrap();
        assert_eq!(facts.calls[0].caller.as_deref(), Some("function f"));
    }

    fn forms(src: &str) -> Vec<(String, Form, Option<String>)> {
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        facts
            .calls
            .iter()
            .map(|c| (c.callee.clone(), c.form.clone(), c.owner.clone()))
            .collect()
    }

    fn form(name: &str, f: Form, owner: Option<&str>) -> (String, Form, Option<String>) {
        (name.into(), f, owner.map(String::from))
    }

    #[test]
    fn calls_record_how_they_are_written() {
        let src = "import * as z from './core/index.js';
import { helper, Klass } from './util';
class A {
  run() { this.go(); helper(); z.make(); B.build(); x.y(); local(); super.go(); Klass.make(); }
}
top();
";
        assert_eq!(
            forms(src),
            vec![
                form("go", Form::SelfRef, Some("A")),
                form("helper", Form::Module("util".into()), Some("A")),
                form("make", Form::Module("core".into()), Some("A")),
                form("build", Form::Type("B".into()), Some("A")),
                form("y", Form::Method, Some("A")),
                form("local", Form::Bare, Some("A")),
                form("go", Form::Method, Some("A")),
                form("make", Form::Type("Klass".into()), Some("A")),
                form("top", Form::Bare, None),
            ]
        );
    }

    #[test]
    fn a_nested_function_declaration_has_no_owner_but_an_arrow_keeps_it() {
        let src = "class A {\n  run() {\n    const f = () => this.a();\n    function g() { this.b(); }\n  }\n}\n";
        assert_eq!(
            forms(src),
            vec![
                form("a", Form::SelfRef, Some("A")),
                form("b", Form::SelfRef, None)
            ]
        );
    }

    #[test]
    fn a_namespace_import_is_a_module_even_from_a_package_or_alias() {
        let src = "import * as z from 'zod/v3';\nimport * as _ from '@scope/lodash';\nz.custom(); _.map();\n";
        assert_eq!(
            forms(src),
            vec![
                form("custom", Form::Module("v3".into()), None),
                form("map", Form::Module("lodash".into()), None)
            ]
        );
    }

    #[test]
    fn a_member_of_an_imported_namespace_is_a_module_of_that_name() {
        let src = "import * as z from './z';\nz.util.f(); this.items.push(1); other.util.g();\n";
        assert_eq!(
            forms(src),
            vec![
                form("f", Form::Module("util".into()), None),
                form("push", Form::Method, None),
                form("g", Form::Method, None)
            ]
        );
    }

    #[test]
    fn packages_and_aliased_imports_are_not_modules_of_ours() {
        let src = "import { map } from 'lodash';\nimport { a as b } from './x';\nmap(); b();\n";
        assert_eq!(
            forms(src),
            vec![form("map", Form::Bare, None), form("b", Form::Bare, None)]
        );
    }

    #[test]
    fn signatures_in_type_literals_are_not_symbols() {
        let src = "type T = { f(): void };\nfunction g(x: { cb(): void }) {}\ninterface I {\n  m(): void;\n}\n";
        let got: Vec<_> = members(src).into_iter().map(|s| s.0).collect();
        assert_eq!(got, vec!["T", "g", "I", "m"]);
    }

    #[test]
    fn getters_and_setters_and_nested_functions_are_not_overloads() {
        let src = "class C {\n  get x() { return 1; }\n  set x(v) {}\n}\nfunction f() {\n  function f() {}\n}\n";
        let got = members(src);
        assert_eq!(got.iter().filter(|s| s.0 == "x").count(), 2);
        assert_eq!(got.iter().filter(|s| s.0 == "f").count(), 2);
    }

    #[test]
    fn object_literal_and_class_expression_methods_have_their_own_container() {
        let src = "class A {\n  run() {\n    const o = { m() { this.a(); } };\n  }\n}\nconst B = class {\n  n() { this.b(); }\n};\n";
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        let owner = |name: &str| {
            facts
                .calls
                .iter()
                .find(|c| c.callee == name)
                .and_then(|c| c.owner.clone())
        };
        assert_eq!(owner("a"), None, "`this` is the object literal");
        assert_eq!(owner("b").as_deref(), Some("B"));
        let n = facts.symbols.iter().find(|s| s.name == "n").unwrap();
        assert_eq!(n.container.as_deref(), Some("B"));
    }

    #[test]
    fn builtin_namespaces_look_like_types() {
        // `Math.max()` is read as `Type(Math)`: no project type has that name, so nothing is ruled out.
        assert_eq!(
            forms("Math.max(1, 2);\n"),
            vec![form("max", Form::Type("Math".into()), None)]
        );
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
